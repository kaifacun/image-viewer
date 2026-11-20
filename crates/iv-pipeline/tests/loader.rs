//! 가짜 디코더로 파이프라인 동작 검증 (실제 파일 불필요)
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use iv_core::*;
use iv_pipeline::*;

struct Slow(Arc<AtomicUsize>);
impl Decoder for Slow {
    fn name(&self) -> &'static str {
        "slow"
    }
    fn probe(&self, _: &ImageSource, _: &[u8]) -> Option<i32> {
        Some(1)
    }
    fn info(&self, _: &ImageSource) -> Result<ImageInfo> {
        Ok(ImageInfo::default())
    }
    fn decode(&self, _: &ImageSource, hint: SizeHint, cancel: &CancelToken) -> Result<Frame> {
        self.0.fetch_add(1, Ordering::SeqCst);
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(5));
            cancel.check()?;
        }
        let s = hint.max_side.unwrap_or(64).min(64);
        Ok(Frame::rgba8(s, s, vec![0; (s * s * 4) as usize], [1000, 1000]))
    }
}

fn src() -> ImageSource {
    ImageSource::Path(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
}

fn window(g: u64, cur: u64, ahead: &[u64]) -> Window {
    Window {
        generation: g,
        current: Some((cur, src())),
        current_want: Want::Side(64),
        ahead: ahead.iter().map(|k| (*k, src())).collect(),
        behind: vec![],
        neighbor_side: 64,
    }
}

fn wait_for(loader: &Loader, mut pred: impl FnMut(&LoadEvent) -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(5) {
        if loader.drain().iter().any(&mut pred) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
fn current_and_neighbors_load_and_dedupe() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = DecoderRegistry::new();
    reg.register(Slow(calls.clone()));
    let loader = Loader::new(Arc::new(reg), LoaderConfig::default(), || {});
    loader.set_window(&window(1, 10, &[11, 12]));
    // 같은 창을 여러 번 알려도 중복 디코드 없음
    loader.set_window(&window(1, 10, &[11, 12]));
    assert!(wait_for(&loader, |e| matches!(e, LoadEvent::Loaded(l) if l.key == 10)));
    std::thread::sleep(Duration::from_millis(300));
    assert!(loader.cached(11, Some(64)).is_some());
    assert!(loader.cached(12, Some(64)).is_some());
    assert_eq!(calls.load(Ordering::SeqCst), 3);

    // 다음 장으로 넘기면 캐시에서 즉시
    loader.set_window(&window(2, 11, &[12, 13]));
    assert!(loader.cached(11, Some(64)).unwrap().1);
}

#[test]
fn moving_away_cancels_out_of_window_jobs() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = DecoderRegistry::new();
    reg.register(Slow(calls.clone()));
    let loader = Loader::new(Arc::new(reg), LoaderConfig::default(), || {});
    loader.set_window(&window(1, 1, &[2, 3]));
    std::thread::sleep(Duration::from_millis(10));
    // 멀리 점프 → 1,2,3 은 취소돼 결과가 오지 않아야 한다
    loader.set_window(&window(2, 100, &[]));
    assert!(wait_for(&loader, |e| matches!(e, LoadEvent::Loaded(l) if l.key == 100)));
    std::thread::sleep(Duration::from_millis(200));
    let late: Vec<_> = loader.drain();
    assert!(late.iter().all(|e| !matches!(e, LoadEvent::Loaded(l) if l.key == 1 || l.key == 2 || l.key == 3)));
}

#[test]
fn wake_called_without_loss() {
    let woke = Arc::new(AtomicUsize::new(0));
    let w2 = woke.clone();
    let mut reg = DecoderRegistry::new();
    reg.register(Slow(Arc::new(AtomicUsize::new(0))));
    let loader = Loader::new(Arc::new(reg), LoaderConfig::default(), move || {
        w2.fetch_add(1, Ordering::SeqCst);
    });
    loader.set_window(&window(1, 1, &[2, 3, 4]));
    std::thread::sleep(Duration::from_millis(500));
    assert!(woke.load(Ordering::SeqCst) >= 1);
    let n = loader.drain().len();
    assert_eq!(n, 4);
}
