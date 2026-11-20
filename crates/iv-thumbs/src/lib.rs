//! 썸네일 서비스.
//!
//! 앱은 "지금 보이는 항목"을 [`ThumbService::request`] 로 알리고(보이는 순서 = 우선순위),
//! 결과는 내부 큐에 쌓인 뒤 `wake` 를 한 번 불러 메인 스레드가 [`ThumbService::drain`] 한다 (Loader 와 같은 규약).
//! 생성: 디스크 캐시 → 내장 미리보기(RAW·HEIF) → 축소 디코드. 결과는 **방향이 적용된** RGBA 라 UI 가 바로 텍스처로 쓴다.
//! 전체 보기 파이프라인과는 [`iv_core::Gate`] 로 중재한다: 현재 장 디코드가 바쁘면 새 작업을 시작하지 않는다.

mod disk;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use iv_core::{CancelToken, DecoderRegistry, Frame, Gate, ImageSource, SizeHint};
use parking_lot::{Condvar, Mutex};

pub use disk::{DiskCache, default_cache_dir};

#[derive(Clone, Debug)]
pub struct ThumbConfig {
    /// 썸네일 긴 변 (물리 픽셀)
    pub side: u32,
    /// 메모리 캐시 바이트 상한
    pub mem_bytes: usize,
    /// 디스크 캐시 폴더 (None = 끔)
    pub disk_dir: Option<PathBuf>,
    /// 디스크 캐시 상한 — 시작 시 넘치면 오래된 것부터 정리
    pub disk_bytes: u64,
    pub workers: usize,
    /// 높은 우선순위 작업(현재 장 디코드)이 바쁘면 기다리는 관문
    pub gate: Option<Arc<Gate>>,
}

impl Default for ThumbConfig {
    fn default() -> Self {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        Self { side: 320, mem_bytes: 192 << 20, disk_dir: default_cache_dir(), disk_bytes: 1 << 30, workers: (cores / 2).clamp(1, 3), gate: None }
    }
}

/// 요청 항목. `mtime`·`len` 은 디스크 캐시 무효화용 지문.
#[derive(Clone, Debug)]
pub struct ThumbRequest {
    pub key: u64,
    pub src: ImageSource,
    pub mtime: SystemTime,
    pub len: u64,
}

#[derive(Clone, Debug)]
pub struct Thumb {
    pub key: u64,
    /// 방향 적용된 RGBA8 (stride = width*4)
    pub image: Arc<Frame>,
    /// 원본 크기 (표시 방향 기준) — "4032×3024" 표시용
    pub full_size: [u32; 2],
}

#[derive(Clone, Debug)]
pub enum ThumbEvent {
    Ready(Thumb),
    Failed { key: u64, error: String },
}

struct State {
    /// 대기 (앞이 먼저)
    queue: std::collections::VecDeque<ThumbRequest>,
    running: HashSet<u64>,
    failed: HashMap<u64, String>,
    mem: HashMap<u64, (Thumb, u64)>,
    mem_bytes: usize,
    tick: u64,
    results: Vec<ThumbEvent>,
    paused: bool,
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
    registry: Arc<DecoderRegistry>,
    config: ThumbConfig,
    disk: Option<DiskCache>,
    pending: AtomicBool,
    wake: Box<dyn Fn() + Send + Sync>,
}

pub struct ThumbService {
    shared: Arc<Shared>,
}

impl ThumbService {
    pub fn new(registry: Arc<DecoderRegistry>, config: ThumbConfig, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let disk = config.disk_dir.as_ref().and_then(|d| DiskCache::open(d.clone(), config.side).map_err(|e| log::warn!("썸네일 디스크 캐시 끔: {e}")).ok());
        if let Some(d) = &disk {
            log::info!("썸네일 디스크 캐시: {}", d.dir().display());
            d.prune_in_background(config.disk_bytes);
        }
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                queue: Default::default(),
                running: HashSet::new(),
                failed: HashMap::new(),
                mem: HashMap::new(),
                mem_bytes: 0,
                tick: 0,
                results: Vec::new(),
                paused: false,
                shutdown: false,
            }),
            cv: Condvar::new(),
            registry,
            config: config.clone(),
            disk,
            pending: AtomicBool::new(false),
            wake: Box::new(wake),
        });
        for i in 0..config.workers.max(1) {
            let s = shared.clone();
            std::thread::Builder::new().name(format!("iv-thumb-{i}")).spawn(move || worker(s)).expect("썸네일 워커 생성 실패");
        }
        Self { shared }
    }

    /// 보이는 항목을 알린다 (앞이 먼저). 목록에 없는 대기 작업은 버린다. 진행 중인 것은 끝까지 간다.
    /// 이미 메모리에 있거나 실패한 항목은 건너뛴다 — 메모리 것은 [`Self::get`] 으로 꺼낸다.
    pub fn request(&self, items: &[ThumbRequest]) {
        {
            let mut guard = self.shared.state.lock();
            let st = &mut *guard;
            st.queue.clear();
            for it in items {
                if !st.mem.contains_key(&it.key) && !st.running.contains(&it.key) && !st.failed.contains_key(&it.key) {
                    st.queue.push_back(it.clone());
                }
            }
        }
        self.shared.cv.notify_all();
    }

    /// 메모리 캐시에서 꺼낸다 (최근 사용으로 갱신)
    pub fn get(&self, key: u64) -> Option<Thumb> {
        let mut st = self.shared.state.lock();
        st.tick += 1;
        let t = st.tick;
        st.mem.get_mut(&key).map(|(th, used)| {
            *used = t;
            th.clone()
        })
    }

    pub fn failed(&self, key: u64) -> Option<String> {
        self.shared.state.lock().failed.get(&key).cloned()
    }

    pub fn drain(&self) -> Vec<ThumbEvent> {
        self.shared.pending.store(false, Ordering::SeqCst);
        std::mem::take(&mut self.shared.state.lock().results)
    }

    /// 전체 보기 중에는 멈춘다 (넘기기 속도 보호). 진행 중인 작업은 끝까지 간다.
    pub fn set_paused(&self, paused: bool) {
        self.shared.state.lock().paused = paused;
        if !paused {
            self.shared.cv.notify_all();
        }
    }

    /// 실패 기록을 지운다 (새로고침 — 파일이 고쳐졌을 수 있음)
    pub fn forget_failures(&self) {
        self.shared.state.lock().failed.clear();
    }

    /// (메모리 바이트, 장 수, 대기, 진행)
    pub fn stats(&self) -> (usize, usize, usize, usize) {
        let st = self.shared.state.lock();
        (st.mem_bytes, st.mem.len(), st.queue.len(), st.running.len())
    }
}

impl Drop for ThumbService {
    fn drop(&mut self) {
        self.shared.state.lock().shutdown = true;
        self.shared.cv.notify_all();
    }
}

fn worker(shared: Arc<Shared>) {
    loop {
        // 1) 작업 고르기
        let req = {
            let mut st = shared.state.lock();
            loop {
                if st.shutdown {
                    return;
                }
                if !st.paused {
                    if let Some(r) = st.queue.pop_front() {
                        st.running.insert(r.key);
                        break r;
                    }
                }
                shared.cv.wait(&mut st);
            }
        };
        // 2) 현재 장 디코드가 바쁘면 양보 (상한이 있어 굶지 않음). 디스크 캐시는 가벼우니 먼저 본다.
        let t = Instant::now();
        let from_disk = shared.disk.as_ref().and_then(|d| d.load(&req));
        let result = match from_disk {
            Some(th) => Ok((th, true)),
            None => {
                if let Some(g) = &shared.config.gate {
                    // 관문은 원자적 허가가 아니다: 확인 직후 현재 장이 시작될 수 있어 짧게 한 번 더 본다.
                    // 남는 겹침은 워커당 썸네일 1장(수십 ms) — 높은 쪽(현재 장)은 절대 기다리지 않는 쪽을 택했다.
                    g.wait_idle(Duration::from_millis(1500));
                    if g.is_busy() {
                        g.wait_idle(Duration::from_millis(500));
                    }
                }
                // 이미 꺼낸 작업은 기다린 뒤에도 생성한다 (작아서 짧다). running 표시가 중복 요청을 막는다.
                generate_one(&shared.registry, &req.src, shared.config.side).map(|th| (Thumb { key: req.key, ..th }, false))
            }
        };
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        // 3) 결과 반영
        let ev = match result {
            Ok((th, disk_hit)) => {
                log::trace!("썸네일 {} {}x{} {ms:.1}ms{}", req.src.display_name(), th.image.width, th.image.height, if disk_hit { " (디스크)" } else { "" });
                if !disk_hit {
                    if let Some(d) = &shared.disk {
                        d.store(&req, &th);
                    }
                }
                let mut st = shared.state.lock();
                insert_mem(&mut st, th.clone(), shared.config.mem_bytes);
                ThumbEvent::Ready(th)
            }
            Err(e) => {
                log::debug!("썸네일 실패 {}: {e}", req.src.display_name());
                shared.state.lock().failed.insert(req.key, e.clone());
                ThumbEvent::Failed { key: req.key, error: e }
            }
        };
        let mut st = shared.state.lock();
        st.running.remove(&req.key);
        st.results.push(ev);
        drop(st);
        if !shared.pending.swap(true, Ordering::SeqCst) {
            (shared.wake)();
        }
    }
}

fn insert_mem(st: &mut State, th: Thumb, cap: usize) {
    st.tick += 1;
    let bytes = th.image.pixels.len();
    if let Some((old, _)) = st.mem.insert(th.key, (th, st.tick)) {
        st.mem_bytes -= old.image.pixels.len();
    }
    st.mem_bytes += bytes;
    while st.mem_bytes > cap && st.mem.len() > 1 {
        let Some((&k, _)) = st.mem.iter().min_by_key(|(_, (_, used))| *used) else { break };
        if let Some((old, _)) = st.mem.remove(&k) {
            st.mem_bytes -= old.image.pixels.len();
        }
    }
}

/// 한 장 생성: 내장 미리보기(RAW·HEIF) → 축소 디코드 → 긴 변 side 로 맞춤 → 방향 적용. key 는 0.
pub fn generate_one(registry: &DecoderRegistry, src: &ImageSource, side: u32) -> Result<Thumb, String> {
    let hint = SizeHint::max_side(side);
    let cancel = CancelToken::new();
    let frame = match registry.preview(src, hint, &cancel) {
        Ok(Some(f)) => f,
        _ => registry.decode(src, hint, &cancel).map_err(|e| e.to_string())?,
    };
    let frame = if frame.max_side() > side { iv_imageops::downscale_rgba8(&frame, side) } else { frame };
    let full_size = frame.display_full_size();
    Ok(Thumb { key: 0, image: Arc::new(iv_imageops::apply_orientation(&frame)), full_size })
}
