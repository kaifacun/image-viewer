//! 재생·프레임 꺼내기 측정: cargo run --release -p iv-video --example spike -- <영상> [최대변]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = std::path::PathBuf::from(&args[1]);
    let max_side: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1920);
    let mut p = iv_video::VideoPlayer::open(&path, max_side, true).expect("열기");
    p.play();
    let t0 = std::time::Instant::now();
    let (mut n, mut copy_ms, mut size) = (0u32, 0.0f64, (0, 0));
    while t0.elapsed().as_secs_f64() < 1.5 {
        // 메인 런루프를 조금씩 돌린다 (앱에선 winit 이 돌린다)
        let until = objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(0.004);
        objc2_foundation::NSRunLoop::currentRunLoop().runUntilDate(&until);
        let t = std::time::Instant::now();
        if let Some(f) = p.poll_frame() {
            copy_ms += t.elapsed().as_secs_f64() * 1000.0;
            n += 1;
            size = (f.width, f.height);
            if n == 1 {
                println!("첫 프레임 {}x{} 방향 {:?} 원본 {:?} ({:.0}ms 후)", f.width, f.height, f.orientation, f.full_size, t0.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }
    println!("{}: {n}프레임 / 1.5초, 프레임 {}x{}, 꺼내기 평균 {:.2}ms, 위치 {:.2}s / {:.2}s", path.display(), size.0, size.1, copy_ms / n.max(1) as f64, p.position(), p.duration());
}
