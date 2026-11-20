//! 넘기기 체감 시간 실측 (창 없이 실제 디코더·파이프라인·뷰어로)
//! cargo run --release -p iv-viewer --example nav_bench -- <폴더> [머무는ms=300] [화면긴변=2560]
use std::sync::Arc;
use std::time::{Duration, Instant};

use iv_core::DecoderRegistry;
use iv_pipeline::{Loader, LoaderConfig, Quality};
use iv_viewer::{Action, ViewerConfig, ViewerState};

fn main() {
    let mut a = std::env::args().skip(1);
    let dir = a.next().expect("폴더");
    let dwell: u64 = a.next().and_then(|s| s.parse().ok()).unwrap_or(300);
    let side: f32 = a.next().and_then(|s| s.parse().ok()).unwrap_or(2560.0);

    let mut reg = DecoderRegistry::new();
    iv_decode_imageio::register(&mut reg);
    reg.register(iv_decode_image::ImageCrateDecoder);
    let reg = Arc::new(reg);
    let exts = reg.known_extensions();
    let loader = Loader::new(reg, LoaderConfig::default(), || {});
    let mut v = ViewerState::new([side, side * 0.625], exts, ViewerConfig::default());

    let fx = v.open(std::path::Path::new(&dir), &loader);
    if fx.window_changed {
        loader.set_window(&v.window());
    }
    let n = v.library.len();
    println!("{} 장, 머무름 {dwell}ms, 화면 긴 변 {side}", n);
    let mut lat = Vec::new();
    for i in 0..n {
        if i > 0 {
            let fx = v.handle(Action::Next, &loader);
            if fx.window_changed {
                loader.set_window(&v.window());
            }
        }
        // 넘긴 순간 캐시에 있었나?
        let t = Instant::now();
        let instant = v.shown.as_ref().is_some_and(|s| s.quality == Quality::Decoded);
        let mut first_any = if v.shown.is_some() { Some(0.0) } else { None };
        // 본 디코드가 화면에 올라올 때까지
        loop {
            for e in loader.drain() {
                let _ = v.on_load_event(e);
            }
            if first_any.is_none() && v.shown.is_some() {
                first_any = Some(t.elapsed().as_secs_f64() * 1000.0);
            }
            if v.shown.as_ref().is_some_and(|s| s.quality == Quality::Decoded) || v.error.is_some() {
                break;
            }
            if t.elapsed() > Duration::from_secs(10) {
                break;
            }
            std::thread::sleep(Duration::from_micros(500));
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let name = v.current().map(|e| e.name.clone()).unwrap_or_default();
        let state = if v.error.is_some() { "오류" } else if instant { "캐시" } else { "디코드" };
        println!("{:>3} {:<14} {:>7.1}ms  {}", i + 1, name, ms, state);
        if v.error.is_none() {
            lat.push(ms);
        }
        std::thread::sleep(Duration::from_millis(dwell));
    }
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if !lat.is_empty() {
        println!("중앙값 {:.1}ms · 최대 {:.1}ms", lat[lat.len() / 2], lat[lat.len() - 1]);
    }
    let (c, _, _) = loader.stats();
    println!("캐시 {} MB / {} 장", c.bytes >> 20, c.frames);
}
