//! 렌더러 생성과 자동 폴백: GPU → 폴백 어댑터(WARP/llvmpipe) → CPU

use std::sync::Arc;

use iv_render::Renderer;
use iv_render_wgpu::{AdapterMode, WgpuRenderer};
use winit::window::Window;

use crate::args::RendererLevel;

/// `start` 단계부터 시도해 처음 성공한 렌더러와 그 단계를 돌려준다. CPU 는 사실상 항상 성공.
pub fn create(window: &Arc<Window>, start: RendererLevel) -> (Box<dyn Renderer>, RendererLevel) {
    let size = window.inner_size();
    let size = [size.width.max(1), size.height.max(1)];
    for level in [RendererLevel::Gpu, RendererLevel::Fallback, RendererLevel::Cpu] {
        if level < start {
            continue;
        }
        let r: Result<Box<dyn Renderer>, String> = match level {
            RendererLevel::Gpu => WgpuRenderer::new_surface(window.clone(), size, AdapterMode::Hardware).map(|r| Box::new(r) as _),
            RendererLevel::Fallback => WgpuRenderer::new_surface(window.clone(), size, AdapterMode::Fallback).map(|r| Box::new(r) as _),
            RendererLevel::Cpu => iv_render_cpu::CpuRenderer::new_surface(window.clone(), size).map(|r| Box::new(r) as _),
        };
        match r {
            Ok(r) => {
                log::info!("렌더러 선택: {} ({level:?})", r.name());
                return (r, level);
            }
            Err(e) => log::warn!("{level:?} 렌더러 실패 → 다음 단계: {e}"),
        }
    }
    panic!("CPU 렌더러까지 모두 실패했습니다");
}

/// 다음 단계 (장치 손실 시)
pub fn next_level(l: RendererLevel) -> Option<RendererLevel> {
    match l {
        RendererLevel::Gpu => Some(RendererLevel::Fallback),
        RendererLevel::Fallback => Some(RendererLevel::Cpu),
        RendererLevel::Cpu => None,
    }
}
