//! 비디오 파일의 대표 장면(포스터)을 한 장의 [`Frame`] 으로 — 격자 썸네일·브라우저 미리보기용.
//! 재생은 이 crate 가 아니다 (`iv-video`).
//!
//! 회전 표시 정보(휴대폰 세로 영상)는 AVFoundation 이 픽셀에 적용해 돌려준다 → `Orientation::Normal`.

/// 다루는 확장자 (`iv_core::formats` 의 비디오 묶음과 같아야 한다)
pub const EXTENSIONS: &[&str] = &["mp4", "m4v", "mov", "qt", "3gp", "3g2"];

#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "macos")]
pub use mac::VideoPosterDecoder;

/// 이 플랫폼에서 쓸 수 있으면 등록
pub fn register(_registry: &mut iv_core::DecoderRegistry) {
    #[cfg(target_os = "macos")]
    _registry.register(VideoPosterDecoder);
}
