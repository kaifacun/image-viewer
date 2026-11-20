//! PDF — 쪽 하나를 원하는 크기의 [`Frame`] 으로 그린다 (흰 바탕, 쪽 회전 반영).
//! 썸네일·미리보기는 [`PdfDecoder`](첫 쪽), 전체 보기의 쪽 넘기기·확대 다시 그리기는 [`render_page`].

use std::path::Path;

pub const EXTENSIONS: &[&str] = &["pdf"];

/// "원본 크기"(1:1) = 72dpi 쪽 크기 × 이 값 (288dpi) — 화면에 맞추거나 확대할 때 이 해상도까지 다시 그린다
pub const DPI_SCALE: f64 = 4.0;

/// 쪽 수 (못 열면 None)
pub fn page_count(path: &Path) -> Option<usize> {
    imp::page_count(path)
}

/// `index`(0부터) 쪽을 긴 변 `max_side` 로. 반환 프레임의 `full_size` = 쪽 크기 × [`DPI_SCALE`] (회전 반영).
pub fn render_page(path: &Path, index: usize, max_side: u32) -> std::result::Result<iv_core::Frame, String> {
    imp::render_page(path, index, max_side)
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_void;
    use std::path::Path;

    use iv_core::Frame;
    use objc2_core_foundation::{CFRetained, CFURL, CGAffineTransform, CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGContext, CGImageAlphaInfo, CGPDFBox, CGPDFDocument, CGPDFPage, kCGColorSpaceSRGB};

    fn open(path: &Path) -> Option<CFRetained<CGPDFDocument>> {
        let url = CFURL::from_file_path(path)?;
        CGPDFDocument::with_url(Some(&url))
    }

    pub fn page_count(path: &Path) -> Option<usize> {
        open(path).map(|d| CGPDFDocument::number_of_pages(Some(&d)))
    }

    pub fn render_page(path: &Path, index: usize, max_side: u32) -> Result<Frame, String> {
        let doc = open(path).ok_or("PDF 를 열 수 없음 (손상되었거나 암호가 걸림)")?;
        let n = CGPDFDocument::number_of_pages(Some(&doc));
        if n == 0 {
            return Err("빈 PDF".into());
        }
        let page = CGPDFDocument::page(Some(&doc), index.min(n - 1) + 1).ok_or("쪽을 읽지 못함")?;
        let b = CGPDFPage::box_rect(Some(&page), CGPDFBox::CropBox);
        let rot = CGPDFPage::rotation_angle(Some(&page)).rem_euclid(360);
        let (bw, bh) = (b.size.width.max(1.0), b.size.height.max(1.0));
        // 화면에 보이는 쪽 크기 (90°/270° 면 가로·세로 바뀜)
        let (dw, dh) = if rot % 180 == 90 { (bh, bw) } else { (bw, bh) };
        let s = max_side.max(16) as f64 / dw.max(dh);
        let (w, h) = ((dw * s).round().max(1.0) as usize, (dh * s).round().max(1.0) as usize);
        //#region 비트맵에 그리기 — 흰 바탕 + 쪽 (회전·자르기 상자 반영 행렬)
        let stride = w * 4;
        let mut buf = vec![0u8; stride * h];
        let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).ok_or("sRGB 색공간 실패")?;
        let ctx = unsafe { CGBitmapContextCreate(buf.as_mut_ptr() as *mut c_void, w, h, 8, stride, Some(&space), CGImageAlphaInfo::PremultipliedLast.0) }.ok_or("비트맵 실패")?;
        CGContext::set_rgb_fill_color(Some(&ctx), 1.0, 1.0, 1.0, 1.0);
        CGContext::fill_rect(Some(&ctx), CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64)));
        let (bx, by) = (b.origin.x, b.origin.y);
        // (x, y) → (a x + c y + tx, b x + d y + ty) — /Rotate 는 시계 방향
        let t = match rot {
            90 => CGAffineTransform { a: 0.0, b: -s, c: s, d: 0.0, tx: -s * by, ty: s * (bw + bx) },
            180 => CGAffineTransform { a: -s, b: 0.0, c: 0.0, d: -s, tx: s * (bw + bx), ty: s * (bh + by) },
            270 => CGAffineTransform { a: 0.0, b: s, c: -s, d: 0.0, tx: s * (bh + by), ty: -s * bx },
            _ => CGAffineTransform { a: s, b: 0.0, c: 0.0, d: s, tx: -s * bx, ty: -s * by },
        };
        CGContext::concat_ctm(Some(&ctx), t);
        CGContext::draw_pdf_page(Some(&ctx), Some(&page));
        drop(ctx);
        //#endregion
        // 불투명 흰 바탕이라 알파는 255 — 그대로 RGBA
        Ok(Frame::rgba8(w as u32, h as u32, buf, [(dw * crate::DPI_SCALE).round() as u32, (dh * crate::DPI_SCALE).round() as u32]))
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::Path;

    pub fn page_count(_path: &Path) -> Option<usize> {
        None
    }

    pub fn render_page(_path: &Path, _index: usize, _max_side: u32) -> Result<iv_core::Frame, String> {
        Err("이 OS 에서는 아직 PDF 를 그리지 않음".into())
    }
}

//#region 디코더 — 첫 쪽 (썸네일·미리보기)
use iv_core::{CancelToken, DecodeError, Decoder, Frame, ImageInfo, ImageSource, Result, SizeHint};

pub struct PdfDecoder;

impl Decoder for PdfDecoder {
    fn name(&self) -> &'static str {
        "PDF"
    }

    fn extensions(&self) -> &'static [&'static str] {
        EXTENSIONS
    }

    fn probe(&self, src: &ImageSource, head: &[u8]) -> Option<i32> {
        let by_ext = src.extension().is_some_and(|e| e == "pdf");
        (by_ext || head.starts_with(b"%PDF")).then_some(30)
    }

    fn info(&self, src: &ImageSource) -> Result<ImageInfo> {
        let p = src.path().ok_or_else(|| DecodeError::Unsupported("경로 아님".into()))?;
        let f = render_page(p, 0, 64).map_err(DecodeError::InvalidData)?;
        Ok(ImageInfo { size: f.full_size, frame_count: page_count(p).unwrap_or(1) as u32, format_name: "PDF".into(), ..Default::default() })
    }

    fn decode(&self, src: &ImageSource, hint: SizeHint, cancel: &CancelToken) -> Result<Frame> {
        let p = src.path().ok_or_else(|| DecodeError::Unsupported("경로 아님".into()))?;
        cancel.check()?;
        render_page(p, 0, hint.max_side.unwrap_or(2048).min(4096)).map_err(DecodeError::InvalidData)
    }
}
//#endregion

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// 시험 PDF 는 macOS 의 cupsfilter 대신 직접 짧은 PDF 를 만든다
    fn make_pdf(rotate: i32) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("iv-pdf-{rotate}-{}.pdf", std::process::id()));
        // 가로 200 × 세로 100 쪽, 왼쪽 절반을 검게 칠함
        let content = "0 0 0 rg 0 0 100 100 re f";
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Rotate {rotate} /Contents 4 0 R >>"),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        ];
        let mut out = String::from("%PDF-1.4\n");
        let mut offs = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offs.push(out.len());
            out += &format!("{} 0 obj\n{o}\nendobj\n", i + 1);
        }
        let xref = out.len();
        out += &format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1);
        for o in offs {
            out += &format!("{o:010} 00000 n \n");
        }
        out += &format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1);
        std::fs::write(&p, out).unwrap();
        p
    }

    fn px(f: &Frame, x: u32, y: u32) -> [u8; 4] {
        let i = (y * f.width + x) as usize * 4;
        f.pixels[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn renders_pages_with_rotation() {
        let p = make_pdf(0);
        assert_eq!(page_count(&p), Some(1));
        let f = render_page(&p, 0, 400).unwrap();
        assert_eq!((f.width, f.height), (400, 200));
        assert_eq!(f.full_size, [800, 400]);
        assert_eq!(px(&f, 10, 100)[..3], [0, 0, 0], "왼쪽 검정");
        assert_eq!(px(&f, 390, 100)[..3], [255, 255, 255], "오른쪽 흰색");
        // 90° 회전 쪽: 세로로 서고, 검은 절반이 위로 (시계 방향)
        let p90 = make_pdf(90);
        let f = render_page(&p90, 0, 400).unwrap();
        assert_eq!((f.width, f.height), (200, 400));
        assert_eq!(px(&f, 100, 10)[..3], [0, 0, 0], "위쪽 검정");
        assert_eq!(px(&f, 100, 390)[..3], [255, 255, 255], "아래쪽 흰색");
        let _ = std::fs::remove_file(p);
        let _ = std::fs::remove_file(p90);
    }
}
