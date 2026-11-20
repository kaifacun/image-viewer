use std::ffi::c_void;

use iv_core::{CancelToken, Caps, DecodeError, Decoder, Frame, ImageInfo, ImageSource, Orientation, Result, SizeHint};
use objc2::rc::autoreleasepool;
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CFURL, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo, CGInterpolationQuality, kCGColorSpaceSRGB,
};
use objc2_image_io::{
    CGImageSource, kCGImagePropertyOrientation, kCGImagePropertyPixelHeight, kCGImagePropertyPixelWidth,
    kCGImageSourceCreateThumbnailFromImageAlways, kCGImageSourceCreateThumbnailFromImageIfAbsent,
    kCGImageSourceCreateThumbnailWithTransform, kCGImageSourceShouldCache, kCGImageSourceShouldCacheImmediately,
    kCGImageSourceThumbnailMaxPixelSize,
};

/// image-rs(10) 보다 먼저 시도
const PRIORITY: i32 = 20;

/// ImageIO 가 다루는 확장자 (후보 축소 힌트. 최종 판정은 CGImageSource 생성 성공 여부)
const EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "gif", "tif", "tiff", "bmp", "ico", "icns", "heic", "heif", "hif", "avif", "webp",
    "jp2", "j2k", "jpx", "psd", "tga", "exr", "hdr", "pbm", "pgm", "ppm", "dng", "cr2", "cr3", "crw", "nef", "nrw", "arw",
    "srf", "sr2", "raf", "orf", "rw2", "rwl", "pef", "srw", "x3f", "3fr", "fff", "iiq", "mos", "mrw", "erf", "dcr", "kdc",
];

pub struct ImageIoDecoder;

fn open(src: &ImageSource) -> Result<CFRetained<CGImageSource>> {
    let path = src.path().ok_or_else(|| DecodeError::Unsupported("경로 아님".into()))?;
    if !path.exists() {
        return Err(DecodeError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, path.display().to_string())));
    }
    let url = CFURL::from_file_path(path).ok_or_else(|| DecodeError::InvalidData("URL 변환 실패".into()))?;
    // 소스 자체는 디코드 결과를 캐시하지 않게 (우리 LRU 가 관리)
    let opts = dict(&[(unsafe { kCGImageSourceShouldCache }, CFBoolean::new(false).as_ref())]);
    unsafe { CGImageSource::with_url(&url, Some(opts.as_opaque())) }
        .filter(|s| unsafe { s.count() } > 0)
        .ok_or_else(|| DecodeError::Unsupported("ImageIO 가 열 수 없음".into()))
}

fn dict(pairs: &[(&CFString, &CFType)]) -> CFRetained<CFDictionary<CFString, CFType>> {
    let keys: Vec<&CFString> = pairs.iter().map(|p| p.0).collect();
    let vals: Vec<&CFType> = pairs.iter().map(|p| p.1).collect();
    CFDictionary::from_slices(&keys, &vals)
}

fn number_of(props: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<i64> {
    props.get(key)?.downcast::<CFNumber>().ok()?.as_i64()
}

fn read_info(source: &CGImageSource) -> Result<([u32; 2], Orientation)> {
    let idx = unsafe { source.primary_image_index() };
    let props = unsafe { source.properties_at_index(idx, None) }
        .ok_or_else(|| DecodeError::InvalidData("속성 없음".into()))?;
    // SAFETY: ImageIO 속성 사전은 CFString 키를 쓴다
    let props: CFRetained<CFDictionary<CFString, CFType>> = unsafe { CFRetained::cast_unchecked(props) };
    let w = number_of(&props, unsafe { kCGImagePropertyPixelWidth }).unwrap_or(0);
    let h = number_of(&props, unsafe { kCGImagePropertyPixelHeight }).unwrap_or(0);
    if w <= 0 || h <= 0 {
        return Err(DecodeError::InvalidData("크기 정보 없음".into()));
    }
    let o = number_of(&props, unsafe { kCGImagePropertyOrientation }).unwrap_or(1);
    Ok(([w as u32, h as u32], Orientation::from_exif(o as u32)))
}

/// CGImage → RGBA8 sRGB straight-alpha 픽셀
pub fn to_rgba8(img: &CGImage) -> Result<(u32, u32, Vec<u8>)> {
    let w = CGImage::width(Some(img));
    let h = CGImage::height(Some(img));
    if w == 0 || h == 0 {
        return Err(DecodeError::InvalidData("빈 이미지".into()));
    }
    let stride = w * 4;
    let mut buf = vec![0u8; stride * h];
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
        .ok_or_else(|| DecodeError::InvalidData("sRGB 색공간 생성 실패".into()))?;
    // 비트맵 컨텍스트는 premultiplied 만 지원 → 그린 뒤 알파가 있으면 되돌린다
    let info = CGImageAlphaInfo::PremultipliedLast.0;
    let ctx = unsafe {
        CGBitmapContextCreate(buf.as_mut_ptr() as *mut c_void, w, h, 8, stride, Some(&space), info)
    }
    .ok_or_else(|| DecodeError::ResourceLimit("비트맵 컨텍스트 생성 실패".into()))?;
    CGContext::set_interpolation_quality(Some(&ctx), CGInterpolationQuality::High);
    let rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64));
    CGContext::draw_image(Some(&ctx), rect, Some(img));
    drop(ctx);
    unpremultiply(&mut buf);
    Ok((w as u32, h as u32, buf))
}

fn unpremultiply(buf: &mut [u8]) {
    for px in buf.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

fn thumbnail(source: &CGImageSource, max_side: Option<u32>, embedded_only: bool) -> Option<CFRetained<CGImage>> {
    let idx = unsafe { source.primary_image_index() };
    let t = CFBoolean::new(true);
    let f = CFBoolean::new(false);
    let max = max_side.map(|m| CFNumber::new_i64(m as i64));
    let mut pairs: Vec<(&CFString, &CFType)> = vec![
        // 방향은 렌더러가 적용 → 여기서 회전하지 않음
        (unsafe { kCGImageSourceCreateThumbnailWithTransform }, f.as_ref()),
        (unsafe { kCGImageSourceShouldCacheImmediately }, t.as_ref()),
    ];
    if embedded_only {
        pairs.push((unsafe { kCGImageSourceCreateThumbnailFromImageIfAbsent }, f.as_ref()));
        pairs.push((unsafe { kCGImageSourceCreateThumbnailFromImageAlways }, f.as_ref()));
    } else {
        pairs.push((unsafe { kCGImageSourceCreateThumbnailFromImageAlways }, t.as_ref()));
    }
    if let Some(m) = &max {
        pairs.push((unsafe { kCGImageSourceThumbnailMaxPixelSize }, m.as_ref()));
    }
    let opts = dict(&pairs);
    unsafe { source.thumbnail_at_index(idx, Some(opts.as_opaque())) }
}

fn frame_from(img: &CGImage, full: [u32; 2], o: Orientation, is_preview: bool) -> Result<Frame> {
    let (w, h, px) = to_rgba8(img)?;
    let mut f = Frame::rgba8(w, h, px, full).with_orientation(o);
    f.is_preview = is_preview;
    Ok(f)
}

impl Decoder for ImageIoDecoder {
    fn name(&self) -> &'static str {
        "ImageIO"
    }

    fn capabilities(&self) -> Caps {
        Caps { preview: true, scaled: true, ..Default::default() }
    }

    fn extensions(&self) -> &'static [&'static str] {
        EXTENSIONS
    }

    fn probe(&self, src: &ImageSource, _head: &[u8]) -> Option<i32> {
        let ext = src.extension()?;
        EXTENSIONS.contains(&ext.as_str()).then_some(PRIORITY)
    }

    fn info(&self, src: &ImageSource) -> Result<ImageInfo> {
        autoreleasepool(|_| {
            let s = open(src)?;
            let (size, orientation) = read_info(&s)?;
            let format_name = unsafe { s.r#type() }.map(|t| t.to_string()).unwrap_or_default();
            Ok(ImageInfo { size, orientation, frame_count: unsafe { s.count() } as u32, format_name, ..Default::default() })
        })
    }

    fn preview(&self, src: &ImageSource, hint: SizeHint, cancel: &CancelToken) -> Result<Option<Frame>> {
        // 내장 미리보기는 RAW·HEIF 에서만 의미가 있다. JPEG 등은 내장 썸네일이 없으면 ImageIO 가
        // 원본 전체를 디코드해 버려(6016px 645ms) 오히려 느려진다 — 본 축소 디코드가 이미 빠르다.
        const PREVIEW_EXT: &[&str] = &[
            "heic", "heif", "hif", "dng", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "rwl",
            "pef", "srw", "x3f", "3fr", "fff", "iiq", "mos", "mrw", "erf", "dcr", "kdc",
        ];
        if !src.extension().is_some_and(|e| PREVIEW_EXT.contains(&e.as_str())) {
            return Ok(None);
        }
        autoreleasepool(|_| {
            let s = open(src)?;
            cancel.check()?;
            let (full, o) = read_info(&s)?;
            let Some(img) = thumbnail(&s, hint.max_side, true) else { return Ok(None) };
            let f = frame_from(&img, full, o, true)?;
            // 너무 작은 내장 썸네일(160px 등)은 오히려 흐릿해 보이므로 버린다
            let cap = hint.max_side.unwrap_or(u32::MAX);
            Ok((f.max_side() >= 320 && f.max_side() <= cap.max(320)).then_some(f))
        })
    }

    fn decode(&self, src: &ImageSource, hint: SizeHint, cancel: &CancelToken) -> Result<Frame> {
        autoreleasepool(|_| {
            let s = open(src)?;
            cancel.check()?;
            let (full, o) = read_info(&s)?;
            let full_max = full[0].max(full[1]);
            let img = match hint.max_side {
                Some(m) if m < full_max => thumbnail(&s, Some(m), false),
                _ => {
                    let idx = unsafe { s.primary_image_index() };
                    unsafe { s.image_at_index(idx, None) }
                }
            }
            .ok_or_else(|| DecodeError::InvalidData("ImageIO 디코드 실패".into()))?;
            cancel.check()?;
            frame_from(&img, full, o, false)
        })
    }
}
