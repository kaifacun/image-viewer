//! `image` 크레이트 기반 디코더. 어느 OS 에서나 동작하는 기본 디코더이며,
//! 플랫폼 디코더(ImageIO/WIC)가 실패할 때의 폴백이기도 하다.

use std::io::Cursor;

use image::{ImageFormat, ImageReader};
use iv_core::{CancelToken, Caps, DecodeError, Decoder, Frame, ImageInfo, ImageSource, Orientation, Result, SizeHint};

use iv_imageops::downscale_rgba8;

#[derive(Default)]
pub struct ImageCrateDecoder;

/// 기본 우선순위 (플랫폼 디코더보다 낮게)
const PRIORITY: i32 = 10;

fn format_of(head: &[u8], ext: Option<&str>) -> Option<ImageFormat> {
    image::guess_format(head).ok().or_else(|| ext.and_then(ImageFormat::from_extension))
}

fn map_err(e: image::ImageError) -> DecodeError {
    use image::ImageError as E;
    match e {
        E::IoError(io) => DecodeError::Io(io),
        E::Unsupported(u) => DecodeError::Unsupported(u.to_string()),
        E::Limits(l) => DecodeError::ResourceLimit(l.to_string()),
        other => DecodeError::InvalidData(other.to_string()),
    }
}

/// EXIF 방향 읽기 (JPEG/TIFF/PNG/WebP/HEIF 컨테이너 모두 kamadak-exif 가 처리)
pub fn read_orientation(bytes: &[u8]) -> Orientation {
    let reader = exif::Reader::new();
    match reader.read_from_container(&mut Cursor::new(bytes)) {
        Ok(ex) => ex
            .get_field(exif::Tag::Orientation, exif::In::PRIMARY)
            .and_then(|f| f.value.get_uint(0))
            .map(Orientation::from_exif)
            .unwrap_or_default(),
        Err(_) => Orientation::Normal,
    }
}

impl Decoder for ImageCrateDecoder {
    fn name(&self) -> &'static str {
        "image-rs"
    }

    fn capabilities(&self) -> Caps {
        // 축소는 디코드 후 리샘플(메모리·업로드 절감)이라 scaled=false 로 정직하게 표기
        Caps::default()
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["jpg", "jpeg", "jpe", "jfif", "png", "apng", "gif", "webp", "bmp", "tif", "tiff", "ico", "tga", "pbm", "pgm", "ppm", "pnm", "qoi"]
    }

    fn probe(&self, src: &ImageSource, head: &[u8]) -> Option<i32> {
        format_of(head, src.extension().as_deref()).map(|_| PRIORITY)
    }

    fn info(&self, src: &ImageSource) -> Result<ImageInfo> {
        let bytes = src.read_all()?;
        let reader = ImageReader::new(Cursor::new(&bytes)).with_guessed_format()?;
        let fmt = reader.format();
        let (w, h) = reader.into_dimensions().map_err(map_err)?;
        Ok(ImageInfo {
            size: [w, h],
            orientation: read_orientation(&bytes),
            frame_count: 1,
            format_name: fmt.map(|f| format!("{f:?}")).unwrap_or_default(),
            ..Default::default()
        })
    }

    fn decode(&self, src: &ImageSource, hint: SizeHint, cancel: &CancelToken) -> Result<Frame> {
        let bytes = src.read_all()?;
        cancel.check()?;
        let orientation = read_orientation(&bytes);
        let mut reader = ImageReader::new(Cursor::new(&bytes)).with_guessed_format()?;
        reader.no_limits();
        let img = reader.decode().map_err(map_err)?;
        cancel.check()?;
        let rgba = img.into_rgba8();
        let (w, h) = rgba.dimensions();
        let frame = Frame::rgba8(w, h, rgba.into_raw(), [w, h]).with_orientation(orientation);
        Ok(match hint.max_side {
            Some(m) if m < frame.max_side() => downscale_rgba8(&frame, m),
            _ => frame,
        })
    }
}
