//! CPU 렌더러 — 최종 비상구.
//!
//! 화면 크기 버퍼에 직접 합성한다. 축소가 크면 먼저 SIMD 리샘플로 화면 배율에 맞춘 사본을 만들어
//! (줌 단계마다 1회) 계단 현상을 막고, 이후 이동은 쌍선형 샘플만 한다. 행 단위 병렬(rayon).
//! UI(egui) 는 [`egui_raster`] 소프트웨어 래스터라이저로 그 위에 합성한다 (`UiCapability::Full`).

mod egui_raster;

use std::num::NonZeroU32;
use std::sync::Arc;

use iv_core::Frame;
use iv_render::{EguiPaint, RenderOutcome, Renderer, Scene, UiCapability, texture_magnification};
use iv_viewport::Viewport;
use rayon::prelude::*;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

/// 축소 사본 캐시 (원본 프레임 + 목표 크기)
struct Scaled {
    src_ptr: *const u8,
    frame: Frame,
}

pub struct CpuRenderer<W: HasDisplayHandle + HasWindowHandle> {
    surface: Option<softbuffer::Surface<W, W>>,
    size: [u32; 2],
    scaled: Option<Scaled>,
    ui: egui_raster::EguiRaster,
}

impl<W: HasDisplayHandle + HasWindowHandle + Clone> CpuRenderer<W> {
    pub fn new_surface(window: W, size: [u32; 2]) -> Result<Self, String> {
        let ctx = softbuffer::Context::new(window.clone()).map_err(|e| format!("softbuffer 컨텍스트: {e}"))?;
        let surface = softbuffer::Surface::new(&ctx, window).map_err(|e| format!("softbuffer 표면: {e}"))?;
        let mut r = Self { surface: Some(surface), size: [1, 1], scaled: None, ui: Default::default() };
        r.resize(size);
        log::info!("렌더러: CPU (softbuffer)");
        Ok(r)
    }
}

/// 창 없는 CPU 렌더러 (--render-to·시험). 창 핸들 타입이 필요 없도록 별도 생성자.
pub fn offscreen(size: [u32; 2]) -> CpuRenderer<NoWindow> {
    CpuRenderer { surface: None, size, scaled: None, ui: Default::default() }
}

/// 오프스크린용 자리표시 타입
#[derive(Clone)]
pub struct NoWindow;
impl HasWindowHandle for NoWindow {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        Err(raw_window_handle::HandleError::NotSupported)
    }
}
impl HasDisplayHandle for NoWindow {
    fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Err(raw_window_handle::HandleError::NotSupported)
    }
}

impl<W: HasDisplayHandle + HasWindowHandle> CpuRenderer<W> {
    /// 화면 배율에 맞는 소스 프레임 (필요하면 축소 사본 생성)
    fn source<'a>(&'a mut self, viewport: &Viewport, frame: &'a Arc<Frame>) -> &'a Frame {
        let mag = texture_magnification(viewport, frame);
        if mag >= 0.75 {
            return frame;
        }
        let target = ((frame.max_side() as f32 * mag).ceil() as u32).max(1);
        let ptr = frame.pixels.as_ptr();
        let reuse = self.scaled.as_ref().is_some_and(|s| s.src_ptr == ptr && s.frame.max_side() == target);
        if reuse {
            // 같은 픽셀에 방향만 바뀌었을 수 있다 (사용자 회전)
            self.scaled.as_mut().unwrap().frame.orientation = frame.orientation;
        } else {
            let t = std::time::Instant::now();
            let f = iv_imageops::downscale_rgba8(frame, target);
            log::debug!("CPU 축소 사본 {}x{} {:.1}ms", f.width, f.height, t.elapsed().as_secs_f64() * 1000.0);
            self.scaled = Some(Scaled { src_ptr: ptr, frame: f });
        }
        &self.scaled.as_ref().unwrap().frame
    }

    /// 이미지 + UI 를 버퍼에 합성. 텍스처 delta 는 set 을 먼저, free 를 나중에 (wgpu 와 같은 순서).
    fn compose_all(&mut self, mut scene: Scene<'_>, out: Option<&mut [u32]>) {
        let ui = scene.ui.take();
        if let Some(p) = &ui {
            self.ui.apply_set(&p.textures_delta);
        }
        if let Some(out) = out {
            self.compose(&scene, out);
            if let Some(EguiPaint { primitives, pixels_per_point, .. }) = &ui {
                let t = std::time::Instant::now();
                self.ui.paint(primitives, *pixels_per_point, out, self.size[0] as usize, self.size[1] as usize);
                log::debug!("CPU UI {}개 {:.1}ms", primitives.len(), t.elapsed().as_secs_f64() * 1000.0);
            }
        }
        if let Some(mut p) = ui {
            self.ui.apply_free(&p.textures_delta);
            p.textures_delta.clear();
        }
    }

    /// 0x00RRGGBB 버퍼에 이미지 합성
    fn compose(&mut self, scene: &Scene<'_>, out: &mut [u32]) {
        let [w, h] = self.size;
        let [br, bg, bb] = scene.background;
        let bgpx = (br as u32) << 16 | (bg as u32) << 8 | bb as u32;
        let area = iv_render::clamp_area(scene.image_area, self.size);
        let (Some((_, frame)), Some(area)) = (scene.image, area) else {
            out.fill(bgpx);
            return;
        };
        let v = scene.viewport;
        let src = self.source(v, frame);
        let mut rect = v.image_rect_on_screen();
        let origin = scene.image_area.map_or([0.0, 0.0], |a| [a[0], a[1]]);
        rect[0] += origin[0];
        rect[2] += origin[0];
        rect[1] += origin[1];
        rect[3] += origin[1];
        let [ax0, ay0, ax1, ay1] = area.map(|v| v as usize);
        let [a, b, c, d, tx, ty] = src.orientation.uv_transform();
        let (sw, sh) = (src.width as f32, src.height as f32);
        let nearest = texture_magnification(v, src) >= 3.0;
        let (rw, rh) = ((rect[2] - rect[0]).max(1e-6), (rect[3] - rect[1]).max(1e-6));
        out.par_chunks_mut(w as usize).enumerate().for_each(|(y, line)| {
            let py = y as f32 + 0.5;
            let vv = (py - rect[1]) / rh;
            if !(0.0..1.0).contains(&vv) || y < ay0 || y >= ay1 {
                line.fill(bgpx);
                return;
            }
            for (x, px) in line.iter_mut().enumerate() {
                let uu = (x as f32 + 0.5 - rect[0]) / rw;
                if !(0.0..1.0).contains(&uu) || x < ax0 || x >= ax1 {
                    *px = bgpx;
                    continue;
                }
                // 표시 UV → 저장 UV (EXIF 방향)
                let su = a * uu + b * vv + tx;
                let sv = c * uu + d * vv + ty;
                let rgba = if nearest { sample_nearest(src, su * sw, sv * sh) } else { sample_bilinear(src, su * sw - 0.5, sv * sh - 0.5) };
                *px = blend(rgba, [br, bg, bb]);
            }
        });
        let _ = h;
    }
}

#[inline]
fn texel(f: &Frame, x: i32, y: i32) -> [f32; 4] {
    let x = x.clamp(0, f.width as i32 - 1) as usize;
    let y = y.clamp(0, f.height as i32 - 1) as usize;
    let i = y * f.stride + x * 4;
    let p = &f.pixels[i..i + 4];
    [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
}

#[inline]
fn sample_nearest(f: &Frame, x: f32, y: f32) -> [f32; 4] {
    texel(f, x.floor() as i32, y.floor() as i32)
}

#[inline]
fn sample_bilinear(f: &Frame, x: f32, y: f32) -> [f32; 4] {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (x0, y0) = (x0 as i32, y0 as i32);
    let p00 = texel(f, x0, y0);
    let p10 = texel(f, x0 + 1, y0);
    let p01 = texel(f, x0, y0 + 1);
    let p11 = texel(f, x0 + 1, y0 + 1);
    let mut o = [0.0; 4];
    for i in 0..4 {
        let top = p00[i] + (p10[i] - p00[i]) * fx;
        let bot = p01[i] + (p11[i] - p01[i]) * fx;
        o[i] = top + (bot - top) * fy;
    }
    o
}

#[inline]
fn blend(c: [f32; 4], bg: [u8; 3]) -> u32 {
    let a = c[3] / 255.0;
    let mix = |s: f32, b: u8| (s * a + b as f32 * (1.0 - a)).round().clamp(0.0, 255.0) as u32;
    mix(c[0], bg[0]) << 16 | mix(c[1], bg[1]) << 8 | mix(c[2], bg[2])
}

impl<W: HasDisplayHandle + HasWindowHandle> Renderer for CpuRenderer<W> {
    fn name(&self) -> &str {
        "CPU"
    }

    fn ui_capability(&self) -> UiCapability {
        UiCapability::Full
    }

    fn max_texture_side(&self) -> u32 {
        // CPU 는 텍스처 한도가 없지만 메모리를 생각해 16K 로 제한
        16384
    }

    fn resize(&mut self, size: [u32; 2]) {
        self.size = [size[0].max(1), size[1].max(1)];
        if let Some(s) = &mut self.surface {
            let (w, h) = (NonZeroU32::new(self.size[0]).unwrap(), NonZeroU32::new(self.size[1]).unwrap());
            if let Err(e) = s.resize(w, h) {
                log::warn!("softbuffer resize 실패: {e}");
            }
        }
    }

    fn render(&mut self, scene: Scene<'_>) -> RenderOutcome {
        let Some(mut surface) = self.surface.take() else {
            // 그리지 못해도 텍스처 변경은 반영 (안 하면 이후 부분 갱신이 깨진다)
            self.compose_all(scene, None);
            return RenderOutcome::Skipped;
        };
        let outcome = match surface.buffer_mut() {
            Ok(mut buf) => {
                let t = std::time::Instant::now();
                self.compose_all(scene, Some(&mut buf));
                log::debug!("CPU 합성 {}x{} {:.1}ms", self.size[0], self.size[1], t.elapsed().as_secs_f64() * 1000.0);
                match buf.present() {
                    Ok(()) => RenderOutcome::Ok,
                    Err(e) => RenderOutcome::DeviceLost(format!("softbuffer present: {e}")),
                }
            }
            Err(e) => {
                self.compose_all(scene, None);
                RenderOutcome::DeviceLost(format!("softbuffer 버퍼: {e}"))
            }
        };
        self.surface = Some(surface);
        outcome
    }

    fn capture(&mut self, scene: Scene<'_>) -> Option<(u32, u32, Vec<u8>)> {
        let [w, h] = self.size;
        let mut buf = vec![0u32; (w * h) as usize];
        self.compose_all(scene, Some(&mut buf));
        let rgba = buf.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]).collect();
        Some((w, h, rgba))
    }
}

