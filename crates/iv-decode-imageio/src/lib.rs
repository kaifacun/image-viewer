//! macOS ImageIO 디코더.
//!
//! - HEIC/AVIF/카메라 RAW/JPEG/PNG/TIFF/WebP… OS 가 아는 거의 모든 형식
//! - `CGImageSourceCreateThumbnailAtIndex` 로 축소 디코딩(JPEG 는 DCT 스케일링)
//! - 내장 미리보기(EXIF 썸네일·RAW 프리뷰) 우선 표시
//!
//! macOS 가 아니면 빈 crate 이며 `register` 는 아무것도 하지 않는다.

#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "macos")]
pub use mac::ImageIoDecoder;
/// CGImage → RGBA8 sRGB 픽셀 (비디오 포스터 등 다른 macOS 디코더가 같이 쓴다)
#[cfg(target_os = "macos")]
pub use mac::to_rgba8 as cg_image_to_rgba8;

/// 이 플랫폼에서 쓸 수 있으면 레지스트리에 등록한다.
pub fn register(_registry: &mut iv_core::DecoderRegistry) {
    #[cfg(target_os = "macos")]
    _registry.register(ImageIoDecoder);
}
