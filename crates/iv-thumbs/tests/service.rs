use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use iv_core::{DecoderRegistry, ImageSource};
use iv_thumbs::{ThumbConfig, ThumbEvent, ThumbRequest, ThumbService};

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iv-thumbs-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_png(path: &PathBuf, w: u32, h: u32, alpha: u8) {
    let px: Vec<u8> = (0..w * h).flat_map(|i| [(i % 255) as u8, 80, 160, alpha]).collect();
    image::save_buffer(path, &px, w, h, image::ExtendedColorType::Rgba8).unwrap();
}

fn req(key: u64, p: &PathBuf) -> ThumbRequest {
    let m = std::fs::metadata(p).unwrap();
    ThumbRequest { key, src: ImageSource::Path(p.clone()), mtime: m.modified().unwrap(), len: m.len() }
}

fn wait_events(svc: &ThumbService, n: usize) -> Vec<ThumbEvent> {
    let mut out = Vec::new();
    let t = Instant::now();
    while out.len() < n && t.elapsed() < Duration::from_secs(10) {
        out.extend(svc.drain());
        std::thread::sleep(Duration::from_millis(5));
    }
    out
}

fn registry() -> Arc<DecoderRegistry> {
    let mut r = DecoderRegistry::new();
    r.register(iv_decode_image::ImageCrateDecoder);
    Arc::new(r)
}

#[test]
fn generates_caches_on_disk_and_reloads() {
    let d = tmpdir("disk");
    let img = d.join("a.png");
    let alpha = d.join("b.png");
    let broken = d.join("c.png");
    write_png(&img, 1000, 500, 255);
    write_png(&alpha, 300, 600, 128);
    std::fs::write(&broken, b"nope").unwrap();
    let cache = d.join("cache");
    let cfg = ThumbConfig { side: 200, disk_dir: Some(cache.clone()), gate: None, ..Default::default() };
    let wakes = Arc::new(AtomicUsize::new(0));
    let w2 = wakes.clone();
    let svc = ThumbService::new(registry(), cfg.clone(), move || {
        w2.fetch_add(1, Ordering::SeqCst);
    });
    svc.request(&[req(1, &img), req(2, &alpha), req(3, &broken)]);
    let evs = wait_events(&svc, 3);
    assert_eq!(evs.len(), 3, "{evs:?}");
    assert!(wakes.load(Ordering::SeqCst) >= 1);
    let t1 = svc.get(1).expect("메모리에 있어야 함");
    assert_eq!((t1.image.width, t1.image.height), (200, 100));
    assert_eq!(t1.full_size, [1000, 500]);
    assert!(svc.failed(3).is_some());
    // 실패한 것은 다시 요청해도 대기열에 안 들어감
    svc.request(&[req(3, &broken)]);
    assert_eq!(svc.stats().2, 0);
    drop(svc);

    // 새 서비스: 디스크에서 즉시 (알파는 PNG 로 보존)
    let svc = ThumbService::new(registry(), cfg.clone(), || {});
    let t = Instant::now();
    svc.request(&[req(1, &img), req(2, &alpha)]);
    let evs = wait_events(&svc, 2);
    assert_eq!(evs.len(), 2);
    assert!(t.elapsed() < Duration::from_secs(2));
    let t2 = svc.get(2).unwrap();
    assert_eq!(t2.full_size, [300, 600]);
    assert_eq!(t2.image.pixels[3], 128);

    // 파일이 바뀌면(len 다름) 캐시를 쓰지 않고 재생성 → 새 크기
    write_png(&img, 400, 400, 255);
    svc.request(&[req(10, &img)]);
    let evs = wait_events(&svc, 1);
    match &evs[0] {
        ThumbEvent::Ready(t) => assert_eq!(t.full_size, [400, 400]),
        e => panic!("{e:?}"),
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn paused_holds_and_request_replaces_queue() {
    let d = tmpdir("pause");
    let files: Vec<PathBuf> = (0..6).map(|i| d.join(format!("{i}.png"))).collect();
    for f in &files {
        write_png(f, 64, 64, 255);
    }
    let svc = ThumbService::new(registry(), ThumbConfig { side: 32, disk_dir: None, workers: 1, gate: None, ..Default::default() }, || {});
    svc.set_paused(true);
    svc.request(&files.iter().enumerate().map(|(i, f)| req(i as u64, f)).collect::<Vec<_>>());
    std::thread::sleep(Duration::from_millis(100));
    assert!(svc.drain().is_empty());
    // 보이는 목록이 바뀜 → 4,5 만
    svc.request(&[req(4, &files[4]), req(5, &files[5])]);
    svc.set_paused(false);
    let evs = wait_events(&svc, 2);
    std::thread::sleep(Duration::from_millis(100));
    let mut keys: Vec<u64> = evs.iter().chain(svc.drain().iter()).map(|e| match e { ThumbEvent::Ready(t) => t.key, ThumbEvent::Failed { key, .. } => *key }).collect();
    keys.sort();
    assert_eq!(keys, [4, 5]);
    let _ = std::fs::remove_dir_all(&d);
}
