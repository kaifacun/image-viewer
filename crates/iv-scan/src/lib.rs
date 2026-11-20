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
//!
//! **형식 필터**: 확장자는 형식 묶음(JPEG·PNG·RAW…, 설정의 `groups` 순서 = 비트 번호)으로 센다. 폴더마다
//! 직접 든 묶음별 수와 "하위 어딘가에 있는 묶음" 비트 표식을 두고, 판정은 조회할 때 표식과 필터를 대조한다
//! ([`Scanner::verdict_for`]) — 필터를 바꿔도 다시 훑지 않는다.

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

/// 하위까지 찾은 결과: 있는 형식 묶음 비트 + 확신 못 함
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// 이 폴더나 하위 어딘가에 있는 형식 묶음 (비트 = `ScanConfig::groups` 순서)
    pub mask: u64,
    /// 건너뛴 하위(링크·다른 루트·읽기 오류·아직 안 훑음)나 감시하지 않는 곳 — 표식에 없는 형식도 있을 수 있다
    pub unknown: bool,
}

impl Found {
    /// 모르는 폴더 (아직 안 훑음 등)
    pub const UNKNOWN: Self = Self { mask: 0, unknown: true };

    /// `filter` 묶음 기준 판정 — 맞는 게 있으면 있음, 없지만 확신 못 하면 모름, 아니면 없음
    pub fn verdict(self, filter: u64) -> Verdict {
        if self.mask & filter != 0 {
            Verdict::HasPhotos
        } else if self.unknown {
            Verdict::Unknown
        } else {
            Verdict::Empty
        }
    }

    fn merge(&mut self, o: Found) {
        self.mask |= o.mask;
        self.unknown |= o.unknown;
    }
}

/// 모든 묶음
pub const ALL: u64 = u64::MAX;

/// 폴더 하나의 스캔 결과
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirInfo {
    pub found: Found,
    /// 이 폴더에 직접 든 묶음별 사진 수 (`groups` 순서)
    pub direct: Vec<u32>,
    /// 트리에 보일 수 있는 하위 (하위 폴더 + 폴더 링크 대상, 시스템 폴더 제외) — 필터별 하위 수 계산용
    pub kids: Vec<PathBuf>,
}

impl DirInfo {
    /// `filter` 묶음의 직접 든 사진 수
    pub fn photos(&self, filter: u64) -> u32 {
        self.direct.iter().enumerate().filter(|(g, _)| filter & (1u64 << g) != 0).map(|(_, n)| *n).sum()
    }
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
    /// 묶음별 수 (`groups` 순서) — 종류별(사진·비디오) 합계는 [`Self::count`]
    pub totals: Vec<u64>,
    /// 직접 든 묶음 표식 → 그런 폴더 수 — 종류별 폴더 수는 [`Self::dirs_with`]
    pub dir_masks: HashMap<u64, u64>,
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
    /// `mask` 묶음의 파일 수
    pub fn count(&self, mask: u64) -> u64 {
        self.totals.iter().enumerate().filter(|(g, _)| mask & (1u64 << g) != 0).map(|(_, n)| *n).sum()
    }

    /// `mask` 묶음 파일이 직접 든 폴더 수
    pub fn dirs_with(&self, mask: u64) -> u64 {
        self.dir_masks.iter().filter(|(m, _)| **m & mask != 0).map(|(_, n)| *n).sum()
    }

    /// 상태줄 오른쪽 글 — `mask` 묶음만 센다 ("사진", "장" / "비디오", "개")
    pub fn text_for(&self, mask: u64, noun: &str, unit: &str) -> String {
        if self.done {
            let tail = if self.updating {
                "변경 반영 중…"
            } else if self.watching {
                "스캔 완료 · 변경 감시 중"
            } else {
                "스캔 완료"
            };
            format!("{noun} {}{unit} · 폴더 {}곳  ·  {tail}", group(self.count(mask)), group(self.dirs_with(mask)))
        } else if self.from_cache {
            format!("저장된 결과 확인 중…  폴더 {}개", group(self.dirs))
        } else {
            format!("{noun} 찾는 중…  폴더 {}개 · {noun} {}{unit}", group(self.dirs), group(self.count(mask)))
        }
    }

    /// 상태줄 오른쪽 글 (모든 묶음)
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

//#region 상태 합계 — 폴더 하나의 묶음별 수를 더하고 뺀다
fn mask_of(counts: &[u32]) -> u64 {
    counts.iter().enumerate().filter(|(_, n)| **n > 0).fold(0, |m, (g, _)| m | (1u64 << g))
}

fn add_counts(st: &mut ScanStatus, counts: &[u32]) {
    if st.totals.len() < counts.len() {
        st.totals.resize(counts.len(), 0);
    }
    let n: u64 = counts.iter().map(|&c| u64::from(c)).sum();
    for (t, &c) in st.totals.iter_mut().zip(counts) {
        *t += u64::from(c);
    }
    st.photos += n;
    st.photo_dirs += u64::from(n > 0);
    let m = mask_of(counts);
    if m != 0 {
        *st.dir_masks.entry(m).or_default() += 1;
    }
}

fn sub_counts(st: &mut ScanStatus, counts: &[u32]) {
    let n: u64 = counts.iter().map(|&c| u64::from(c)).sum();
    for (t, &c) in st.totals.iter_mut().zip(counts) {
        *t = t.saturating_sub(u64::from(c));
    }
    st.photos = st.photos.saturating_sub(n);
    st.photo_dirs = st.photo_dirs.saturating_sub(u64::from(n > 0));
    let m = mask_of(counts);
    if let Some(d) = st.dir_masks.get_mut(&m) {
        *d = d.saturating_sub(1);
        if *d == 0 {
            st.dir_masks.remove(&m);
        }
    }
}
//#endregion

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
    /// 형식 묶음 — 묶음마다 확장자들(소문자). 순서 = 비트 번호 (64개 미만)
    pub groups: Vec<Vec<&'static str>>,
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
    /// 먼저 훑을 폴더 (정규 경로, **뒤집어 둠** — `pop()` 이 앱이 준 목록의 맨 앞). 첫 훑기 동안만 쓴다.
    priority: Mutex<Vec<PathBuf>>,
    /// 시험용: 판정을 공개한 순서
    #[cfg(test)]
    order: Mutex<Vec<PathBuf>>,
}

pub struct Scanner {
    shared: Arc<Shared>,
    /// (트리에 보이는 경로, 정규 경로) — 예: /Volumes/Macintosh HD → /
    aliases: Vec<(PathBuf, PathBuf)>,
}

impl Scanner {
    /// 스캔 스레드를 띄운다. `wake` 는 진행이 있을 때 (최대 초당 몇 번) 불린다.
    pub fn start(cfg: ScanConfig, wake: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new(Shared { verdicts: RwLock::new(HashMap::new()), status: Mutex::new(ScanStatus::default()), stop: AtomicBool::new(false), changed: Mutex::new(Vec::new()),
            priority: Mutex::new(Vec::new()),
            #[cfg(test)]
            order: Mutex::new(Vec::new()),
        });
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
            // 서명 = 묶음 구성 그대로 (묶음·확장자가 바뀌면 저장 결과를 버린다)
            let sig = cfg.groups.iter().map(|g| g.join(",")).collect::<Vec<_>>().join(";");
            let ext_group: HashMap<&'static str, u8> = cfg.groups.iter().enumerate().flat_map(|(i, g)| g.iter().map(move |e| (*e, i as u8))).collect();
            let prev = cfg.cache_file.as_deref().map(|f| persist::load(f, &sig)).unwrap_or_default();
            if !prev.is_empty() {
                let mut v = s.verdicts.write().unwrap_or_else(|e| e.into_inner());
                for (d, c) in &prev {
                    let kids = c.subdirs.iter().chain(&c.links).cloned().collect();
                    v.insert(d.clone(), DirInfo { found: c.found, direct: c.counts.clone(), kids });
                }
                drop(v);
                s.status.lock().unwrap_or_else(|e| e.into_inner()).from_cache = true;
                log::info!("스캔: 저장된 결과 {}개 폴더 불러옴", prev.len());
                (wake)();
            }
            //#endregion
            let t0 = Instant::now();
            let unwatched = roots.iter().filter(|r| !watching || r.as_path() == Path::new("/") || !is_local_volume(r)).cloned().collect();
            let mut w = Walker { shared: s.clone(), roots: roots.clone(), ext_group, groups: cfg.groups.len(), gate: cfg.gate, ignore, wake: Box::new(wake), last_wake: Instant::now(), count: 0, counts: HashMap::new(), unwatched, mtime_trusted: roots.iter().filter(|r| dir_mtime_reliable(r)).cloned().collect(), prev, cur: HashMap::new(), cache_file: cfg.cache_file, sig, dirty: false, last_save: Instant::now(), listed: 0, stack: Vec::new(), tried: HashSet::new(), in_priority: false, priority_on: true };
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
            // 첫 훑기가 끝나면 모든 폴더를 안다 — 우선 큐는 더 쓰지 않는다
            w.priority_on = false;
            w.tried = HashSet::new();
            s.priority.lock().unwrap_or_else(|e| e.into_inner()).clear();
            log::info!("스캔 완료: 폴더 {} (다시 나열 {}) · 파일 {} · {:.1}s · 감시 {}", st.dirs, w.listed, st.photos, t0.elapsed().as_secs_f64(), if watching { "켬" } else { "끔" });
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

    /// 트리 경로의 판정 — 모든 형식 (매 프레임 불려도 되게 가볍게 — 파일 시스템을 건드리지 않는다)
    pub fn verdict(&self, p: &Path) -> Verdict {
        self.verdict_for(p, ALL)
    }

    /// `filter` 형식 묶음 기준 판정
    pub fn verdict_for(&self, p: &Path, filter: u64) -> Verdict {
        let key = self.key(p);
        self.shared.verdicts.read().unwrap_or_else(|e| e.into_inner()).get(key.as_ref()).map_or(Verdict::Unknown, |i| i.found.verdict(filter))
    }

    /// 트리 옆 숫자: (`filter` 묶음의 직접 든 사진 수, 그 필터로 보일 하위 폴더 수). 아직 안 훑었으면 None.
    /// 하위 수는 트리 숨김과 같은 판정(`verdict_for`)으로 센다 — 펼침 삼각형이 숨김과 어긋나지 않게.
    pub fn counts_for(&self, p: &Path, filter: u64) -> Option<(u32, u32)> {
        let key = self.key(p);
        let map = self.shared.verdicts.read().unwrap_or_else(|e| e.into_inner());
        let info = map.get(key.as_ref())?;
        let folders = info.kids.iter().filter(|k| map.get(k.as_path()).map_or(Found::UNKNOWN, |i| i.found).verdict(filter) != Verdict::Empty).count();
        Some((info.photos(filter), folders as u32))
    }

    /// 트리 경로의 스캔 결과 (아직 안 훑었으면 None)
    pub fn info(&self, p: &Path) -> Option<DirInfo> {
        let key = self.key(p);
        self.shared.verdicts.read().unwrap_or_else(|e| e.into_inner()).get(key.as_ref()).cloned()
    }

    /// 트리 경로 → 정규 경로 (별칭 루트)
    fn key<'a>(&self, p: &'a Path) -> std::borrow::Cow<'a, Path> {
        match self.aliases.iter().find(|(a, _)| p.starts_with(a)) {
            Some((a, c)) => c.join(p.strip_prefix(a).unwrap_or(Path::new(""))).into(),
            None => p.into(),
        }
    }

    pub fn status(&self) -> ScanStatus {
        self.shared.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 먼저 훑을 폴더들 (트리 경로 — 앞쪽이 먼저). 큐를 통째로 바꾼다. 첫 훑기 도중에만 효과가 있다:
    /// 스캐너는 지금 훑는 폴더의 다음 하위로 넘어가기 전에 큐에서 하나씩 꺼내 그 폴더(하위 전체)를 먼저 훑고 판정을 공개한다.
    /// 매 프레임 불려도 되게 가볍다 (파일 시스템을 건드리지 않음, 잠금 짧게). 받을지 말지는 스캔 스레드가 정한다.
    pub fn prioritize(&self, paths: Vec<PathBuf>) {
        let q: Vec<PathBuf> = paths.iter().rev().map(|p| self.key(p).into_owned()).collect();
        *self.shared.priority.lock().unwrap_or_else(|e| e.into_inner()) = q;
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
    /// 확장자 → 형식 묶음 번호
    ext_group: HashMap<&'static str, u8>,
    groups: usize,
    gate: Option<Arc<Gate>>,
    ignore: Vec<PathBuf>,
    wake: Box<dyn Fn() + Send>,
    last_wake: Instant,
    count: u64,
    /// 폴더 → 직접 든 사진 수 (상태줄 합계를 변경 때 바로잡기 위해)
    counts: HashMap<PathBuf, Vec<u32>>,
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
    /// 지금 훑는 중인 폴더들 (나열 성공 ~ 공개 직전) — 우선 스캔이 조상·자신을 다시 훑지 않게
    stack: Vec<PathBuf>,
    /// 우선 큐에서 이미 꺼내 본 경로 (받았든 버렸든 다시 보지 않는다)
    tried: HashSet<PathBuf>,
    /// 우선 스캔 중 (중첩해서 꺼내지 않는다)
    in_priority: bool,
    /// 우선 큐를 볼 것인가 (첫 훑기 동안만)
    priority_on: bool,
}

/// 한 폴더 나열 결과
#[derive(Clone)]
struct Listing {
    /// 묶음별 직접 든 사진 수
    counts: Vec<u32>,
    subdirs: Vec<PathBuf>,
    /// 판정에 넣지 못한 하위가 있음 (종류를 모름)
    unknown_child: bool,
    /// 폴더를 가리키는 링크의 대상 (정규 경로) — 따라 들어가지 않고 대상의 판정을 빌린다
    links: Vec<PathBuf>,
}

impl Listing {
    fn mask(&self) -> u64 {
        self.counts.iter().enumerate().filter(|(_, n)| **n > 0).fold(0, |m, (g, _)| m | (1u64 << g))
    }
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
            return Some((Listing { counts: c.counts.clone(), subdirs: c.subdirs.clone(), unknown_child: c.unknown_child, links: c.links.clone() }, mtime));
        }
        self.listed += 1;
        self.read_listing(dir).map(|l| (l, mtime))
    }

    fn read_listing(&self, dir: &Path) -> Option<Listing> {
        let rd = std::fs::read_dir(dir).ok()?;
        let mut l = Listing { counts: vec![0; self.groups], subdirs: Vec::new(), unknown_child: false, links: Vec::new() };
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
                    Ok(m) if m.is_file() => self.count_photo(&mut l, &path),
                    _ => {}
                }
            } else if ft.is_dir() {
                // 트리(list_subdirs)와 같은 규칙: 번들·Finder 숨김 폴더는 없는 셈
                if !is_bundle(&path) && !finder_hidden(&path) {
                    l.subdirs.push(path);
                }
            } else if ft.is_file() {
                self.count_photo(&mut l, &path);
            }
        }
        Some(l)
    }
    //#endregion

    //#region 저장
    fn remember(&mut self, dir: &Path, mtime: SystemTime, l: &Listing, found: Found) {
        let c = Cached { mtime, counts: l.counts.clone(), unknown_child: l.unknown_child, subdirs: l.subdirs.clone(), links: l.links.clone(), found };
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
    /// 결과 공개. 돌려주는 값 = 실제로 공개한 결과 (저장도 이것으로 — 다음 실행에 그대로 공개되므로)
    fn set_info(&self, dir: &Path, f: Found, l: &Listing, kids: Vec<PathBuf>) -> Found {
        // 감시하지 않는 곳은 나중에 (어떤 형식이든) 사진이 생겨도 알 수 없으니 늘 '확신 못 함' — 필터를 걸어도 숨김이 굳지 않게
        let f = if self.watched(dir) { f } else { Found { unknown: true, ..f } };
        let info = DirInfo { found: f, direct: l.counts.clone(), kids };
        self.shared.verdicts.write().unwrap_or_else(|e| e.into_inner()).insert(dir.to_path_buf(), info);
        #[cfg(test)]
        self.shared.order.lock().unwrap().push(dir.to_path_buf());
        f
    }

    /// 읽지 못한 폴더 — 숫자 없이 모름으로
    fn set_unreadable(&self, dir: &Path) {
        self.shared.verdicts.write().unwrap_or_else(|e| e.into_inner()).remove(dir);
    }

    fn record_count(&mut self, dir: &Path, counts: &[u32]) {
        let old = self.counts.insert(dir.to_path_buf(), counts.to_vec());
        let mut st = self.shared.status.lock().unwrap_or_else(|e| e.into_inner());
        if old.is_none() {
            st.dirs += 1;
        }
        if let Some(o) = &old {
            sub_counts(&mut st, o);
        }
        add_counts(&mut st, counts);
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
                sub_counts(&mut st, &c);
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
    fn walk(&mut self, dir: &Path, is_root: bool) -> Found {
        if self.stopped() {
            return Found::UNKNOWN;
        }
        self.yield_now();
        let Some((l, mtime)) = self.list(dir) else {
            self.set_unreadable(dir);
            return Found::UNKNOWN;
        };
        self.record_count(dir, &l.counts);
        self.stack.push(dir.to_path_buf());
        //#region 하위 판정 — 다른 루트·시스템 폴더는 들어가지 않고 이미 아는 결과(없으면 모름)를 쓴다
        // 표식은 매번 새로 합친다 (옛 값에 OR 하지 않음 — 마지막 PNG 를 지우면 PNG 비트가 빠져야 한다)
        let mut f = Found { mask: l.mask(), unknown: l.unknown_child };
        let mut kids = Vec::new();
        for t in &l.links {
            f.merge(self.link_found(t));
            if !self.is_system(t) {
                kids.push(t.clone());
            }
        }
        for sub in l.subdirs.clone() {
            self.run_priority();
            let v = if self.is_system(&sub) {
                continue;
            } else if self.skip(&sub, is_root.then_some(dir)) {
                self.known(&sub)
            } else if self.priority_on && self.cur.contains_key(&sub) {
                // 첫 훑기에서 이미 cur 에 있으면 우선 스캔으로 훑은 것 — 다시 내려가지 않는다 (합계·나열 이중 계산 없음).
                // 변경 따라가기(follow)에선 예전처럼 다시 내려간다.
                self.known(&sub)
            } else {
                self.walk(&sub, false)
            };
            f.merge(v);
            kids.push(sub);
        }
        //#endregion
        f.unknown |= self.stopped();
        self.stack.pop();
        let published = self.set_info(dir, f, &l, kids);
        if !self.stopped() {
            self.remember(dir, mtime, &l, published);
        }
        self.maybe_wake();
        f
    }

    //#region 우선 스캔 — 앱이 트리에서 보이는(아직 모르는) 폴더를 먼저 훑는다
    /// 우선 큐에서 받을 만한 경로 하나를 꺼내 그 자리에서 (하위까지) 훑는다. 나중에 본 순회가 거기 오면 `cur` 를 보고 내려가지 않는다.
    fn run_priority(&mut self) {
        if !self.priority_on || self.in_priority || self.stopped() {
            return;
        }
        loop {
            // 잠금은 하나 꺼내는 동안만 — 받을지 판단(파일 시스템 조회)은 잠금 밖에서
            let Some(p) = self.shared.priority.lock().unwrap_or_else(|e| e.into_inner()).pop() else { return };
            if self.accept(&p) {
                log::debug!("스캔: 우선 훑기 {}", p.display());
                self.in_priority = true;
                self.walk(&p, false);
                self.in_priority = false;
                return;
            }
        }
    }

    /// 우선 훑어도 되는 경로인가 — 본 순회가 언젠가 그대로 들를 곳만 받는다 (그래야 합계·저장이 어긋나지 않는다)
    fn accept(&mut self, p: &Path) -> bool {
        if !self.tried.insert(p.to_path_buf()) || self.cur.contains_key(p) {
            return false;
        }
        // 지금 훑는 중인 조상·자신이면 버린다 (그 순회가 곧 공개한다)
        if self.stack.iter().any(|s| s.starts_with(p)) || self.roots.iter().any(|r| r == p) {
            return false;
        }
        let Some(root) = self.root_of(p) else { return false };
        // 조상을 이미 다 훑었는데 자신이 없으면 본 순회가 들르지 않은 곳(나열 뒤 생긴 폴더 등) — 감시가 맡는다
        if p.ancestors().skip(1).take_while(|a| a.starts_with(&root)).any(|a| self.cur.contains_key(a)) {
            return false;
        }
        is_local_volume(&root) && p.canonicalize().is_ok_and(|c| c == p) && p.is_dir() && self.in_scope(&root, p)
    }

    /// `p` 를 품은 가장 가까운 루트
    fn root_of(&self, p: &Path) -> Option<PathBuf> {
        self.roots.iter().filter(|r| p.starts_with(r)).max_by_key(|r| r.components().count()).cloned()
    }

    /// 루트부터 내려가며 본 순회가 건너뛰는 폴더(숨김·번들·Finder 숨김·시스템·~/Library·다른 루트) 아래가 아닌가
    fn in_scope(&self, root: &Path, p: &Path) -> bool {
        let Ok(rel) = p.strip_prefix(root) else { return false };
        let mut cur = root.to_path_buf();
        for c in rel.components() {
            let next = cur.join(c);
            let hidden = c.as_os_str().to_string_lossy().starts_with('.');
            if hidden || is_bundle(&next) || finder_hidden(&next) || self.skip(&next, (cur == root).then_some(cur.as_path())) {
                return false;
            }
            cur = next;
        }
        true
    }
    //#endregion

    /// 한 폴더만 다시 판정 (하위 판정은 아는 것을 쓴다). `walk_new` 면 모르는 하위(새 폴더)는 훑고, 사라진 하위는 잊는다.
    fn shallow(&mut self, dir: &Path, walk_new: bool) -> Found {
        let is_root = self.roots.iter().any(|r| r == dir);
        let Some((l, mtime)) = self.list(dir) else {
            self.set_unreadable(dir);
            return Found::UNKNOWN;
        };
        self.record_count(dir, &l.counts);
        if walk_new {
            let gone: Vec<PathBuf> = self.counts.keys().filter(|k| k.parent() == Some(dir) && !l.subdirs.contains(k)).cloned().collect();
            for g in gone {
                self.forget(&g);
            }
        }
        let mut f = Found { mask: l.mask(), unknown: l.unknown_child };
        let mut kids = Vec::new();
        for t in &l.links {
            f.merge(self.link_found(t));
            if !self.is_system(t) {
                kids.push(t.clone());
            }
        }
        for sub in &l.subdirs {
            if self.is_system(sub) {
                continue;
            }
            let skip = self.skip(sub, is_root.then_some(dir));
            let mut v = self.known(sub);
            if v.verdict(ALL) == Verdict::Unknown && walk_new && !skip {
                v = self.walk(sub, false);
            }
            f.merge(v);
            kids.push(sub.clone());
        }
        let published = self.set_info(dir, f, &l, kids);
        self.remember(dir, mtime, &l, published);
        f
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
        let root = self.root_of(&e.dir)?;
        // 루트부터 내려가며 건너뛰는 폴더(시스템·~/Library·번들·점 폴더) 아래면 버린다
        self.in_scope(&root, &e.dir).then_some(e)
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

    fn known(&self, p: &Path) -> Found {
        self.shared.verdicts.read().unwrap_or_else(|e| e.into_inner()).get(p).map_or(Found::UNKNOWN, |i| i.found)
    }

    /// 들어가지 않을 폴더: 다른 루트(따로 훑는다), 시스템 루트(/) 아래의 시스템 폴더, 홈의 Library
    fn skip(&self, p: &Path, root_parent: Option<&Path>) -> bool {
        if self.roots.iter().any(|r| r == p) {
            return true;
        }
        // /Volumes 아래는 볼륨마다 따로 루트다
        root_parent == Some(Path::new("/")) && p == Path::new("/Volumes") || self.is_system(p)
    }

    /// 폴더 링크의 결과: 시스템 폴더면 없음, 이미 훑은 곳이면 그 결과, 아니면 모름
    fn link_found(&self, target: &Path) -> Found {
        if self.is_system(target) { Found::default() } else { self.known(target) }
    }

    /// 시스템 폴더인가 — 단, 루트 자체가 시스템 폴더 안이면(예: /private/var 의 임시 폴더) 그 루트 아래는 예외
    fn is_system(&self, p: &Path) -> bool {
        is_system_dir(p) && !self.roots.iter().any(|r| p.starts_with(r) && is_system_dir(r))
    }

    /// FSEvents 로 감시되는 루트 아래인가 (가장 가까운 루트 기준)
    fn watched(&self, p: &Path) -> bool {
        self.roots.iter().filter(|r| p.starts_with(r)).max_by_key(|r| r.components().count()).is_some_and(|r| !self.unwatched.contains(r))
    }

    /// 사진이면 그 묶음 칸을 하나 늘린다
    fn count_photo(&self, l: &mut Listing, p: &Path) {
        let g = p.extension().and_then(|e| e.to_str()).and_then(|e| self.ext_group.get(e.to_ascii_lowercase().as_str()));
        if let Some(&g) = g {
            l.counts[g as usize] += 1;
        }
    }
}

/// 사진 보관 장소가 아닌 시스템 폴더 — 정책상 "사진 없음" (들어가지 않는다).
/// 설치 디스크의 `Applications → /Applications` 링크 같은 것 때문에 빈 볼륨이 숨겨지지 않던 문제를 막는다.
fn is_system_dir(p: &Path) -> bool {
    const SYS: [&str; 13] = ["/System", "/Library", "/Applications", "/private", "/usr", "/bin", "/sbin", "/opt", "/cores", "/dev", "/etc", "/tmp", "/var"];
    SYS.iter().any(|s| p.starts_with(s)) || std::env::var_os("HOME").is_some_and(|h| p.starts_with(Path::new(&h).join("Library")))
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
        let s = Scanner::start(ScanConfig { roots: vec![d.to_path_buf()], groups: vec![vec!["jpg"], vec!["png"]], gate: None, ignore: Vec::new(), cache_file }, move || {
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
        assert_eq!(s.counts_for(&c.join("photo"), ALL), Some((1, 0)));
        assert_eq!(s.counts_for(&c, ALL).map(|i| i.1), Some(3));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 형식 필터: 폴더 표시·숫자가 필터 기준, 필터를 바꿔도 다시 훑지 않는다
    #[test]
    fn filter_masks_per_format() {
        let d = tree("f");
        let c = d.canonicalize().unwrap();
        std::fs::create_dir_all(c.join("mixdir")).unwrap();
        std::fs::write(c.join("mixdir/m.jpg"), b"x").unwrap();
        std::fs::write(c.join("mixdir/n.png"), b"x").unwrap();
        std::fs::write(c.join("mixdir/o.PNG"), b"x").unwrap();
        let s = run(&d);
        let (jpg, png) = (1u64, 2u64);
        // photo: jpg 만 / deep: png 만
        assert_eq!(s.verdict_for(&c.join("photo"), jpg), Verdict::HasPhotos);
        assert_eq!(s.verdict_for(&c.join("photo"), png), Verdict::Empty);
        assert_eq!(s.verdict_for(&c.join("deep"), png), Verdict::HasPhotos);
        assert_eq!(s.verdict_for(&c.join("deep"), jpg), Verdict::Empty);
        assert_eq!(s.counts_for(&c.join("mixdir"), png), Some((2, 0)));
        assert_eq!(s.counts_for(&c.join("mixdir"), jpg), Some((1, 0)));
        // 루트의 보일 하위: png 필터면 deep·mixdir + 링크(대상 판정)
        let st = s.status();
        assert_eq!((st.count(jpg), st.count(png)), (2, 3), "종류별 합계");
        assert_eq!(st.dirs_with(png), 2);
        let (_, f_png) = s.counts_for(&c, png).unwrap();
        let (_, f_all) = s.counts_for(&c, ALL).unwrap();
        assert!(f_png < f_all, "{f_png} < {f_all}");
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
        // 형식 비트도 다시 합쳐진다: png 만 지우면 그 폴더·조상은 png 필터에서 빠진다
        assert_eq!(s.verdict_for(&c.join("deep"), 2), Verdict::HasPhotos);
        std::fs::remove_file(c.join("deep/x/y/b.png")).unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        while s.verdict_for(&c.join("deep"), 2) != Verdict::Empty && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(s.verdict_for(&c.join("deep"), 2), Verdict::Empty);
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
        assert_eq!(second.verdict_for(&c.join("deep"), 1), Verdict::Empty, "저장 결과에도 형식 표식이 남는다");
        assert_eq!(second.verdict_for(&c.join("deep"), 2), Verdict::HasPhotos);
        assert_eq!(st.photos, 3);
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 큰 시험 트리: 맨 위 폴더 t0..t9 마다 사진 없는 하위 60개 + 끝쪽 폴더 z(사진 없는 s1/t·s2·s3, 숨김 .hid),
    /// t3/k 에 jpg 1장, t7 에 png 1장. 돌려주는 값 = (루트, 본 순회가 **마지막에** 들를 맨 위 폴더 이름).
    /// 나열 순서는 파일 시스템이 정하므로, 실제 read_dir 순서의 마지막 폴더 아래에 z 를 둔다.
    fn big_tree(tag: &str) -> (PathBuf, String) {
        let d = std::env::temp_dir().join(format!("iv-scan-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for i in 0..10 {
            for j in 0..60 {
                std::fs::create_dir_all(d.join(format!("t{i}/e{j:02}"))).unwrap();
            }
        }
        let last = std::fs::read_dir(&d).unwrap().flatten().last().unwrap().file_name().to_string_lossy().into_owned();
        for s in ["s1/t", "s2", "s3", ".hid"] {
            std::fs::create_dir_all(d.join(&last).join("z").join(s)).unwrap();
        }
        std::fs::create_dir_all(d.join("t3/k")).unwrap();
        std::fs::write(d.join("t3/k/p.jpg"), b"x").unwrap();
        std::fs::write(d.join("t7/q.png"), b"x").unwrap();
        std::fs::write(d.join(&last).join("z/s2/notes.txt"), b"x").unwrap();
        (d, last)
    }

    /// 트리 전체의 (경로 → 판정·직접 수·하위) — 두 실행 비교용
    fn snapshot(s: &Scanner, c: &Path) -> Vec<(PathBuf, Found, Vec<u32>, usize)> {
        let mut out = Vec::new();
        let mut todo = vec![c.to_path_buf()];
        while let Some(p) = todo.pop() {
            let Some(i) = s.info(&p) else {
                out.push((p, Found::UNKNOWN, Vec::new(), usize::MAX));
                continue;
            };
            todo.extend(i.kids.iter().cloned());
            out.push((p, i.found, i.direct.clone(), i.kids.len()));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// 게이트로 스캐너를 첫 폴더 앞에 세워 두고 큐를 채운 뒤 풀어 준다 (첫 하위 전에 큐를 확실히 보게)
    fn start_prioritized(d: &Path, paths: Vec<PathBuf>, cache_file: Option<PathBuf>) -> Scanner {
        let gate = Arc::new(Gate::new());
        gate.set_busy(true);
        let s = Scanner::start(ScanConfig { roots: vec![d.to_path_buf()], groups: vec![vec!["jpg"], vec!["png"]], gate: Some(gate.clone()), ignore: Vec::new(), cache_file }, || {});
        s.prioritize(paths);
        gate.set_busy(false);
        let until = Instant::now() + Duration::from_secs(10);
        while !s.status().done && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(s.status().done);
        s
    }

    /// 우선 스캔: 결과(판정·직접 수·하위·합계·나열 수)가 우선 스캔이 없을 때와 똑같다
    #[test]
    fn priority_scan_identical() {
        let (d, last) = big_tree("p");
        let c = d.canonicalize().unwrap();
        let (z, zc) = (d.join(&last).join("z"), c.join(&last).join("z"));
        let base = run(&d);
        let want = snapshot(&base, &c);
        let bst = base.status();
        drop(base);
        // 받으면 안 되는 것도 섞는다: 루트 자신, 숨김 폴더, 범위 밖, 없는 경로
        let s = start_prioritized(&d, vec![z.join("s1"), z.join("s2"), d.clone(), z.join(".hid"), std::env::temp_dir(), d.join("nope"), z.clone()], None);
        let st = s.status();
        assert_eq!(s.verdict(&zc.join("s1")), Verdict::Empty);
        assert_eq!(s.verdict(&zc), Verdict::Empty);
        assert_eq!(s.verdict(&zc.join(".hid")), Verdict::Unknown, "숨김 폴더는 받지 않는다");
        assert_eq!(snapshot(&s, &c), want);
        assert_eq!((st.dirs, st.photos, st.photo_dirs, st.totals.clone(), st.dir_masks.clone()), (bst.dirs, bst.photos, bst.photo_dirs, bst.totals.clone(), bst.dir_masks.clone()));
        assert_eq!(st.relisted, st.dirs, "폴더마다 한 번만 나열");
        assert_eq!(bst.relisted, bst.dirs);
        // 공개는 폴더마다 한 번 (우선 훑은 곳을 본 순회가 다시 공개하지 않음)
        let order = s.shared.order.lock().unwrap().clone();
        assert_eq!(order.len() as u64, st.dirs);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 우선 스캔: 큐에 넣은 폴더(본 순회가 마지막에 들를 곳)가 다른 맨 위 폴더들보다 먼저 공개된다
    #[test]
    fn priority_scan_publishes_first() {
        let (d, last) = big_tree("po");
        let c = d.canonicalize().unwrap();
        let s = start_prioritized(&d, vec![d.join(&last).join("z/s1"), d.join(&last).join("z")], None);
        let order = s.shared.order.lock().unwrap().clone();
        let pos = |p: &Path| order.iter().position(|o| o == p).unwrap_or_else(|| panic!("공개 안 됨: {}", p.display()));
        let z = pos(&c.join(&last).join("z"));
        assert!(pos(&c.join(&last).join("z/s1")) < z);
        // z 밑 폴더 5개(z·s1·s1/t·s2·s3) 공개가 맨 처음
        assert!(z < 5, "z 공개 순서 {z}");
        for i in 0..10 {
            let t = format!("t{i}");
            if t != last {
                assert!(z < pos(&c.join(&t)), "{t} 보다 먼저");
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 저장 결과 + 우선 스캔: 저장 파일 내용이 우선 스캔 없을 때와 같다
    #[test]
    fn priority_scan_saves_same_cache() {
        let (d, last) = big_tree("ps");
        let f1 = std::env::temp_dir().join(format!("iv-scan-pcache1-{}.tsv", std::process::id()));
        let f2 = std::env::temp_dir().join(format!("iv-scan-pcache2-{}.tsv", std::process::id()));
        let _ = (std::fs::remove_file(&f1), std::fs::remove_file(&f2));
        drop(run_cached(&d, Some(f1.clone())));
        let s = start_prioritized(&d, vec![d.join(&last).join("z/s1/t"), d.join("t5"), d.join(&last).join("z")], Some(f2.clone()));
        drop(s);
        let sig = "jpg;png";
        let (a, b) = (persist::load(&f1, sig), persist::load(&f2, sig));
        assert!(!a.is_empty());
        let mut ka: Vec<_> = a.keys().collect();
        let mut kb: Vec<_> = b.keys().collect();
        ka.sort();
        kb.sort();
        assert_eq!(ka, kb);
        for (k, v) in &a {
            let w = &b[k];
            assert_eq!((v.found, &v.counts, &v.subdirs, &v.links, v.unknown_child), (w.found, &w.counts, &w.subdirs, &w.links, w.unknown_child), "{}", k.display());
        }
        let _ = (std::fs::remove_file(&f1), std::fs::remove_file(&f2));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn status_text() {
        assert_eq!(group(1234567), "1,234,567");
        assert_eq!(group(12), "12");
    }
}
