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
}

impl ScanStatus {
    /// 상태줄 오른쪽 글
    pub fn text(&self) -> String {
        if self.done {
            format!("사진 {}장 · 폴더 {}곳  ·  스캔 완료", group(self.photos), group(self.photo_dirs))
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
}

struct Shared {
    /// 정규 경로 → 사진 있음(true)/없음(false). 없는 키 = 모름.
    verdicts: RwLock<HashMap<PathBuf, bool>>,
    status: Mutex<ScanStatus>,
    stop: AtomicBool,
}

pub struct Scanner {
    shared: Arc<Shared>,
    /// (트리에 보이는 경로, 정규 경로) — 예: /Volumes/Macintosh HD → /
    aliases: Vec<(PathBuf, PathBuf)>,
}

impl Scanner {
    /// 스캔 스레드를 띄운다. `wake` 는 진행이 있을 때 (최대 초당 몇 번) 불린다.
    pub fn start(cfg: ScanConfig, wake: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new(Shared { verdicts: RwLock::new(HashMap::new()), status: Mutex::new(ScanStatus::default()), stop: AtomicBool::new(false) });
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
            let t0 = Instant::now();
            let mut w = Walker { shared: s.clone(), roots: roots.clone(), extensions: cfg.extensions, gate: cfg.gate, wake: Box::new(wake), last_wake: Instant::now(), count: 0 };
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
                st.clone()
            };
            log::info!("스캔 완료: 폴더 {} · 사진 {} · {:.1}s", st.dirs, st.photos, t0.elapsed().as_secs_f64());
            (w.wake)();
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
    wake: Box<dyn Fn() + Send>,
    last_wake: Instant,
    count: u64,
}

impl Walker {
    fn stopped(&self) -> bool {
        self.shared.stop.load(Ordering::Relaxed)
    }

    /// 폴더 하나를 (하위까지) 훑고 판정을 돌려준다
    fn walk(&mut self, dir: &Path, is_root: bool) -> Verdict {
        if self.stopped() {
            return Verdict::Unknown;
        }
        //#region 양보 — 현재 장 디코드가 바쁘면 기다리고, 조금씩 쉰다 (기능에 영향 없게)
        if let Some(g) = &self.gate {
            g.wait_idle(Duration::from_secs(2));
        }
        self.count += 1;
        if self.count % 64 == 0 {
            std::thread::sleep(Duration::from_millis(3));
        }
        //#endregion
        let Ok(rd) = std::fs::read_dir(dir) else { return Verdict::Unknown };
        let mut photos = 0u64;
        let mut subdirs = Vec::new();
        let mut unknown_child = false;
        //#region 한 폴더 나열 — 사진 세기, 하위 폴더 모으기
        for e in rd.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            let path = e.path();
            let Ok(ft) = e.file_type() else {
                unknown_child = true;
                continue;
            };
            if ft.is_symlink() {
                // 링크는 따라가지 않는다: 폴더를 가리키면 트리에 보이므로 "모름", 파일이면 격자처럼 사진으로 센다
                match std::fs::metadata(&path) {
                    Ok(m) if m.is_dir() => unknown_child |= !is_bundle(&path),
                    Ok(m) if m.is_file() => photos += u64::from(self.is_photo(&path)),
                    _ => {}
                }
            } else if ft.is_dir() {
                if !is_bundle(&path) {
                    subdirs.push(path);
                }
            } else if ft.is_file() {
                photos += u64::from(self.is_photo(&path));
            }
        }
        //#endregion
        {
            let mut st = self.shared.status.lock().unwrap_or_else(|e| e.into_inner());
            st.dirs += 1;
            st.photos += photos;
            st.photo_dirs += u64::from(photos > 0);
        }
        //#region 하위 판정 — 다른 루트·시스템 폴더는 들어가지 않고 이미 아는 판정(없으면 모름)을 쓴다
        let mut any_photos = photos > 0;
        for sub in subdirs {
            let v = if self.skip(&sub, is_root.then_some(dir)) { self.known(&sub) } else { self.walk(&sub, false) };
            match v {
                Verdict::HasPhotos => any_photos = true,
                Verdict::Unknown => unknown_child = true,
                Verdict::Empty => {}
            }
        }
        //#endregion
        let v = if any_photos {
            Verdict::HasPhotos
        } else if unknown_child || self.stopped() {
            Verdict::Unknown
        } else {
            Verdict::Empty
        };
        if v != Verdict::Unknown {
            self.shared.verdicts.write().unwrap_or_else(|e| e.into_inner()).insert(dir.to_path_buf(), v == Verdict::HasPhotos);
        }
        if self.last_wake.elapsed() > Duration::from_millis(500) {
            self.last_wake = Instant::now();
            (self.wake)();
        }
        v
    }

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
        let s = Scanner::start(ScanConfig { roots: vec![d.to_path_buf()], extensions: HashSet::from(["jpg", "png"]), gate: None }, move || {
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

    #[test]
    fn status_text() {
        assert_eq!(group(1234567), "1,234,567");
        assert_eq!(group(12), "12");
    }
}
