//! 추가 코덱 엔진(FFmpeg 별도 프로그램)으로 대표 장면 — macOS 기본 엔진이 못 여는 MKV·WebM·AVI…
//! 엔진이 없으면 "코덱 엔진 필요" 오류 (앱이 설치를 권한다). 엔진은 호출 때마다 찾는다(설치 직후 바로 쓰게).

use iv_core::{CancelToken, DecodeError, Decoder, Frame, ImageInfo, ImageSource, Result, SizeHint};

use crate::FFMPEG_EXTENSIONS;

pub struct FfmpegPosterDecoder;

/// 오류 문구 앞머리 — 앱이 이것으로 "엔진 없음"을 알아본다
pub const NEED_ENGINE: &str = "코덱 엔진 필요";

fn tools() -> Result<iv_ffmpeg::Tools> {
    iv_codecpack::find().ok_or_else(|| DecodeError::Unsupported(NEED_ENGINE.into()))
}

impl Decoder for FfmpegPosterDecoder {
    fn name(&self) -> &'static str {
        "FFmpeg 포스터"
    }

    fn extensions(&self) -> &'static [&'static str] {
        FFMPEG_EXTENSIONS
    }

    fn probe(&self, src: &ImageSource, _head: &[u8]) -> Option<i32> {
        let ext = src.extension()?;
        FFMPEG_EXTENSIONS.contains(&ext.as_str()).then_some(25)
    }

    fn info(&self, src: &ImageSource) -> Result<ImageInfo> {
        let path = src.path().ok_or_else(|| DecodeError::Unsupported("경로 아님".into()))?;
        let pr = iv_ffmpeg::probe(&tools()?, path).map_err(DecodeError::InvalidData)?;
        Ok(ImageInfo { size: pr.size, frame_count: 1, format_name: format!("비디오 ({})", pr.video.unwrap_or_default()), ..Default::default() })
    }

    fn decode(&self, src: &ImageSource, hint: SizeHint, cancel: &CancelToken) -> Result<Frame> {
        let path = src.path().ok_or_else(|| DecodeError::Unsupported("경로 아님".into()))?;
        let t = tools()?;
        let pr = iv_ffmpeg::probe(&t, path).map_err(DecodeError::InvalidData)?;
        cancel.check()?;
        let png = iv_ffmpeg::poster_png(&t, path, pr.duration, hint.max_side.unwrap_or(1920)).map_err(DecodeError::InvalidData)?;
        cancel.check()?;
        let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png).map_err(|e| DecodeError::InvalidData(e.to_string()))?.to_rgba8();
        let (w, h) = img.dimensions();
        Ok(Frame::rgba8(w, h, img.into_raw(), pr.size))
    }
}
