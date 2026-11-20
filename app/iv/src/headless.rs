//! --render-to: 창 없이 한 장을 그려 PNG 로 저장 (방향·색·렌더러 시험용)

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use iv_core::{CancelToken, DecoderRegistry, ImageSource, SizeHint};
use iv_render::{Renderer, Scene};
use iv_render_wgpu::{AdapterMode, WgpuRenderer};
use iv_viewport::Viewport;

use crate::args::{Args, RendererLevel};

pub fn run(args: &Args, registry: &DecoderRegistry, out: &Path) -> i32 {
    let Some(path) = &args.path else {
        eprintln!("--render-to 에는 이미지 경로가 필요합니다");
        return 2;
    };
    let src = ImageSource::Path(path.clone());
    let side = args.size[0].max(args.size[1]);
    let hint = if args.zoom.is_some() { SizeHint::FULL } else { SizeHint::max_side(side) };
    let t = Instant::now();
    let frame = match registry.decode(&src, hint, &CancelToken::new()) {
        Ok(f) => Arc::new(f),
        Err(e) => {
            eprintln!("디코드 실패: {e}");
            return 1;
        }
    };
    let decode_ms = t.elapsed().as_secs_f64() * 1000.0;

    let mut renderer: Box<dyn Renderer> = match create(args.renderer, args.size) {
        Some(r) => r,
        None => return 1,
    };
    let mut vp = Viewport::new([args.size[0] as f32, args.size[1] as f32]);
    vp.set_image(frame.display_full_size());
    if let Some(z) = args.zoom {
        vp.zoom_to(z, [args.size[0] as f32 / 2.0, args.size[1] as f32 / 2.0]);
    }
    let t = Instant::now();
    let scene = Scene { viewport: &vp, image: Some((1, &frame)), image_area: None, background: [30, 30, 32], ui: None };
    let Some((w, h, px)) = renderer.capture(scene) else {
        eprintln!("캡처 실패");
        return 1;
    };
    let render_ms = t.elapsed().as_secs_f64() * 1000.0;
    if let Err(e) = image::save_buffer(out, &px, w, h, image::ExtendedColorType::Rgba8) {
        eprintln!("PNG 저장 실패: {e}");
        return 1;
    }
    println!(
        "{} → {} | 디코드 {:.1}ms ({}x{}, 방향 {:?}) | 렌더 {:.1}ms | 렌더러 {}",
        path.display(),
        out.display(),
        decode_ms,
        frame.width,
        frame.height,
        frame.orientation,
        render_ms,
        renderer.name()
    );
    0
}

fn create(start: RendererLevel, size: [u32; 2]) -> Option<Box<dyn Renderer>> {
    for level in [RendererLevel::Gpu, RendererLevel::Fallback, RendererLevel::Cpu] {
        if level < start {
            continue;
        }
        let r: Result<Box<dyn Renderer>, String> = match level {
            RendererLevel::Gpu => WgpuRenderer::new_offscreen(size, AdapterMode::Hardware).map(|r| Box::new(r) as _),
            RendererLevel::Fallback => WgpuRenderer::new_offscreen(size, AdapterMode::Fallback).map(|r| Box::new(r) as _),
            RendererLevel::Cpu => Ok(Box::new(iv_render_cpu::offscreen(size))),
        };
        match r {
            Ok(r) => return Some(r),
            Err(e) => log::warn!("{level:?} 실패 → 다음 단계: {e}"),
        }
    }
    None
}
