//! 백그라운드 사진 폴더 스캔.
//!
//! 트리 루트들을 낮은 우선순위 스레드 하나로 천천히 훑어, 폴더마다 [`Verdict`] 를 정한다:
//! 하위 어디엔가 사진이 있음 / 확실히 없음 / 모름. **모르면 숨기지 않는다** — 건너뛴 폴더(심볼릭 링크·
//! 다른 루트·시스템 폴더·읽기 오류·아직 안 훑음)가 하나라도 있으면서 사진을 못 찾았다면 `Unknown` 이다.
//!
//! 사진 판정은 격자(`iv_library::Library::open`)와 같은 규칙: 숨김 아님 + 알려진 확장자(대소문자 무시).
//! 폴더 판정은 트리(`iv_fstree::list_subdirs`)와 같은 규칙: 점 폴더·번들은 트리에 없으므로 세지 않는다.
//!
//! UI 는 기다리지 않는다: 판정은 짧은 쓰기 잠금으로 하나씩 공개되고, UI 는 읽기 잠금으로 조회한다.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use iv_core::Gate;

#[cfg(target_os = "macos")]
mod watch;

/// 폴더 판정
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// 아직 모름 (또는 건너뛴 하위가 있어 확신 못 함) — 숨기지 않는다
    Unknown,
    /// 하위 어디에도 사진이 없음이 확인됨 — 숨겨도 됨
    Empty,
    /// 이 폴더나 하위 어딘가에 사진이 있음
    HasPhotos,
}

/// 상태줄에 보일 진행 상황
#[derive(Clone, Debug, Default)]
pub struct ScanStatus {
    pub done: bool,
    /// 훑은 폴더 수
    pub dirs: u64,
    /// 찾은 사진 수
    pub photos: u64,
    /// 사진이 직접 들어 있는 폴더 수
    pub photo_dirs: u64,
    /// 변경 감시 중 (FSEvents)
    pub watching: bool,
    /// 바뀐 폴더를 다시 훑는 중
    pub updating: bool,
}

impl ScanStatus {
    /// 상태줄 오른쪽 글
    pub fn text(&self) -> String {
        if self.done {
            let tail = if self.updating {
                "변경 반영 중…"
            } else if self.watching {
                "스캔 완료 · 변경 감시 중"
            } else {
                "스캔 완료"
            };
            format!("사진 {}장 · 폴더 {}곳  ·  {tail}", group(self.photos), group(self.photo_dirs))
        } else {
            format!("사진 찾는 중…  폴더 {}개 · 사진 {}장", group(self.dirs), group(self.photos))
        }
    }
}

/// 1234567 → "1,234,567"
fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub struct ScanConfig {
    /// 트리 루트 (트리에 보이는 경로 그대로 — 별칭은 스캐너가 정규화)
    pub roots: Vec<PathBuf>,
    /// 사진 확장자 (소문자)
    pub extensions: HashSet<&'static str>,
    /// 현재 장 디코드가 바쁘면 기다린다
    pub gate: Option<Arc<Gate>>,
    /// 변경 감시에서 버릴 경로 (썸네일 캐시 — 앱 자신의 쓰기가 다시 스캔을 부르지 않게)
    pub ignore: Vec<PathBuf>,
}

struct Shared {
    /// 정규 경로 → 사진 있음(true)/없음(false). 없는 키 = 모름.
    verdicts: RwLock<HashMap<PathBuf, bool>>,
    status: Mutex<ScanStatus>,
    stop: AtomicBool,
    /// 스캔 후 바뀐 폴더 (정규 경로) — 앱이 가져가 트리·격자를 맞춘다
    changed: Mutex<Vec<PathBuf>>,
}

pub struct Scanner {
    shared: Arc<Shared>,
    /// (트리에 보이는 경로, 정규 경로) — 예: /Volumes/Macintosh HD → /
    aliases: Vec<(PathBuf, PathBuf)>,
}

impl Scanner {
    /// 스캔 스레드를 띄운다. `wake` 는 진행이 있을 때 (최대 초당 몇 번) 불린다.
    pub fn start(cfg: ScanConfig, wake: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new(Shared { verdicts: RwLock::new(HashMap::new()), status: Mutex::new(ScanStatus::default()), stop: AtomicBool::new(false), changed: Mutex::new(Vec::new()) });
        //#region 루트 정규화 — 별칭 기록, 중복 제거, 깊은 루트부터 (조상 루트가 그 결과를 쓸 수 있게)
        let mut aliases = Vec::new();
        let mut roots: Vec<PathBuf> = Vec::new();
        for r in &cfg.roots {
            let Ok(c) = r.canonicalize() else { continue };
            if &c != r {
                aliases.push((r.clone(), c.clone()));
            }
            if !roots.contains(&c) {
                roots.push(c);
            }
        }
        roots.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
        //#endregion
        let s = shared.clone();
        let spawned = std::thread::Builder::new().name("iv-scan".into()).spawn(move || {
            set_background_priority();
            let ignore: Vec<PathBuf> = cfg.ignore.iter().filter_map(|p| p.canonicalize().ok()).collect();
            //#region 감시를 먼저 켠다 — 첫 훑기 동안의 변경도 놓치지 않게 (/ 는 시스템 변경이 잦아 감시 안 함)
            let (tx, rx) = std::sync::mpsc::channel();
            #[cfg(target_os = "macos")]
            let watching = {
                let mut ig = ignore.clone();
                ig.extend(std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library")));
                watch::watch(roots.iter().filter(|r| r.as_path() != Path::new("/") && is_local_volume(r)).cloned().collect(), ig, tx)
            };
            #[cfg(not(target_os = "macos"))]
            let watching = {
                drop(tx);
                false
            };
            //#endregion
            let t0 = Instant::now();
            let mut w = Walker { shared: s.clone(), roots: roots.clone(), extensions: cfg.extensions, gate: cfg.gate, ignore, wake: Box::new(wake), last_wake: Instant::now(), count: 0, counts: HashMap::new() };
            for r in &roots {
                if w.stopped() {
                    return;
                }
                if !is_local_volume(r) {
                    log::info!("스캔: 로컬이 아닌 볼륨은 건너뜀");
                    continue;
                }
                w.walk(r, true);
            }
            let st = {
                let mut st = s.status.lock().unwrap_or_else(|e| e.into_inner());
                st.done = true;
                st.watching = watching;
                st.clone()
            };
            log::info!("스캔 완료: 폴더 {} · 사진 {} · {:.1}s · 감시 {}", st.dirs, st.photos, t0.elapsed().as_secs_f64(), if watching { "켬" } else { "끔" });
            (w.wake)();
            if watching {
                w.follow(&rx);
            }
        });
        if let Err(e) = spawned {
            log::warn!("스캔 스레드 생성 실패: {e}");
        }
        Self { shared, aliases }
    }

    /// 트리 경로의 판정 (매 프레임 불려도 되게 가볍게 — 파일 시스템을 건드리지 않는다)
    pub fn verdict(&self, p: &Path) -> Verdict {
        let mapped;
        let key = match self.aliases.iter().find(|(a, _)| p.starts_with(a)) {
            Some((a, c)) => {
                mapped = c.join(p.strip_prefix(a).unwrap_or(Path::new("")));
                mapped.as_path()
            }
            None => p,
        };
        let v = self.shared.verdicts.read().unwrap_or_else(|e| e.into_inner());
        match v.get(key) {
            Some(true) => Verdict::HasPhotos,
            Some(false) => Verdict::Empty,
            None => Verdict::Unknown,
        }
    }

    pub fn status(&self) -> ScanStatus {
        self.shared.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 마지막으로 가져간 뒤 바뀐 폴더들 (정규 경로)
    pub fn take_changed(&self) -> Vec<PathBuf> {
        std::mem::take(&mut *self.shared.changed.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl Drop for Scanner {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

struct Walker {
    shared: Arc<Shared>,
    roots: Vec<PathBuf>,
    extensions: HashSet<&'static str>,
    gate: Option<Arc<Gate>>,
    ignore: Vec<PathBuf>,
    wake: Box<dyn Fn() + Send>,
    last_wake: Instant,
    count: u64,
    /// 폴더 → 직접 든 사진 수 (상태줄 합계를 변경 때 바로잡기 위해)
    counts: HashMap<PathBuf, u64>,
}

/// 한 폴더 나열 결과
struct Listing {
    photos: u64,
    subdirs: Vec<PathBuf>,
    /// 판정에 넣지 못한 하위가 있음 (폴더 링크·종류를 모름)
    unknown_child: bool,
}

impl Walker {
    fn stopped(&self) -> bool {
        self.shared.stop.load(Ordering::Relaxed)
    }

    //#region 양보 — 현재 장 디코드가 바쁘면 기다리고, 조금씩 쉰다 (기능에 영향 없게)
    fn yield_now(&mut self) {
        if let Some(g) = &self.gate {
            g.wait_idle(Duration::from_secs(2));
        }
        self.count += 1;
        if self.count % 64 == 0 {
            std::thread::sleep(Duration::from_millis(3));
        }
    }
    //#endregion

    //#region 한 폴더 나열 — 사진 세기(격자 규칙), 하위 폴더 모으기(트리 규칙)
    fn list(&self, dir: &Path) -> Option<Listing> {
        let rd = std::fs::read_dir(dir).ok()?;
        let mut l = Listing { photos: 0, subdirs: Vec::new(), unknown_child: false };
        for e in rd.flatten() {
            let name = e.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let path = e.path();
            let Ok(ft) = e.file_type() else {
                l.unknown_child = true;
                continue;
            };
            if ft.is_symlink() {
                // 링크는 따라가지 않는다: 폴더를 가리키면 트리에 보이므로 "모름", 파일이면 격자처럼 사진으로 센다
                match std::fs::metadata(&path) {
                    Ok(m) if m.is_dir() => l.unknown_child |= !is_bundle(&path),
                    Ok(m) if m.is_file() => l.photos += u64::from(self.is_photo(&path)),
                    _ => {}
                }
            } else if ft.is_dir() {
                if !is_bundle(&path) {
                    l.subdirs.push(path);
                }
            } else if ft.is_file() {
                l.photos += u64::from(self.is_photo(&path));
            }
        }
        Some(l)
    }
    //#endregion

    //#region 판정·합계 기록
    fn set_verdict(&self, dir: &Path, v: Verdict) {
        let mut m = self.shared.verdicts.write().unwrap_or_else(|e| e.into_inner());
        match v {
            Verdict::Unknown => {
                m.remove(dir);
            }
            v => {
                m.insert(dir.to_path_buf(), v == Verdict::HasPhotos);
            }
        }
    }

    fn record_count(&mut self, dir: &Path, photos: u64) {
        let old = self.counts.insert(dir.to_path_buf(), photos);
        let mut st = self.shared.status.lock().unwrap_or_else(|e| e.into_inner());
        if old.is_none() {
            st.dirs += 1;
        }
        let old = old.unwrap_or(0);
        st.photos = (st.photos + photos).saturating_sub(old);
        st.photo_dirs = (st.photo_dirs + u64::from(photos > 0)).saturating_sub(u64::from(old > 0));
    }

    /// 사라진 폴더(와 하위)를 잊는다
    fn forget(&mut self, dir: &Path) {
        self.shared.verdicts.write().unwrap_or_else(|e| e.into_inner()).retain(|k, _| !k.starts_with(dir));
        let gone: Vec<PathBuf> = self.counts.keys().filter(|k| k.starts_with(dir)).cloned().collect();
        let mut st = self.shared.status.lock().unwrap_or_else(|e| e.into_inner());
        for k in gone {
            if let Some(c) = self.counts.remove(&k) {
                st.dirs = st.dirs.saturating_sub(1);
                st.photos = st.photos.saturating_sub(c);
                st.photo_dirs = st.photo_dirs.saturating_sub(u64::from(c > 0));
            }
        }
    }

    fn maybe_wake(&mut self) {
        if self.last_wake.elapsed() > Duration::from_millis(500) {
            self.last_wake = Instant::now();
            (self.wake)();
        }
    }
    //#endregion

    /// 폴더 하나를 (하위까지) 훑고 판정을 돌려준다
    fn walk(&mut self, dir: &Path, is_root: bool) -> Verdict {
        if self.stopped() {
            return Verdict::Unknown;
        }
        self.yield_now();
        let Some(l) = self.list(dir) else {
            self.set_verdict(dir, Verdict::Unknown);
            return Verdict::Unknown;
        };
        self.record_count(dir, l.photos);
        //#region 하위 판정 — 다른 루트·시스템 폴더는 들어가지 않고 이미 아는 판정(없으면 모름)을 쓴다
        let mut any_photos = l.photos > 0;
        let mut unknown = l.unknown_child;
        for sub in l.subdirs {
            let v = if self.skip(&sub, is_root.then_some(dir)) { self.known(&sub) } else { self.walk(&sub, false) };
            match v {
                Verdict::HasPhotos => any_photos = true,
                Verdict::Unknown => unknown = true,
                Verdict::Empty => {}
            }
        }
        //#endregion
        let v = combine(any_photos, unknown || self.stopped());
        self.set_verdict(dir, v);
        self.maybe_wake();
        v
    }

    /// 한 폴더만 다시 판정 (하위 판정은 아는 것을 쓴다). `walk_new` 면 모르는 하위(새 폴더)는 훑고, 사라진 하위는 잊는다.
    fn shallow(&mut self, dir: &Path, walk_new: bool) -> Verdict {
        let is_root = self.roots.iter().any(|r| r == dir);
        let Some(l) = self.list(dir) else {
            self.set_verdict(dir, Verdict::Unknown);
            return Verdict::Unknown;
        };
        self.record_count(dir, l.photos);
        if walk_new {
            let gone: Vec<PathBuf> = self.counts.keys().filter(|k| k.parent() == Some(dir) && !l.subdirs.contains(k)).cloned().collect();
            for g in gone {
                self.forget(&g);
            }
        }
        let mut any_photos = l.photos > 0;
        let mut unknown = l.unknown_child;
        for sub in &l.subdirs {
            let skip = self.skip(sub, is_root.then_some(dir));
            let mut v = self.known(sub);
            if v == Verdict::Unknown && walk_new && !skip {
                v = self.walk(sub, false);
            }
            match v {
                Verdict::HasPhotos => any_photos = true,
                Verdict::Unknown => unknown = true,
                Verdict::Empty => {}
            }
        }
        let v = combine(any_photos, unknown);
        self.set_verdict(dir, v);
        v
    }

    //#region 변경 따라가기 — 바뀐 폴더만 다시 훑고 조상 판정을 바로잡는다
    fn follow(&mut self, rx: &std::sync::mpsc::Receiver<DirEvent>) {
        use std::sync::mpsc::RecvTimeoutError;
        loop {
            let first = match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(e) => e,
                Err(RecvTimeoutError::Timeout) => {
                    if self.stopped() {
                        return;
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return,
            };
            // 잠깐 모아서 한 번에 (연속 저장·복사)
            std::thread::sleep(Duration::from_millis(200));
            let mut batch = vec![first];
            batch.extend(rx.try_iter());
            let mut todo: Vec<DirEvent> = batch.into_iter().filter_map(|e| self.normalize(e)).collect();
            if todo.is_empty() {
                continue;
            }
            todo.sort_by_key(|e| e.dir.components().count());
            let mut kept: Vec<DirEvent> = Vec::new();
            for e in todo {
                // 같은 폴더 중복, 또는 조상을 이미 깊게 다시 훑을 예정이면 생략
                if let Some(k) = kept.iter_mut().find(|k| k.dir == e.dir) {
                    k.deep |= e.deep;
                } else if !kept.iter().any(|k| k.deep && e.dir.starts_with(&k.dir)) {
                    kept.push(e);
                }
            }
            self.shared.status.lock().unwrap_or_else(|e| e.into_inner()).updating = true;
            (self.wake)();
            for e in &kept {
                if self.stopped() {
                    return;
                }
                self.yield_now();
                self.rescan(&e.dir, e.deep);
            }
            log::debug!("스캔: 바뀐 폴더 {}곳 반영", kept.len());
            self.shared.changed.lock().unwrap_or_else(|e| e.into_inner()).extend(kept.into_iter().map(|e| e.dir));
            self.shared.status.lock().unwrap_or_else(|e| e.into_inner()).updating = false;
            self.last_wake = Instant::now();
            (self.wake)();
        }
    }

    /// 이벤트 경로를 스캔 범위의 정규 경로로 (범위 밖·건너뛰는 곳이면 None)
    fn normalize(&self, mut e: DirEvent) -> Option<DirEvent> {
        if !self.roots.iter().any(|r| e.dir.starts_with(r)) {
            // APFS 데이터 볼륨 경로로 올 수 있다
            let stripped = e.dir.strip_prefix("/System/Volumes/Data").ok().map(|p| Path::new("/").join(p))?;
            e.dir = stripped;
        }
        if self.ignore.iter().any(|ig| e.dir.starts_with(ig)) {
            return None;
        }
        let root = self.roots.iter().filter(|r| e.dir.starts_with(r)).max_by_key(|r| r.components().count())?.clone();
        // 루트부터 내려가며 건너뛰는 폴더(시스템·~/Library·번들·점 폴더) 아래면 버린다
        let rel = e.dir.strip_prefix(&root).ok()?.to_path_buf();
        let mut cur = root.clone();
        for c in rel.components() {
            let next = cur.join(c);
            let hidden = c.as_os_str().to_string_lossy().starts_with('.');
            if hidden || is_bundle(&next) || self.skip(&next, (cur == root).then_some(cur.as_path())) {
                return None;
            }
            cur = next;
        }
        Some(e)
    }

    fn rescan(&mut self, dir: &Path, deep: bool) {
        let is_root = self.roots.iter().any(|r| r == dir);
        if !dir.is_dir() {
            self.forget(dir);
        } else if deep {
            self.forget(dir);
            self.walk(dir, is_root);
        } else {
            self.shallow(dir, true);
        }
        // 조상들의 판정을 바로잡는다 (루트까지)
        if is_root {
            return;
        }
        let mut cur = dir.parent().map(Path::to_path_buf);
        while let Some(a) = cur {
            if !self.roots.iter().any(|r| a.starts_with(r)) {
                break;
            }
            self.shallow(&a, false);
            if self.roots.contains(&a) {
                break;
            }
            cur = a.parent().map(Path::to_path_buf);
        }
    }
    //#endregion

    fn known(&self, p: &Path) -> Verdict {
        match self.shared.verdicts.read().unwrap_or_else(|e| e.into_inner()).get(p) {
            Some(true) => Verdict::HasPhotos,
            Some(false) => Verdict::Empty,
            None => Verdict::Unknown,
        }
    }

    /// 들어가지 않을 폴더: 다른 루트(따로 훑는다), 시스템 루트(/) 아래의 시스템 폴더, 홈의 Library
    fn skip(&self, p: &Path, root_parent: Option<&Path>) -> bool {
        if self.roots.iter().any(|r| r == p) {
            return true;
        }
        if root_parent == Some(Path::new("/")) {
            let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if matches!(n, "System" | "Library" | "Applications" | "private" | "usr" | "bin" | "sbin" | "opt" | "cores" | "dev" | "Volumes" | "etc" | "tmp" | "var") {
                return true;
            }
        }
        p.file_name().is_some_and(|n| n == "Library") && p.parent().is_some_and(|par| std::env::var_os("HOME").is_some_and(|h| par == Path::new(&h)))
    }

    fn is_photo(&self, p: &Path) -> bool {
        p.extension().and_then(|e| e.to_str()).is_some_and(|e| self.extensions.contains(e.to_ascii_lowercase().as_str()))
    }
}

/// 사진을 찾았으면 있음, 아니면 확신 못 하는 하위가 있을 때 모름, 그 밖엔 없음
fn combine(any_photos: bool, unknown: bool) -> Verdict {
    if any_photos {
        Verdict::HasPhotos
    } else if unknown {
        Verdict::Unknown
    } else {
        Verdict::Empty
    }
}

#[cfg(not(target_os = "macos"))]
#[derive(Debug, Clone)]
struct DirEvent {
    dir: PathBuf,
    deep: bool,
}
#[cfg(target_os = "macos")]
use watch::DirEvent;

/// 트리(`iv_fstree`)와 같은 번들 목록 — 트리에 없는 폴더는 판정에도 넣지 않는다
fn is_bundle(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e, "app" | "bundle" | "framework" | "photoslibrary" | "pkg"))
}

//#region 플랫폼 — 낮은 우선순위(맥: QoS background = CPU·디스크 I/O 모두 양보), 로컬 볼륨 판별
#[cfg(target_os = "macos")]
fn set_background_priority() {
    // SAFETY: 현재 스레드의 QoS 만 바꾸는 libc 호출
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_BACKGROUND, 0);
    }
}

#[cfg(not(target_os = "macos"))]
fn set_background_priority() {}

#[cfg(target_os = "macos")]
fn is_local_volume(p: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { return false };
    // SAFETY: statfs 는 c 경로를 읽고 buf 를 채운다
    unsafe {
        let mut buf: libc::statfs = std::mem::zeroed();
        libc::statfs(c.as_ptr(), &mut buf) == 0 && (buf.f_flags & libc::MNT_LOCAL as u32) != 0
    }
}

#[cfg(not(target_os = "macos"))]
fn is_local_volume(_p: &Path) -> bool {
    true
}
//#endregion

#[cfg(test)]
mod tests {
    use super::*;

    /// 시험 트리:
    /// root/photo/a.JPG · root/empty/ · root/deep/x/y/b.png · root/text/notes.txt
    /// root/.dot/c.jpg(숨김 — 셈 안 함) · root/mixed/(빈 하위 + 링크 폴더) · root/onlybundle/z.app/d.jpg
    fn tree(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iv-scan-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for s in ["photo", "empty", "deep/x/y", "text", ".dot", "mixed/sub", "onlybundle/z.app", "linkonly"] {
            std::fs::create_dir_all(d.join(s)).unwrap();
        }
        for f in ["photo/a.JPG", "deep/x/y/b.png", ".dot/c.jpg", "onlybundle/z.app/d.jpg"] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        std::fs::write(d.join("text/notes.txt"), b"x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(d.join("photo"), d.join("linkonly/to-photo")).unwrap();
        d
    }

    fn run(d: &Path) -> Scanner {
        let (tx, rx) = std::sync::mpsc::channel();
        let s = Scanner::start(ScanConfig { roots: vec![d.to_path_buf()], extensions: HashSet::from(["jpg", "png"]), gate: None, ignore: Vec::new() }, move || {
            let _ = tx.send(());
        });
        let until = Instant::now() + Duration::from_secs(10);
        while !s.status().done && Instant::now() < until {
            let _ = rx.recv_timeout(Duration::from_millis(50));
        }
        assert!(s.status().done);
        s
    }

    #[test]
    fn verdicts_never_hide_photos() {
        let d = tree("v");
        let c = d.canonicalize().unwrap();
        let s = run(&d);
        let v = |p: &str| s.verdict(&c.join(p));
        assert_eq!(v("photo"), Verdict::HasPhotos);
        assert_eq!(v("deep"), Verdict::HasPhotos);
        assert_eq!(v("deep/x/y"), Verdict::HasPhotos);
        assert_eq!(v("empty"), Verdict::Empty);
        assert_eq!(v("text"), Verdict::Empty);
        assert_eq!(v("mixed"), Verdict::Empty);
        // 번들 안 사진은 트리·격자 어디에도 안 보이므로 빈 폴더
        assert_eq!(v("onlybundle"), Verdict::Empty);
        // 폴더 링크는 따라가지 않으므로 모름 → 숨기지 않음
        assert_eq!(v("linkonly"), Verdict::Unknown);
        assert_eq!(s.verdict(&c), Verdict::HasPhotos);
        let st = s.status();
        assert_eq!(st.photos, 2);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn alias_root_maps_to_canonical() {
        let d = tree("a");
        let alias = std::env::temp_dir().join(format!("iv-scan-alias-{}", std::process::id()));
        let _ = std::fs::remove_file(&alias);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&d, &alias).unwrap();
        let s = run(&alias);
        assert_eq!(s.verdict(&alias.join("empty")), Verdict::Empty);
        assert_eq!(s.verdict(&alias.join("photo")), Verdict::HasPhotos);
        let _ = std::fs::remove_file(&alias);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 변경 감시: 빈 폴더에 사진이 생기면 있음으로, 지우면 다시 없음으로. 새 폴더도 판정된다.
    #[cfg(target_os = "macos")]
    #[test]
    fn follows_changes() {
        let d = tree("w");
        let c = d.canonicalize().unwrap();
        let s = run(&d);
        assert!(s.status().watching);
        let wait = |want: Verdict, p: &Path| {
            let until = Instant::now() + Duration::from_secs(10);
            while s.verdict(p) != want && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(50));
            }
            s.verdict(p)
        };
        // FSEvents 스트림이 자리 잡을 시간
        std::thread::sleep(Duration::from_millis(500));
        std::fs::write(c.join("empty/new.jpg"), b"x").unwrap();
        assert_eq!(wait(Verdict::HasPhotos, &c.join("empty")), Verdict::HasPhotos);
        std::fs::remove_file(c.join("empty/new.jpg")).unwrap();
        assert_eq!(wait(Verdict::Empty, &c.join("empty")), Verdict::Empty);
        std::fs::create_dir_all(c.join("text/sub")).unwrap();
        std::fs::write(c.join("text/sub/z.png"), b"x").unwrap();
        assert_eq!(wait(Verdict::HasPhotos, &c.join("text")), Verdict::HasPhotos);
        assert_eq!(s.verdict(&c.join("text/sub")), Verdict::HasPhotos);
        assert!(!s.take_changed().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn status_text() {
        assert_eq!(group(1234567), "1,234,567");
        assert_eq!(group(12), "12");
    }
}
