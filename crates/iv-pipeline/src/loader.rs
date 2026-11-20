use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use iv_core::{CancelToken, DecodeError, DecoderRegistry, Frame, Gate, ImageSource, SizeHint};
use parking_lot::{Condvar, Mutex};

use crate::{EntryKey, FrameCache};

/// 결과 프레임이 어떤 작업에서 왔는가
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Quality {
    Preview,
    Decoded,
}

#[derive(Clone, Debug)]
pub struct Loaded {
    pub key: EntryKey,
    pub generation: u64,
    pub frame: Arc<Frame>,
    pub quality: Quality,
    pub decode_ms: f64,
}

#[derive(Clone, Debug)]
pub enum LoadEvent {
    Loaded(Loaded),
    Failed { key: EntryKey, generation: u64, error: String, unsupported: bool },
}

/// 현재 장에 원하는 해상도
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// 긴 변이 이 값 이상이면 충분 (화면 크기)
    Side(u32),
}

impl Want {
    fn hint(self) -> SizeHint {
        match self {
            Want::Side(n) => SizeHint::max_side(n),
        }
    }
    fn side(self) -> u32 {
        match self {
            Want::Side(n) => n,
        }
    }
}

/// 앱이 알려주는 "지금 보고 있는 곳"
#[derive(Clone, Debug)]
pub struct Window {
    pub generation: u64,
    pub current: Option<(EntryKey, ImageSource)>,
    /// 현재 장에 필요한 해상도 (확대하면 커진다)
    pub current_want: Want,
    /// 앞쪽 이웃 (가까운 순)
    pub ahead: Vec<(EntryKey, ImageSource)>,
    /// 뒤쪽 이웃 (가까운 순)
    pub behind: Vec<(EntryKey, ImageSource)>,
    /// 이웃 미리 읽기 해상도 (보통 화면 긴 변)
    pub neighbor_side: u32,
}

#[derive(Clone, Debug)]
pub struct LoaderConfig {
    /// 캐시 바이트 상한
    pub cache_bytes: usize,
    /// 미리 읽기 진행 중 바이트 예산 (현재 장은 예외)
    pub inflight_budget: usize,
    /// 미리 읽기 워커 수
    pub prefetch_workers: usize,
    /// 현재 장 디코드가 대기·진행 중이면 busy — 다른 작업 풀(썸네일)이 양보하는 관문
    pub gate: Arc<Gate>,
}

impl Default for LoaderConfig {
    fn default() -> Self {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        Self { cache_bytes: 1 << 30, inflight_budget: 768 << 20, prefetch_workers: (cores.saturating_sub(1)).clamp(1, 4), gate: Arc::new(Gate::new()) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
    Preview,
    Decode(SizeHint),
}

#[derive(Clone, Debug)]
struct Job {
    key: EntryKey,
    src: ImageSource,
    kind: Kind,
    /// 0 = 현재 장, 클수록 나중
    prio: u32,
    generation: u64,
    est_bytes: usize,
    /// 미리보기 상한 (화면 크기)
    preview_hint: SizeHint,
}

struct Running {
    key: EntryKey,
    prio: u32,
    kind: Kind,
    cancel: CancelToken,
    est_bytes: usize,
    generation: u64,
}

struct State {
    queue: Vec<Job>,
    running: Vec<Running>,
    inflight_bytes: usize,
    cache: FrameCache,
    pinned: Vec<EntryKey>,
    results: Vec<LoadEvent>,
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
    registry: Arc<DecoderRegistry>,
    config: LoaderConfig,
    /// lost-wakeup 방지: 워커는 push 후 swap(true) 가 false 였을 때만 wake,
    /// 메인은 store(false) 후 drain (순서 고정)
    pending: AtomicBool,
    wake: Box<dyn Fn() + Send + Sync>,
}

pub struct Loader {
    shared: Arc<Shared>,
}

fn est_bytes(kind: Kind) -> usize {
    match kind {
        Kind::Preview => 4 << 20,
        Kind::Decode(h) => {
            let s = h.max_side.unwrap_or(8000) as usize;
            s * s * 4
        }
    }
}

impl Loader {
    pub fn new(registry: Arc<DecoderRegistry>, config: LoaderConfig, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                queue: Vec::new(),
                running: Vec::new(),
                inflight_bytes: 0,
                cache: FrameCache::new(config.cache_bytes),
                pinned: Vec::new(),
                results: Vec::new(),
                shutdown: false,
            }),
            cv: Condvar::new(),
            registry,
            config: config.clone(),
            pending: AtomicBool::new(false),
            wake: Box::new(wake),
        });
        // 현재 장 전용 레인 1개 + 미리 읽기 워커
        spawn_worker(&shared, "iv-current", true);
        for i in 0..config.prefetch_workers {
            spawn_worker(&shared, &format!("iv-prefetch-{i}"), false);
        }
        Self { shared }
    }

    pub fn registry(&self) -> &Arc<DecoderRegistry> {
        &self.shared.registry
    }

    /// 보고 있는 곳을 알린다. 창 밖 작업은 취소하고, 창 안 작업은 우선순위만 바꾼다.
    pub fn set_window(&self, w: &Window) {
        let mut desired: Vec<Job> = Vec::new();
        let mut st = self.shared.state.lock();
        st.pinned = w.current.iter().map(|c| c.0).collect();

        if let Some((key, src)) = &w.current {
            let want = w.current_want;
            if !st.cache.has_satisfying(*key, Some(want.side())) {
                // 아무 프레임도 없을 때만 미리보기 시도
                if !st.cache.contains(*key) {
                    desired.push(Job { key: *key, src: src.clone(), kind: Kind::Preview, prio: 0, generation: w.generation, est_bytes: 0, preview_hint: want.hint() });
                }
                desired.push(Job { key: *key, src: src.clone(), kind: Kind::Decode(want.hint()), prio: 0, generation: w.generation, est_bytes: 0, preview_hint: want.hint() });
            }
        }
        let nh = SizeHint::max_side(w.neighbor_side);
        let mut add_neighbors = |list: &Vec<(EntryKey, ImageSource)>, base: u32, st: &mut State| {
            for (i, (key, src)) in list.iter().enumerate() {
                if !st.cache.has_satisfying(*key, Some(w.neighbor_side)) {
                    desired.push(Job { key: *key, src: src.clone(), kind: Kind::Decode(nh), prio: base + 2 * i as u32, generation: w.generation, est_bytes: 0, preview_hint: nh });
                }
            }
        };
        add_neighbors(&w.ahead, 1, &mut st);
        add_neighbors(&w.behind, 2, &mut st);
        for j in &mut desired {
            j.est_bytes = est_bytes(j.kind);
        }

        let wanted = |key: EntryKey, kind: Kind| desired.iter().find(|d| d.key == key && d.kind == kind);
        // 진행 중: 창 밖이면 취소, 창 안이면 세대 갱신
        for r in &mut st.running {
            match wanted(r.key, r.kind) {
                Some(d) => {
                r.generation = d.generation;
                r.prio = d.prio;
            }
                None => r.cancel.cancel(),
            }
        }
        // 대기열: 창 밖 제거, 창 안은 우선순위 갱신
        st.queue.retain(|j| wanted(j.key, j.kind).is_some());
        for j in &mut st.queue {
            if let Some(d) = wanted(j.key, j.kind) {
                j.prio = d.prio;
                j.generation = d.generation;
            }
        }
        // 새 작업 추가 (이미 대기·진행 중인 건 중복 제거)
        for d in desired.iter() {
            let queued = st.queue.iter().any(|j| j.key == d.key && j.kind == d.kind);
            let running = st.running.iter().any(|r| r.key == d.key && r.kind == d.kind && !r.cancel.is_cancelled());
            if !queued && !running {
                st.queue.push(d.clone());
            }
        }
        self.shared.config.gate.set_busy(current_pending(&st));
        drop(st);
        self.shared.cv.notify_all();
    }

    /// 캐시에서 즉시 꺼내기 (넘길 때 0ms 표시용). 두 번째 값 = 요구 해상도 만족 여부
    pub fn cached(&self, key: EntryKey, need_side: Option<u32>) -> Option<(Arc<Frame>, bool)> {
        self.shared.state.lock().cache.best(key, need_side)
    }

    /// 쌓인 결과를 가져간다 (메인 스레드)
    pub fn drain(&self) -> Vec<LoadEvent> {
        self.shared.pending.store(false, Ordering::SeqCst);
        std::mem::take(&mut self.shared.state.lock().results)
    }

    pub fn stats(&self) -> (crate::CacheStats, usize, usize) {
        let st = self.shared.state.lock();
        (st.cache.stats(), st.queue.len(), st.running.len())
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        let mut st = self.shared.state.lock();
        st.shutdown = true;
        for r in &st.running {
            r.cancel.cancel();
        }
        drop(st);
        self.shared.cv.notify_all();
    }
}

/// 현재 장 본 디코드가 대기·진행 중인가
fn current_pending(st: &State) -> bool {
    st.queue.iter().any(|j| j.prio == 0 && matches!(j.kind, Kind::Decode(_)))
        || st.running.iter().any(|r| r.prio == 0 && matches!(r.kind, Kind::Decode(_)) && !r.cancel.is_cancelled())
}

/// 다음 작업 고르기. 현재 레인은 prio 0 의 본 디코드만.
/// 미리 읽기 워커는 prio 0 작업(미리보기 우선, 현재 레인이 막혀 있으면 본 디코드도)을 먼저 돕고,
/// 현재 장 본 디코드가 끝난 뒤에만 이웃(prio>0)을 예산 안에서 집는다.
fn pick(st: &mut State, current_lane: bool, budget: usize) -> Option<Job> {
    let idx = if current_lane {
        st.queue.iter().enumerate().filter(|(_, j)| j.prio == 0 && matches!(j.kind, Kind::Decode(_))).map(|(i, _)| i).next()
    } else {
        // 현재 장 본 디코드가 대기·진행 중이면 미리 읽기는 기다린다 (CPU·ImageIO 경합으로 현재 장이 늦어지지 않게)
        let current_pending = current_pending(st);
        st.queue
            .iter()
            .enumerate()
            .filter(|(_, j)| j.prio == 0 || (!current_pending && st.inflight_bytes + j.est_bytes <= budget))
            .min_by_key(|(_, j)| (j.prio, !matches!(j.kind, Kind::Preview)))
            .map(|(i, _)| i)
    }?;
    Some(st.queue.swap_remove(idx))
}

fn spawn_worker(shared: &Arc<Shared>, name: &str, current_lane: bool) {
    let shared = shared.clone();
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || worker(shared, current_lane))
        .expect("워커 스레드 생성 실패");
}

fn worker(shared: Arc<Shared>, current_lane: bool) {
    loop {
        let (job, cancel) = {
            let mut st = shared.state.lock();
            let job = loop {
                if st.shutdown {
                    return;
                }
                if let Some(j) = pick(&mut st, current_lane, shared.config.inflight_budget) {
                    break j;
                }
                shared.cv.wait(&mut st);
            };
            let cancel = CancelToken::new();
            st.inflight_bytes += job.est_bytes;
            st.running.push(Running { key: job.key, prio: job.prio, kind: job.kind, cancel: cancel.clone(), est_bytes: job.est_bytes, generation: job.generation });
            (job, cancel)
        };

        let t = Instant::now();
        let result: Result<Option<(Frame, Quality)>, DecodeError> = match job.kind {
            Kind::Preview => shared.registry.preview(&job.src, job.preview_hint, &cancel).map(|o| o.map(|f| (f, Quality::Preview))),
            Kind::Decode(h) => shared.registry.decode(&job.src, h, &cancel).map(|f| Some((f, Quality::Decoded))),
        };
        let ms = t.elapsed().as_secs_f64() * 1000.0;

        let mut st = shared.state.lock();
        let pos = st.running.iter().position(|r| r.cancel.ptr_eq(&cancel));
        let generation = match pos {
            Some(p) => {
                let r = st.running.swap_remove(p);
                st.inflight_bytes -= r.est_bytes;
                r.generation
            }
            None => job.generation,
        };
        // 창 밖으로 밀려 취소된 작업은 성공했더라도 버린다 (캐시 예산·화면 오염 방지)
        let result = if cancel.is_cancelled() { Err(DecodeError::Cancelled) } else { result };
        let event = match result {
            Ok(Some((frame, quality))) => {
                log::debug!("{:?} {} {}x{} {:.1}ms", job.kind, job.src.display_name(), frame.width, frame.height, ms);
                let frame = Arc::new(frame);
                let pinned = st.pinned.clone();
                st.cache.insert(job.key, frame.clone(), &pinned);
                Some(LoadEvent::Loaded(Loaded { key: job.key, generation, frame, quality, decode_ms: ms }))
            }
            Ok(None) => None,
            Err(DecodeError::Cancelled) => None,
            Err(e) => {
                log::warn!("{} 디코드 실패: {e}", job.src.display_name());
                // 미리보기 실패는 알릴 필요 없음 (본 디코드가 따로 돈다)
                (job.kind != Kind::Preview).then(|| LoadEvent::Failed {
                    key: job.key,
                    generation,
                    unsupported: matches!(e, DecodeError::Unsupported(_)),
                    error: e.to_string(),
                })
            }
        };
        shared.config.gate.set_busy(current_pending(&st));
        let notify_more = !st.queue.is_empty();
        if let Some(ev) = event {
            st.results.push(ev);
            drop(st);
            if !shared.pending.swap(true, Ordering::SeqCst) {
                (shared.wake)();
            }
        } else {
            drop(st);
        }
        // 예산이 풀렸으니 대기 중인 다른 워커를 깨운다
        if notify_more {
            shared.cv.notify_all();
        }
    }
}
