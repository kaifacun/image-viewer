//! 비디오 파일의 대표 장면(포스터)을 한 장의 [`Frame`] 으로 — 격자 썸네일·브라우저 미리보기용.
//! 재생은 이 crate 가 아니다 (`iv-video`).
//!
//! 회전 표시 정보(휴대폰 세로 영상)는 AVFoundation 이 픽셀에 적용해 돌려준다 → `Orientation::Normal`.

/// macOS 기본 엔진(AVFoundation)이 여는 확장자
pub const EXTENSIONS: &[&str] = &["mp4", "m4v", "mov", "qt", "3gp", "3g2"];
/// 추가 코덱 엔진(FFmpeg)이 여는 확장자 (`iv_core::formats` 비디오 묶음의 나머지)
pub const FFMPEG_EXTENSIONS: &[&str] = &["mkv", "webm", "avi", "wmv", "asf", "flv", "f4v", "mpg", "mpeg", "m2v", "vob", "mts", "m2ts", "ogv"];

mod ffmpeg;
pub use ffmpeg::{FfmpegPosterDecoder, NEED_ENGINE};

#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "macos")]
pub use mac::VideoPosterDecoder;

/// 이 플랫폼에서 쓸 수 있으면 등록
pub fn register(registry: &mut iv_core::DecoderRegistry) {
    #[cfg(target_os = "macos")]
    registry.register(VideoPosterDecoder);
    // 엔진이 없어도 등록 — 파일은 격자에 보이고, 썸네일은 엔진을 설치하면 생긴다
    registry.register(FfmpegPosterDecoder);
}
