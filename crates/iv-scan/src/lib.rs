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
use std::time::{Duration, Instant, SystemTime};

use iv_core::Gate;

mod persist;
#[cfg(target_os = "macos")]
mod watch;

use persist::Cached;

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

/// 폴더 하나의 스캔 결과 (트리 옆 숫자)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirInfo {
    pub verdict: Verdict,
    /// 이 폴더에 직접 든 사진 수
    pub photos: u32,
    /// 트리에 보일 하위 폴더 수 (사진 없음이 확인된 것은 뺀다)
    pub folders: u32,
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
    /// 저장된 결과로 시작해 수정 시각만 확인하는 중
    pub from_cache: bool,
    /// 첫 훑기에서 실제로 다시 나열한 폴더 수 (나머지는 수정 시각만 확인)
    pub relisted: u64,
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
        } else if self.from_cache {
            format!("저장된 결과 확인 중…  폴더 {}개", group(self.dirs))
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
    /// 스캔 결과 저장 파일 — 다음 실행 때 폴더 수정 시각만 확인한다 (None 이면 매번 전부 나열)
    pub cache_file: Option<PathBuf>,
}

struct Shared {
    /// 정규 경로 → 스캔 결과. 없는 키 = 아직 모름.
    verdicts: RwLock<HashMap<PathBuf, DirInfo>>,
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
            //#region 저장된 결과 — 불러오자마자 판정을 공개해 트리가 바로 정리된다
            let mut exts: Vec<&str> = cfg.extensions.iter().copied().collect();
            exts.sort_unstable();
            let sig = exts.join(",");
            let prev = cfg.cache_file.as_deref().map(|f| persist::load(f, &sig)).unwrap_or_default();
            if !prev.is_empty() {
                let mut v = s.verdicts.write().unwrap_or_else(|e| e.into_inner());
                for (d, c) in &prev {
                    v.insert(d.clone(), DirInfo { verdict: c.verdict, photos: c.photos.min(u32::MAX as u64) as u32, folders: c.folders });
                }
                drop(v);
                s.status.lock().unwrap_or_else(|e| e.into_inner()).from_cache = true;
                log::info!("스캔: 저장된 결과 {}개 폴더 불러옴", prev.len());
                (wake)();
            }
            //#endregion
            let t0 = Instant::now();
            let unwatched = roots.iter().filter(|r| !watching || r.as_path() == Path::new("/") || !is_local_volume(r)).cloned().collect();
            let mut w = Walker { shared: s.clone(), roots: roots.clone(), extensions: cfg.extensions, gate: cfg.gate, ignore, wake: Box::new(wake), last_wake: Instant::now(), count: 0, counts: HashMap::new(), unwatched, mtime_trusted: roots.iter().filter(|r| dir_mtime_reliable(r)).cloned().collect(), prev, cur: HashMap::new(), cache_file: cfg.cache_file, sig, dirty: false, last_save: Instant::now(), listed: 0 };
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
            // 저장을 먼저 — '완료'를 본 앱이 곧바로 끝나도 결과가 남게. 이번에 들르지 않은 옛 항목은 버린다.
            w.prev = HashMap::new();
            if !w.stopped() {
                w.save();
            }
            let st = {
                let mut st = s.status.lock().unwrap_or_else(|e| e.into_inner());
                st.done = true;
                st.watching = watching;
                st.relisted = w.listed;
                st.clone()
            };
            log::info!("스캔 완료: 폴더 {} (다시 나열 {}) · 사진 {} · {:.1}s · 감시 {}", st.dirs, w.listed, st.photos, t0.elapsed().as_secs_f64(), if watching { "켬" } else { "끔" });
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
        self.info(p).map_or(Verdict::Unknown, |i| i.verdict)
    }

    /// 트리 경로의 스캔 결과 (아직 안 훑었으면 None)
    pub fn info(&self, p: &Path) -> Option<DirInfo> {
        let mapped;
        let key = match self.aliases.iter().find(|(a, _)| p.starts_with(a)) {
            Some((a, c)) => {
                mapped = c.join(p.strip_prefix(a).unwrap_or(Path::new("")));
                mapped.as_path()
            }
            None => p,
        };
        self.shared.verdicts.read().unwrap_or_else(|e| e.into_inner()).get(key).copied()
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
    /// 감시하지 않는 루트 (/ · 원격 볼륨 · 감시 실패 시 전부) — 여기선 "없음"을 공개하지 않는다
    unwatched: Vec<PathBuf>,
    /// 지난 실행의 저장 결과 (첫 훑기 동안만)
    prev: HashMap<PathBuf, Cached>,
    /// 이번 실행에서 확인한 결과 (저장 대상)
    cur: HashMap<PathBuf, Cached>,
    cache_file: Option<PathBuf>,
    sig: String,
    dirty: bool,
    last_save: Instant,
    /// 실제로 다시 나열한 폴더 수 (수정 시각이 바뀌었거나 처음 보는 폴더)
    listed: u64,
    /// 폴더 수정 시각을 믿을 수 있는 루트 (APFS·HFS+). FAT/exFAT 카드 등은 늘 다시 나열한다.
    mtime_trusted: Vec<PathBuf>,
}

/// 한 폴더 나열 결과
#[derive(Clone)]
struct Listing {
    photos: u64,
    subdirs: Vec<PathBuf>,
    /// 판정에 넣지 못한 하위가 있음 (종류를 모름)
    unknown_child: bool,
    /// 폴더를 가리키는 링크의 대상 (정규 경로) — 따라 들어가지 않고 대상의 판정을 빌린다
    links: Vec<PathBuf>,
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
    /// 폴더 수정 시각이 저장된 것과 같으면 저장된 나열을 쓰고, 다르면 다시 나열한다
    fn list(&mut self, dir: &Path) -> Option<(Listing, SystemTime)> {
        let mtime = std::fs::metadata(dir).ok()?.modified().ok()?;
        let trusted = self.mtime_trusted.iter().any(|r| dir.starts_with(r));
        if let Some(c) = self.cur.get(dir).or_else(|| self.prev.get(dir)).filter(|c| trusted && c.mtime == mtime) {
            return Some((Listing { photos: c.photos, subdirs: c.subdirs.clone(), unknown_child: c.unknown_child, links: c.links.clone() }, mtime));
        }
        self.listed += 1;
        self.read_listing(dir).map(|l| (l, mtime))
    }

    fn read_listing(&self, dir: &Path) -> Option<Listing> {
        let rd = std::fs::read_dir(dir).ok()?;
        let mut l = Listing { photos: 0, subdirs: Vec::new(), unknown_child: false, links: Vec::new() };
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
                    Ok(m) if m.is_dir() && !is_bundle(&path) => match path.canonicalize() {
                        Ok(t) => l.links.push(t),
                        Err(_) => l.unknown_child = true,
                    },
                    Ok(m) if m.is_file() => l.photos += u64::from(self.is_photo(&path)),
                    _ => {}
                }
            } else if ft.is_dir() {
                // 트리(list_subdirs)와 같은 규칙: 번들·Finder 숨김 폴더는 없는 셈
                if !is_bundle(&path) && !finder_hidden(&path) {
                    l.subdirs.push(path);
                }
            } else if ft.is_file() {
                l.photos += u64::from(self.is_photo(&path));
            }
        }
        Some(l)
    }
    //#endregion

    //#region 저장
    fn remember(&mut self, dir: &Path, mtime: SystemTime, l: &Listing, verdict: Verdict, folders: u32) {
        let c = Cached { mtime, photos: l.photos, unknown_child: l.unknown_child, subdirs: l.subdirs.clone(), links: l.links.clone(), verdict, folders };
        self.cur.insert(dir.to_path_buf(), c);
        self.dirty = true;
    }

    fn save(&mut self) {
        let Some(f) = self.cache_file.clone() else { return };
        match persist::save(&f, &self.sig, &self.cur) {
            Ok(()) => log::debug!("스캔 결과 저장: {}개 폴더", self.cur.len()),
            Err(e) => log::warn!("스캔 결과 저장 실패: {e}"),
        }
        self.dirty = false;
        self.last_save = Instant::now();
    }
    //#endregion

    //#region 판정·합계 기록
    /// 판정 공개. 돌려주는 값 = 실제로 공개한 판정 (저장도 이것으로 — 다음 실행에 그대로 공개되므로)
    fn set_info(&self, dir: &Path, v: Verdict, photos: u64, folders: u32) -> Verdict {
        // 감시하지 않는 곳의 "없음"은 나중에 사진이 생겨도 알 수 없으니 공개하지 않는다 (숨김이 굳지 않게)
        let verdict = if v == Verdict::Empty && !self.watched(dir) { Verdict::Unknown } else { v };
        let info = DirInfo { verdict, photos: photos.min(u32::MAX as u64) as u32, folders };
        self.shared.verdicts.write().unwrap_or_else(|e| e.into_inner()).insert(dir.to_path_buf(), info);
        verdict
    }

    /// 읽지 못한 폴더 — 숫자 없이 모름으로
    fn set_unreadable(&self, dir: &Path) {
        self.shared.verdicts.write().unwrap_or_else(|e| e.into_inner()).remove(dir);
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
        self.cur.retain(|k, _| !k.starts_with(dir));
        self.prev.retain(|k, _| !k.starts_with(dir));
        self.dirty = true;
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
        let Some((l, mtime)) = self.list(dir) else {
            self.set_unreadable(dir);
            return Verdict::Unknown;
        };
        self.record_count(dir, l.photos);
        //#region 하위 판정 — 다른 루트·시스템 폴더는 들어가지 않고 이미 아는 판정(없으면 모름)을 쓴다
        let mut any_photos = l.photos > 0;
        let mut unknown = l.unknown_child;
        let mut folders = 0u32;
        for t in &l.links {
            let v = self.link_verdict(t);
            folders += u32::from(v != Verdict::Empty);
            match v {
                Verdict::HasPhotos => any_photos = true,
                Verdict::Unknown => unknown = true,
                Verdict::Empty => {}
            }
        }
        let photos = l.photos;
        for sub in l.subdirs.clone() {
            let v = if self.is_system(&sub) {
                Verdict::Empty
            } else if self.skip(&sub, is_root.then_some(dir)) {
                self.known(&sub)
            } else {
                self.walk(&sub, false)
            };
            folders += u32::from(v != Verdict::Empty);
            match v {
                Verdict::HasPhotos => any_photos = true,
                Verdict::Unknown => unknown = true,
                Verdict::Empty => {}
            }
        }
        //#endregion
        let v = combine(any_photos, unknown || self.stopped());
        let published = self.set_info(dir, v, photos, folders);
        if !self.stopped() {
            self.remember(dir, mtime, &l, published, folders);
        }
        self.maybe_wake();
        v
    }

    /// 한 폴더만 다시 판정 (하위 판정은 아는 것을 쓴다). `walk_new` 면 모르는 하위(새 폴더)는 훑고, 사라진 하위는 잊는다.
    fn shallow(&mut self, dir: &Path, walk_new: bool) -> Verdict {
        let is_root = self.roots.iter().any(|r| r == dir);
        let Some((l, mtime)) = self.list(dir) else {
            self.set_unreadable(dir);
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
        let mut folders = 0u32;
        for t in &l.links {
            let v = self.link_verdict(t);
            folders += u32::from(v != Verdict::Empty);
            match v {
                Verdict::HasPhotos => any_photos = true,
                Verdict::Unknown => unknown = true,
                Verdict::Empty => {}
            }
        }
        for sub in &l.subdirs {
            let skip = self.skip(sub, is_root.then_some(dir));
            let mut v = if self.is_system(sub) { Verdict::Empty } else { self.known(sub) };
            if v == Verdict::Unknown && walk_new && !skip {
                v = self.walk(sub, false);
            }
            folders += u32::from(v != Verdict::Empty);
            match v {
                Verdict::HasPhotos => any_photos = true,
                Verdict::Unknown => unknown = true,
                Verdict::Empty => {}
            }
        }
        let v = combine(any_photos, unknown);
        let published = self.set_info(dir, v, l.photos, folders);
        self.remember(dir, mtime, &l, published, folders);
        v
    }

    //#region 변경 따라가기 — 바뀐 폴더만 다시 훑고 조상 판정을 바로잡는다
    fn follow(&mut self, rx: &std::sync::mpsc::Receiver<DirEvent>) {
        use std::sync::mpsc::RecvTimeoutError;
        loop {
            let first = match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(e) => e,
                Err(RecvTimeoutError::Timeout) => {
                    // 바뀐 게 있으면 10초에 한 번 저장 (끝낼 때도)
                    if self.dirty && (self.stopped() || self.last_save.elapsed() > Duration::from_secs(10)) {
                        self.save();
                    }
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
            if hidden || is_bundle(&next) || finder_hidden(&next) || self.skip(&next, (cur == root).then_some(cur.as_path())) {
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
        self.shared.verdicts.read().unwrap_or_else(|e| e.into_inner()).get(p).map_or(Verdict::Unknown, |i| i.verdict)
    }

    /// 들어가지 않을 폴더: 다른 루트(따로 훑는다), 시스템 루트(/) 아래의 시스템 폴더, 홈의 Library
    fn skip(&self, p: &Path, root_parent: Option<&Path>) -> bool {
        if self.roots.iter().any(|r| r == p) {
            return true;
        }
        // /Volumes 아래는 볼륨마다 따로 루트다
        root_parent == Some(Path::new("/")) && p == Path::new("/Volumes") || self.is_system(p)
    }

    /// 폴더 링크의 판정: 시스템 폴더면 없음, 이미 훑은 곳이면 그 판정, 아니면 모름
    fn link_verdict(&self, target: &Path) -> Verdict {
        if self.is_system(target) { Verdict::Empty } else { self.known(target) }
    }

    /// 시스템 폴더인가 — 단, 루트 자체가 시스템 폴더 안이면(예: /private/var 의 임시 폴더) 그 루트 아래는 예외
    fn is_system(&self, p: &Path) -> bool {
        is_system_dir(p) && !self.roots.iter().any(|r| p.starts_with(r) && is_system_dir(r))
    }

    /// FSEvents 로 감시되는 루트 아래인가 (가장 가까운 루트 기준)
    fn watched(&self, p: &Path) -> bool {
        self.roots.iter().filter(|r| p.starts_with(r)).max_by_key(|r| r.components().count()).is_some_and(|r| !self.unwatched.contains(r))
    }

    fn is_photo(&self, p: &Path) -> bool {
        p.extension().and_then(|e| e.to_str()).is_some_and(|e| self.extensions.contains(e.to_ascii_lowercase().as_str()))
    }
}

/// 사진 보관 장소가 아닌 시스템 폴더 — 정책상 "사진 없음" (들어가지 않는다).
/// 설치 디스크의 `Applications → /Applications` 링크 같은 것 때문에 빈 볼륨이 숨겨지지 않던 문제를 막는다.
fn is_system_dir(p: &Path) -> bool {
    const SYS: [&str; 13] = ["/System", "/Library", "/Applications", "/private", "/usr", "/bin", "/sbin", "/opt", "/cores", "/dev", "/etc", "/tmp", "/var"];
    SYS.iter().any(|s| p.starts_with(s)) || std::env::var_os("HOME").is_some_and(|h| p.starts_with(Path::new(&h).join("Library")))
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
/// Finder 숨김 표시(chflags hidden — ~/Library 등)가 있는 항목. Finder 처럼 트리에서 뺀다.
fn finder_hidden(p: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        const UF_HIDDEN: u32 = 0x8000;
        use std::os::macos::fs::MetadataExt;
        std::fs::symlink_metadata(p).is_ok_and(|m| m.st_flags() & UF_HIDDEN != 0)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = p;
        false
    }
}

fn is_bundle(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e, "app" | "bundle" | "framework" | "photoslibrary" | "pkg"))
}

//#region 플랫폼 — 낮은 우선순위(맥: QoS utility — UI 보다 낮음. background 는 I/O 를 너무 조여 stat 하나에 1.4ms, 5만 폴더 78초), 로컬 볼륨 판별
#[cfg(target_os = "macos")]
fn set_background_priority() {
    // SAFETY: 현재 스레드의 QoS 만 바꾸는 libc 호출
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
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

/// 하위 항목이 바뀌면 폴더 수정 시각이 반드시 바뀌는 파일 시스템인가 (저장된 나열을 믿어도 되는가)
#[cfg(target_os = "macos")]
fn dir_mtime_reliable(p: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { return false };
    // SAFETY: statfs 는 c 경로를 읽고 buf 를 채운다
    let name = unsafe {
        let mut buf: libc::statfs = std::mem::zeroed();
        if libc::statfs(c.as_ptr(), &mut buf) != 0 {
            return false;
        }
        std::ffi::CStr::from_ptr(buf.f_fstypename.as_ptr()).to_string_lossy().into_owned()
    };
    matches!(name.as_str(), "apfs" | "hfs")
}

#[cfg(not(target_os = "macos"))]
fn dir_mtime_reliable(_p: &Path) -> bool {
    false
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
        for s in ["photo", "empty", "deep/x/y", "text", ".dot", "mixed/sub", "onlybundle/z.app", "linkonly", "syslink"] {
            std::fs::create_dir_all(d.join(s)).unwrap();
        }
        for f in ["photo/a.JPG", "deep/x/y/b.png", ".dot/c.jpg", "onlybundle/z.app/d.jpg"] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        std::fs::write(d.join("text/notes.txt"), b"x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(d.join("photo"), d.join("linkonly/to-photo")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/Applications", d.join("syslink/Applications")).unwrap();
        d
    }

    fn run(d: &Path) -> Scanner {
        run_cached(d, None)
    }

    fn run_cached(d: &Path, cache_file: Option<PathBuf>) -> Scanner {
        let (tx, rx) = std::sync::mpsc::channel();
        let s = Scanner::start(ScanConfig { roots: vec![d.to_path_buf()], extensions: HashSet::from(["jpg", "png"]), gate: None, ignore: Vec::new(), cache_file }, move || {
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
        // 폴더 링크는 따라가지 않고 대상 판정을 빌린다 — 대상을 먼저 훑었으면 있음, 아니면 모름 (어느 쪽이든 숨기지 않음)
        assert_ne!(v("linkonly"), Verdict::Empty);
        // 시스템 폴더 링크(설치 디스크의 Applications)만 있는 폴더는 없음
        assert_eq!(v("syslink"), Verdict::Empty);
        assert_eq!(s.verdict(&c), Verdict::HasPhotos);
        let st = s.status();
        assert_eq!(st.photos, 2);
        // 트리 옆 숫자: 직접 든 사진, 보일 하위 폴더 (빈 폴더 empty·text·mixed·onlybundle·syslink 는 빼고 photo·deep·linkonly)
        assert_eq!(s.info(&c.join("photo")).map(|i| (i.photos, i.folders)), Some((1, 0)));
        assert_eq!(s.info(&c).map(|i| i.folders), Some(3));
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

    /// 저장 결과로 다시 시작: 판정은 곧바로, 다시 나열은 바뀐 폴더만
    #[test]
    fn cache_skips_unchanged_folders() {
        let d = tree("c");
        let c = d.canonicalize().unwrap();
        let file = std::env::temp_dir().join(format!("iv-scan-cache-{}.tsv", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let first = run_cached(&d, Some(file.clone()));
        let total = first.status().dirs;
        assert_eq!(first.status().relisted, total);
        drop(first);
        // 한 폴더만 바꾼다
        std::fs::write(c.join("empty/new.jpg"), b"x").unwrap();
        let second = run_cached(&d, Some(file.clone()));
        let st = second.status();
        assert!(st.from_cache);
        assert_eq!(st.dirs, total);
        assert_eq!(st.relisted, 1, "바뀐 폴더 하나만 다시 나열");
        assert_eq!(second.verdict(&c.join("empty")), Verdict::HasPhotos);
        assert_eq!(second.verdict(&c.join("text")), Verdict::Empty);
        assert_eq!(st.photos, 3);
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn status_text() {
        assert_eq!(group(1234567), "1,234,567");
        assert_eq!(group(12), "12");
    }
}
