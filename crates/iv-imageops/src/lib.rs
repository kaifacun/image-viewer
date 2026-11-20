//! 프레임 픽셀 연산 (축소 리샘플 등). 디코더·렌더러가 공유한다.

use fast_image_resize::{images::Image, PixelType, ResizeAlg, ResizeOptions, Resizer, FilterType};
use iv_core::Frame;

/// 긴 변이 `max_side` 가 되도록 RGBA8 프레임을 축소 (SIMD, 비율 유지).
/// 다른 디코더 crate 들도 재사용한다.
pub fn downscale_rgba8(src: &Frame, max_side: u32) -> Frame {
    let scale = max_side as f64 / src.max_side() as f64;
    if scale >= 1.0 {
        return src.clone();
    }
    let dw = ((src.width as f64 * scale).round() as u32).max(1);
    let dh = ((src.height as f64 * scale).round() as u32).max(1);
    // stride 가 꽉 차지 않은 경우 대비해 행 단위로 모은다
    let packed: Vec<u8> = if src.stride == src.width as usize * 4 {
        src.pixels.to_vec()
    } else {
        src.pixels.chunks(src.stride).flat_map(|r| r[..src.width as usize * 4].iter().copied()).collect()
    };
    let from = match Image::from_vec_u8(src.width, src.height, packed, PixelType::U8x4) {
        Ok(i) => i,
        Err(_) => return src.clone(),
    };
    let mut to = Image::new(dw, dh, PixelType::U8x4);
    let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    if Resizer::new().resize(&from, &mut to, &opts).is_err() {
        return src.clone();
    }
    let mut out = Frame::rgba8(dw, dh, to.into_vec(), src.full_size);
    out.orientation = src.orientation;
    out.color = src.color.clone();
    out.is_preview = src.is_preview;
    out
}

/// EXIF 방향을 픽셀에 실제로 적용한 사본 (작은 이미지 — 썸네일용). 결과의 orientation 은 Normal.
/// 큰 이미지는 렌더러가 UV 변환으로 처리하므로 이 함수를 쓰지 않는다.
pub fn apply_orientation(src: &Frame) -> Frame {
    use iv_core::Orientation;
    if src.orientation == Orientation::Normal && src.stride == src.width as usize * 4 {
        return src.clone();
    }
    let (w, h) = (src.width as usize, src.height as usize);
    let [dw, dh] = src.orientation.display_size([src.width, src.height]).map(|v| v as usize);
    let [a, b, c, d, tx, ty] = src.orientation.uv_transform();
    let mut out = vec![0u8; dw * dh * 4];
    for y in 0..dh {
        let v = (y as f32 + 0.5) / dh as f32;
        for x in 0..dw {
            let u = (x as f32 + 0.5) / dw as f32;
            let su = a * u + b * v + tx;
            let sv = c * u + d * v + ty;
            let sx = ((su * w as f32) as usize).min(w - 1);
            let sy = ((sv * h as f32) as usize).min(h - 1);
            let s = sy * src.stride + sx * 4;
            out[(y * dw + x) * 4..][..4].copy_from_slice(&src.pixels[s..s + 4]);
        }
    }
    let full = src.orientation.display_size(src.full_size);
    let mut f = Frame::rgba8(dw as u32, dh as u32, out, full);
    f.color = src.color.clone();
    f.is_preview = src.is_preview;
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use iv_core::Orientation;

    #[test]
    fn orientation_rotate90() {
        // 2x1 저장: [빨강, 파랑]. Rotate90(EXIF 6) = 시계 방향 90° 표시 → 1x2, 위=빨강
        let mut f = Frame::rgba8(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255], [2, 1]);
        f.orientation = Orientation::Rotate90;
        let o = apply_orientation(&f);
        assert_eq!((o.width, o.height), (1, 2));
        assert_eq!(&o.pixels[0..4], &[255, 0, 0, 255]);
        assert_eq!(o.full_size, [1, 2]);
        assert_eq!(o.orientation, Orientation::Normal);
    }
}
