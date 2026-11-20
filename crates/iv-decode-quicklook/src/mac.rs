use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use iv_core::{CancelToken, DecodeError, Decoder, Frame, ImageInfo, ImageSource, Result, SizeHint};
use objc2::AnyThread;
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSError, NSString, NSURL};
use objc2_quick_look_thumbnailing::{QLThumbnailGenerationRequest, QLThumbnailGenerationRequestRepresentationTypes, QLThumbnailGenerator, QLThumbnailRepresentation};

use crate::EXTENSIONS;

pub struct QuickLookDecoder;

/// 훑어보기 썸네일 → RGBA 픽셀
fn thumbnail(path: &std::path::Path, side: u32) -> Result<(u32, u32, Vec<u8>)> {
    let s = path.to_str().ok_or_else(|| DecodeError::InvalidData("경로가 UTF-8 이 아님".into()))?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(s));
    let side = side.clamp(64, 2048) as f64;
    // 진짜 미리보기 우선, 없으면 파일 아이콘 (빈칸보다 낫다)
    let types = QLThumbnailGenerationRequestRepresentationTypes::Thumbnail | QLThumbnailGenerationRequestRepresentationTypes::Icon;
    let req = unsafe { QLThumbnailGenerationRequest::initWithFileAtURL_size_scale_representationTypes(QLThumbnailGenerationRequest::alloc(), &url, CGSize::new(side, side), 1.0, types) };
    let (tx, rx) = mpsc::channel::<std::result::Result<(u32, u32, Vec<u8>), String>>();
    let block = RcBlock::new(move |rep: *mut QLThumbnailRepresentation, err: *mut NSError| {
        // SAFETY: 완료 처리기 인자 — 둘 중 하나는 null 이 아니다 (호출 동안만 유효)
        let r = match unsafe { rep.as_ref() } {
            Some(rep) => {
                let img = unsafe { rep.CGImage() };
                iv_decode_imageio::cg_image_to_rgba8(&img).map_err(|e| e.to_string())
            }
            None => Err(unsafe { err.as_ref() }.map_or("미리보기 없음".to_string(), |e| e.localizedDescription().to_string())),
        };
        let _ = tx.send(r);
    });
    let generator = unsafe { QLThumbnailGenerator::sharedGenerator() };
    unsafe { generator.generateBestRepresentationForRequest_completionHandler(&req, &block) };
    match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(r) => r.map_err(DecodeError::InvalidData),
        Err(_) => Err(DecodeError::InvalidData("미리보기 시간 초과".into())),
    }
}

impl Decoder for QuickLookDecoder {
    fn name(&self) -> &'static str {
        "훑어보기"
    }

    fn extensions(&self) -> &'static [&'static str] {
        EXTENSIONS
    }

    fn probe(&self, src: &ImageSource, _head: &[u8]) -> Option<i32> {
        let ext = src.extension()?;
        EXTENSIONS.contains(&ext.as_str()).then_some(15)
    }

    fn info(&self, src: &ImageSource) -> Result<ImageInfo> {
        let p = src.path().ok_or_else(|| DecodeError::Unsupported("경로 아님".into()))?;
        let (w, h, _) = thumbnail(p, 256)?;
        Ok(ImageInfo { size: [w, h], frame_count: 1, format_name: "문서".into(), ..Default::default() })
    }

    fn decode(&self, src: &ImageSource, hint: SizeHint, cancel: &CancelToken) -> Result<Frame> {
        let p = src.path().ok_or_else(|| DecodeError::Unsupported("경로 아님".into()))?;
        cancel.check()?;
        let (w, h, px) = thumbnail(p, hint.max_side.unwrap_or(1600))?;
        Ok(Frame::rgba8(w, h, px, [w, h]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_rtf_thumbnails() {
        let d = std::env::temp_dir().join(format!("iv-ql-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let txt = d.join("a.txt");
        std::fs::write(&txt, "Hello KaiIV\n문서 미리보기\n".repeat(20)).unwrap();
        let f = QuickLookDecoder.decode(&ImageSource::Path(txt), SizeHint::max_side(256), &CancelToken::new()).unwrap();
        assert!(f.width > 16 && f.height > 16, "{}x{}", f.width, f.height);
        assert!(f.width.max(f.height) <= 256);
        let _ = std::fs::remove_dir_all(&d);
    }
}
