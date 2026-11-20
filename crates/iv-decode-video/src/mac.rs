use iv_core::{CancelToken, DecodeError, Decoder, Frame, ImageInfo, ImageSource, Result, SizeHint};
use objc2::rc::{Retained, autoreleasepool};
use objc2_av_foundation::{AVAssetImageGenerator, AVMediaTypeVideo, AVURLAsset};
use objc2_core_foundation::CGSize;
use objc2_core_media::CMTime;
use objc2_foundation::{NSString, NSURL};

use crate::EXTENSIONS;

pub struct VideoPosterDecoder;

fn asset(src: &ImageSource) -> Result<Retained<AVURLAsset>> {
    let path = src.path().ok_or_else(|| DecodeError::Unsupported("경로 아님".into()))?;
    if !path.exists() {
        return Err(DecodeError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, path.display().to_string())));
    }
    let s = path.to_str().ok_or_else(|| DecodeError::InvalidData("경로가 UTF-8 이 아님".into()))?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(s));
    Ok(unsafe { AVURLAsset::URLAssetWithURL_options(&url, None) })
}

/// 화면에 보이는 크기 (회전 표시 정보 적용)
#[allow(deprecated)]
fn display_size(a: &AVURLAsset) -> Result<[u32; 2]> {
    let ty = unsafe { AVMediaTypeVideo }.ok_or_else(|| DecodeError::Unsupported("AVMediaTypeVideo 없음".into()))?;
    let tracks = unsafe { a.tracksWithMediaType(ty) };
    let t = tracks.firstObject().ok_or_else(|| DecodeError::Unsupported("영상 트랙 없음".into()))?;
    let n = unsafe { t.naturalSize() };
    let m = unsafe { t.preferredTransform() };
    let (w, h) = (n.width.abs().round() as u32, n.height.abs().round() as u32);
    if w == 0 || h == 0 {
        return Err(DecodeError::InvalidData("영상 크기 0".into()));
    }
    // 90°/270° 회전이면 가로·세로가 바뀐다 (b, c 성분이 0 이 아님)
    Ok(if m.b.abs() > 0.5 { [h, w] } else { [w, h] })
}

impl Decoder for VideoPosterDecoder {
    fn name(&self) -> &'static str {
        "AVFoundation 포스터"
    }

    fn extensions(&self) -> &'static [&'static str] {
        EXTENSIONS
    }

    fn probe(&self, src: &ImageSource, _head: &[u8]) -> Option<i32> {
        let ext = src.extension()?;
        EXTENSIONS.contains(&ext.as_str()).then_some(30)
    }

    fn info(&self, src: &ImageSource) -> Result<ImageInfo> {
        autoreleasepool(|_| {
            let a = asset(src)?;
            Ok(ImageInfo { size: display_size(&a)?, frame_count: 1, format_name: "비디오".into(), ..Default::default() })
        })
    }

    #[allow(deprecated)]
    fn decode(&self, src: &ImageSource, hint: SizeHint, cancel: &CancelToken) -> Result<Frame> {
        autoreleasepool(|_| {
            let a = asset(src)?;
            let full = display_size(&a)?;
            cancel.check()?;
            let g = unsafe { AVAssetImageGenerator::assetImageGeneratorWithAsset(&a) };
            unsafe { g.setAppliesPreferredTrackTransform(true) };
            if let Some(m) = hint.max_side {
                unsafe { g.setMaximumSize(CGSize::new(m as f64, m as f64)) };
            }
            // 첫 장면은 검은 화면이 많아 1초(짧으면 길이의 10%) 지점
            let dur = unsafe { a.duration().seconds() };
            let at = if dur.is_finite() && dur > 0.0 { (dur * 0.1).min(1.0) } else { 0.0 };
            let img = unsafe { g.copyCGImageAtTime_actualTime_error(CMTime::with_seconds(at, 600), std::ptr::null_mut()) }
                .map_err(|e| DecodeError::InvalidData(format!("포스터 실패: {}", e.localizedDescription())))?;
            cancel.check()?;
            let (w, h, px) = iv_decode_imageio::cg_image_to_rgba8(&img)?;
            Ok(Frame::rgba8(w, h, px, full))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fixture(name: &str) -> Option<ImageSource> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scratch/videos").join(name);
        if !p.exists() {
            eprintln!("시험 영상 없음 — tools/make-video-fixtures.sh");
            return None;
        }
        Some(ImageSource::Path(p))
    }

    #[test]
    fn poster_and_rotation() {
        let Some(src) = fixture("counter.mp4") else { return };
        let d = VideoPosterDecoder;
        assert_eq!(d.info(&src).unwrap().size, [1280, 720]);
        let f = d.decode(&src, SizeHint::max_side(320), &CancelToken::new()).unwrap();
        assert_eq!((f.width, f.height), (320, 180));
        assert_eq!(f.full_size, [1280, 720]);
        // 세로 영상: 표시 크기도, 포스터 픽셀도 세로
        let Some(p) = fixture("portrait.mov") else { return };
        assert_eq!(d.info(&p).unwrap().size, [720, 1280]);
        let f = d.decode(&p, SizeHint::max_side(320), &CancelToken::new()).unwrap();
        assert!(f.height > f.width, "{}x{}", f.width, f.height);
    }
}
