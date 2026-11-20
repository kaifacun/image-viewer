//! 디코더별 시간 비교: cargo run --release -p iv-decode-imageio --example decode_bench -- <폴더> [max_side]
use std::time::Instant;

use iv_core::{CancelToken, Decoder, ImageSource, SizeHint};

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("폴더 경로");
    let max: Option<u32> = args.next().and_then(|s| s.parse().ok());
    let hint = SizeHint { max_side: max };
    let decoders: Vec<Box<dyn Decoder>> = vec![
        #[cfg(target_os = "macos")]
        Box::new(iv_decode_imageio::ImageIoDecoder),
        Box::new(iv_decode_image::ImageCrateDecoder),
    ];
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
    files.sort();
    for f in files {
        let src = ImageSource::Path(f.clone());
        let head = src.read_head(64).unwrap_or_default();
        print!("{:<14}", f.file_name().unwrap().to_string_lossy());
        for d in &decoders {
            if d.probe(&src, &head).is_none() {
                print!(" | {:>8}: -            ", d.name());
                continue;
            }
            let t = Instant::now();
            let r = d.decode(&src, hint, &CancelToken::new());
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            match r {
                Ok(fr) => print!(" | {:>8}: {:>6.1}ms {:>5}x{:<5} o{}", d.name(), ms, fr.width, fr.height, fr.orientation as u8),
                Err(e) => print!(" | {:>8}: ERR {}", d.name(), e.to_string().chars().take(30).collect::<String>()),
            }
        }
        if let Some(d) = decoders.first() {
            let t = Instant::now();
            if let Ok(Some(p)) = d.preview(&src, hint, &CancelToken::new()) {
                print!(" | preview {:.1}ms {}x{}", t.elapsed().as_secs_f64() * 1000.0, p.width, p.height);
            }
        }
        println!();
    }
}
