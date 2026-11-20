//! egui 소프트웨어 래스터라이저 — CPU 렌더러에서도 UI(브라우저 화면)를 그린다.
//!
//! egui-wgpu 0.37 의 비-sRGB 표면 경로(`fs_main_gamma_framebuffer`)와 같은 결과를 목표로 한다:
//! - 정점색·텍스처 모두 **감마 공간 프리멀티플라이드** 값, 출력 = 정점색 × 텍스처
//! - 블렌드 = `src + dst * (1 - src.a)` (감마 공간)
//! - 텍스처 샘플 = `uv*size - 0.5` 쌍선형, 가장자리 고정 (Nearest 옵션이면 최근접)
//! - 클립 = `round(clip * ppp)` 를 화면 안으로
//! - 픽셀 중심 판정 + top-left 규칙 (페더링 가장자리가 두 번 칠해지지 않게)
//! dithering 은 생략 (GPU 와 ±1~2 차이 허용).

use std::collections::HashMap;

use egui::epaint::{ClippedPrimitive, ImageData, Primitive, TextureId, Vertex, textures::TexturesDelta};
use rayon::prelude::*;

struct Tex {
    w: usize,
    h: usize,
    /// 프리멀티플라이드 RGBA
    px: Vec<[u8; 4]>,
    nearest: bool,
}

#[derive(Default)]
pub struct EguiRaster {
    textures: HashMap<TextureId, Tex>,
}

/// 행 띠 높이 (병렬 단위)
const BAND: usize = 32;

impl EguiRaster {
    /// 텍스처 생성·부분 갱신 (그리기 전에)
    pub fn apply_set(&mut self, delta: &TexturesDelta) {
        for (id, deltas) in &delta.set {
            for d in deltas {
                let ImageData::Color(img) = &d.image;
                let [w, h] = img.size;
                let nearest = d.options.magnification == egui::TextureFilter::Nearest;
                match d.pos {
                    None => {
                        let px = img.pixels.iter().map(|c| c.to_array()).collect();
                        self.textures.insert(*id, Tex { w, h, px, nearest });
                    }
                    Some([x0, y0]) => {
                        let Some(t) = self.textures.get_mut(id) else {
                            log::warn!("CPU egui: 없는 텍스처 부분 갱신 {id:?}");
                            continue;
                        };
                        for y in 0..h {
                            for x in 0..w {
                                if x0 + x < t.w && y0 + y < t.h {
                                    t.px[(y0 + y) * t.w + x0 + x] = img.pixels[y * w + x].to_array();
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// 텍스처 해제 (그린 뒤에)
    pub fn apply_free(&mut self, delta: &TexturesDelta) {
        for id in &delta.free {
            self.textures.remove(id);
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    /// 0x00RRGGBB 버퍼 위에 egui 메시를 합성한다
    pub fn paint(&self, prims: &[ClippedPrimitive], ppp: f32, buf: &mut [u32], width: usize, height: usize) {
        // 미리 화면 좌표로 바꾼 삼각형 목록 (클립 사각형 포함)
        struct Tri<'a> {
            v: [Vertex; 3],
            clip: [i32; 4],
            bbox: [i32; 4],
            tex: Option<&'a Tex>,
            uv_const: bool,
        }
        let mut tris: Vec<Tri<'_>> = Vec::new();
        for p in prims {
            let Primitive::Mesh(mesh) = &p.primitive else { continue };
            let c = p.clip_rect;
            let clip = [
                ((c.min.x * ppp).round() as i32).clamp(0, width as i32),
                ((c.min.y * ppp).round() as i32).clamp(0, height as i32),
                ((c.max.x * ppp).round() as i32).clamp(0, width as i32),
                ((c.max.y * ppp).round() as i32).clamp(0, height as i32),
            ];
            if clip[2] <= clip[0] || clip[3] <= clip[1] {
                continue;
            }
            let tex = self.textures.get(&mesh.texture_id);
            for idx in mesh.indices.chunks_exact(3) {
                let mut v = [mesh.vertices[idx[0] as usize], mesh.vertices[idx[1] as usize], mesh.vertices[idx[2] as usize]];
                for vv in &mut v {
                    vv.pos.x *= ppp;
                    vv.pos.y *= ppp;
                }
                let minx = v.iter().map(|q| q.pos.x).fold(f32::MAX, f32::min).floor() as i32;
                let maxx = v.iter().map(|q| q.pos.x).fold(f32::MIN, f32::max).ceil() as i32;
                let miny = v.iter().map(|q| q.pos.y).fold(f32::MAX, f32::min).floor() as i32;
                let maxy = v.iter().map(|q| q.pos.y).fold(f32::MIN, f32::max).ceil() as i32;
                let bbox = [minx.max(clip[0]), miny.max(clip[1]), maxx.min(clip[2]), maxy.min(clip[3])];
                if bbox[2] <= bbox[0] || bbox[3] <= bbox[1] {
                    continue;
                }
                let uv_const = v[0].uv == v[1].uv && v[1].uv == v[2].uv;
                tris.push(Tri { v, clip, bbox, tex, uv_const });
            }
        }
        if tris.is_empty() {
            return;
        }
        buf.par_chunks_mut(width * BAND).enumerate().for_each(|(band, rows)| {
            let y0 = (band * BAND) as i32;
            let y1 = y0 + (rows.len() / width) as i32;
            for t in &tris {
                if t.bbox[3] <= y0 || t.bbox[1] >= y1 {
                    continue;
                }
                raster_tri(t.v, t.tex, t.uv_const, [t.bbox[0], t.bbox[1].max(y0), t.bbox[2], t.bbox[3].min(y1)], t.clip, rows, width, y0);
            }
        });
    }
}

/// 변 함수 (b 가 a→c 의 어느 쪽인가 ×2 넓이)
#[inline]
fn edge(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (bx - ax) * (py - ay) - (by - ay) * (px - ax)
}

#[allow(clippy::too_many_arguments)]
fn raster_tri(v: [Vertex; 3], tex: Option<&Tex>, uv_const: bool, bb: [i32; 4], _clip: [i32; 4], rows: &mut [u32], width: usize, y0: i32) {
    let (p0, p1, p2) = (v[0].pos, v[1].pos, v[2].pos);
    let area = edge(p0.x, p0.y, p1.x, p1.y, p2.x, p2.y);
    if area.abs() < 1e-8 {
        return;
    }
    // 감김 방향과 무관하게: 넓이가 음수면 부호를 뒤집어 "안쪽 = 양수"로
    let sign = area.signum();
    let inv_area = 1.0 / area.abs();
    // top-left 규칙: 경계에 정확히 걸친 픽셀은 위쪽·왼쪽 변일 때만 포함
    let top_left = |a: egui::Pos2, b: egui::Pos2| {
        let (dx, dy) = ((b.x - a.x) * sign, (b.y - a.y) * sign);
        (dy == 0.0 && dx < 0.0) || dy > 0.0
    };
    let tl = [top_left(p1, p2), top_left(p2, p0), top_left(p0, p1)];
    let col = |c: egui::Color32| [c.r() as f32, c.g() as f32, c.b() as f32, c.a() as f32];
    let c = [col(v[0].color), col(v[1].color), col(v[2].color)];
    let const_tex = if uv_const { tex.map(|t| sample(t, v[0].uv.x, v[0].uv.y)) } else { None };
    let solid = c[0] == c[1] && c[1] == c[2];

    for y in bb[1]..bb[3] {
        let py = y as f32 + 0.5;
        let row = &mut rows[(y - y0) as usize * width..][..width];
        for x in bb[0]..bb[2] {
            let px = x as f32 + 0.5;
            let w0 = edge(p1.x, p1.y, p2.x, p2.y, px, py) * sign;
            let w1 = edge(p2.x, p2.y, p0.x, p0.y, px, py) * sign;
            let w2 = edge(p0.x, p0.y, p1.x, p1.y, px, py) * sign;
            let inside = |w: f32, tl: bool| w > 0.0 || (w == 0.0 && tl);
            if !(inside(w0, tl[0]) && inside(w1, tl[1]) && inside(w2, tl[2])) {
                continue;
            }
            let (b0, b1, b2) = (w0 * inv_area, w1 * inv_area, w2 * inv_area);
            let vc = if solid { c[0] } else { std::array::from_fn(|i| c[0][i] * b0 + c[1][i] * b1 + c[2][i] * b2) };
            let tc = match (const_tex, tex) {
                (Some(t), _) => t,
                (None, Some(t)) => {
                    let u = v[0].uv.x * b0 + v[1].uv.x * b1 + v[2].uv.x * b2;
                    let vv = v[0].uv.y * b0 + v[1].uv.y * b1 + v[2].uv.y * b2;
                    sample(t, u, vv)
                }
                (None, None) => [255.0; 4],
            };
            // 출력 = 정점색 × 텍스처 (둘 다 프리멀티플라이드, 0..255)
            let s: [f32; 4] = std::array::from_fn(|i| vc[i] * tc[i] / 255.0);
            if s[3] <= 0.0 && s[0] <= 0.0 && s[1] <= 0.0 && s[2] <= 0.0 {
                continue;
            }
            let d = row[x as usize];
            let inv = 1.0 - s[3] / 255.0;
            let mix = |sv: f32, dv: u32| (sv + dv as f32 * inv).round().clamp(0.0, 255.0) as u32;
            row[x as usize] = mix(s[0], (d >> 16) & 255) << 16 | mix(s[1], (d >> 8) & 255) << 8 | mix(s[2], d & 255);
        }
    }
}

/// 프리멀티플라이드 텍셀 샘플 (0..255)
#[inline]
fn sample(t: &Tex, u: f32, v: f32) -> [f32; 4] {
    let texel = |x: i32, y: i32| {
        let x = x.clamp(0, t.w as i32 - 1) as usize;
        let y = y.clamp(0, t.h as i32 - 1) as usize;
        let p = t.px[y * t.w + x];
        [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
    };
    let fx = u * t.w as f32 - 0.5;
    let fy = v * t.h as f32 - 0.5;
    if t.nearest {
        return texel((fx + 0.5).floor() as i32, (fy + 0.5).floor() as i32);
    }
    let (x0, y0) = (fx.floor(), fy.floor());
    let (ax, ay) = (fx - x0, fy - y0);
    let (x0, y0) = (x0 as i32, y0 as i32);
    let (a, b, c, d) = (texel(x0, y0), texel(x0 + 1, y0), texel(x0, y0 + 1), texel(x0 + 1, y0 + 1));
    std::array::from_fn(|i| {
        let top = a[i] + (b[i] - a[i]) * ax;
        let bot = c[i] + (d[i] - c[i]) * ax;
        top + (bot - top) * ay
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::{Mesh, Rect, Color32, Pos2};

    fn quad(rect: Rect, color: Color32) -> ClippedPrimitive {
        let mut m = Mesh::default();
        m.add_colored_rect(rect, color);
        ClippedPrimitive { clip_rect: Rect::EVERYTHING, primitive: Primitive::Mesh(m) }
    }

    #[test]
    fn solid_rect_blend_and_clip() {
        let r = EguiRaster::default();
        let (w, h) = (40usize, 70usize); // 띠 경계(32)를 가로지르게
        let mut buf = vec![0x00_10_20_30u32; w * h];
        // 불투명 빨강 사각형 (점 → ppp 2 → 물리 px 10..30, 10..60)
        r.paint(&[quad(Rect::from_min_max(Pos2::new(5.0, 5.0), Pos2::new(15.0, 30.0)), Color32::RED)], 2.0, &mut buf, w, h);
        assert_eq!(buf[20 * w + 20], 0xFF0000);
        assert_eq!(buf[59 * w + 29], 0xFF0000);
        assert_eq!(buf[60 * w + 29], 0x102030, "아래 변은 포함하지 않음");
        assert_eq!(buf[20 * w + 30], 0x102030, "오른쪽 변은 포함하지 않음");
        assert_eq!(buf[9 * w + 20], 0x102030);
        // 공유 변 이중 블렌드 없음: 반투명 사각형 하나(삼각형 2개)의 대각선 위 픽셀도 한 번만
        let mut buf2 = vec![0u32; w * h];
        let half = Color32::from_rgba_premultiplied(100, 0, 0, 128);
        r.paint(&[quad(Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(20.0, 35.0)), half)], 1.0, &mut buf2, w, h);
        let vals: std::collections::HashSet<u32> = (0..35).flat_map(|y| (0..20).map(move |x| (x, y))).map(|(x, y)| buf2[y * w + x]).collect();
        assert_eq!(vals.len(), 1, "대각선에서 두 번 칠해진 픽셀이 있음: {vals:?}");
        // 클립 사각형
        let mut m = Mesh::default();
        m.add_colored_rect(Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(40.0, 70.0)), Color32::WHITE);
        let mut buf3 = vec![0u32; w * h];
        r.paint(&[ClippedPrimitive { clip_rect: Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(20.0, 20.0)), primitive: Primitive::Mesh(m) }], 1.0, &mut buf3, w, h);
        assert_eq!(buf3.iter().filter(|p| **p == 0xFFFFFF).count(), 100);
    }

    #[test]
    fn textured_quad_and_partial_update() {
        let mut r = EguiRaster::default();
        let id = TextureId::Managed(7);
        let img = egui::ColorImage::new([2, 2], vec![Color32::RED, Color32::GREEN, Color32::BLUE, Color32::WHITE]);
        let mut delta = TexturesDelta::default();
        delta.set.insert(id, std::iter::once(egui::epaint::ImageDelta::full(img, egui::TextureOptions::NEAREST)).collect());
        r.apply_set(&delta);
        delta.clear();
        let mut m = Mesh::with_texture(id);
        m.add_rect_with_uv(Rect::from_min_max(Pos2::ZERO, Pos2::new(4.0, 4.0)), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        let prim = ClippedPrimitive { clip_rect: Rect::EVERYTHING, primitive: Primitive::Mesh(m) };
        let mut buf = vec![0u32; 16];
        r.paint(std::slice::from_ref(&prim), 1.0, &mut buf, 4, 4);
        assert_eq!(buf[0], 0xFF0000);
        assert_eq!(buf[3], 0x00FF00);
        assert_eq!(buf[12], 0x0000FF);
        assert_eq!(buf[15], 0xFFFFFF);
        // 부분 갱신: (1,1) 을 검정으로
        let patch = egui::ColorImage::new([1, 1], vec![Color32::BLACK]);
        delta.set.insert(id, std::iter::once(egui::epaint::ImageDelta::partial([1, 1], patch, egui::TextureOptions::NEAREST)).collect());
        r.apply_set(&delta);
        delta.clear();
        let mut buf = vec![0u32; 16];
        r.paint(std::slice::from_ref(&prim), 1.0, &mut buf, 4, 4);
        assert_eq!(buf[15], 0x000000);
        assert_eq!(buf[0], 0xFF0000);
        // 해제
        delta.free.insert(id);
        r.apply_free(&delta);
        delta.clear();
        assert_eq!(r.texture_count(), 0);
    }
}
