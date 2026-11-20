//! 디코더 등록 — 새 포맷 crate 는 여기에 한 줄 추가한다.

use iv_core::DecoderRegistry;

pub fn build_registry() -> DecoderRegistry {
    let mut r = DecoderRegistry::new();
    // 플랫폼 디코더 (우선순위 높음): 맥 ImageIO — HEIC/AVIF/RAW/축소 디코딩
    iv_decode_imageio::register(&mut r);
    // 범용 순수 Rust 디코더 (폴백)
    r.register(iv_decode_image::ImageCrateDecoder);
    r
}
