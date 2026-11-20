//! 렌더러 확장 지점. 앱은 이 trait 만 알고, GPU(wgpu)·CPU(softbuffer) 구현을 갈아 끼운다.

use std::sync::Arc;

use iv_core::Frame;
use iv_viewport::Viewport;

/// 이 렌더러가 그릴 수 있는 UI 수준
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiCapability {
    /// egui 오버레이 전체
    Full,
    /// UI 를 그리지 못함 — 앱이 상태를 창 제목 등으로 표시
    StatusOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderOutcome {
    Ok,
    /// 이번 프레임 건너뜀 (창 가려짐·타임아웃) — 다음 이벤트에 다시
    Skipped,
    /// 표면 재구성 후 다시 그려야 함
    Reconfigure,
    /// 장치 손실 — 앱이 다음 단계 렌더러로 폴백
    DeviceLost(String),
}

/// egui 가 만든 그리기 목록 (Full UI 렌더러만 사용)
pub struct EguiPaint {
    pub primitives: Vec<egui::ClippedPrimitive>,
    pub textures_delta: egui::TexturesDelta,
    pub pixels_per_point: f32,
}

/// 한 프레임에 그릴 내용
pub struct Scene<'a> {
    pub viewport: &'a Viewport,
    /// (식별 키, 프레임). 키가 같고 프레임 포인터가 같으면 재업로드하지 않는다.
    pub image: Option<(u64, &'a Arc<Frame>)>,
    /// 이미지를 그릴 영역 (물리 px `[x0, y0, x1, y1]`). None = 화면 전체.
    /// `viewport.screen` 은 이 영역의 크기와 같아야 한다 (브라우저 미리보기 칸).
    pub image_area: Option<[f32; 4]>,
    /// 배경색 (sRGB)
    pub background: [u8; 3],
    pub ui: Option<EguiPaint>,
}

pub trait Renderer {
    /// 사람이 읽는 이름 — "Metal (Apple M3)", "WARP", "CPU"
    fn name(&self) -> &str;
    fn ui_capability(&self) -> UiCapability;
    /// 한 번에 올릴 수 있는 텍스처 최대 변
    fn max_texture_side(&self) -> u32;
    /// 창 크기 변경 (물리 픽셀)
    fn resize(&mut self, size: [u32; 2]);
    fn render(&mut self, scene: Scene<'_>) -> RenderOutcome;
    /// 오프스크린으로 그려 RGBA8(sRGB) 픽셀을 돌려준다 (--render-to·시험용)
    fn capture(&mut self, scene: Scene<'_>) -> Option<(u32, u32, Vec<u8>)>;
}

/// 표시 크기 기준 이미지가 화면 사각형 어디에 오는지 + 텍스처 확대율 (렌더러 공통 계산)
pub fn texture_magnification(viewport: &Viewport, frame: &Frame) -> f32 {
    // 화면 픽셀 / 텍스처 픽셀
    let display = frame.orientation.display_size([frame.width, frame.height]);
    let full = frame.display_full_size();
    viewport.scale * full[0] as f32 / display[0].max(1) as f32
}

/// 이미지 영역을 타깃 안 정수 사각형 `[x0, y0, x1, y1]` 로. 비면 None (그리지 않음).
/// 한 프레임 늦은 레이아웃 값이 창 밖으로 나가도 scissor 오류(→ 장치 오류)가 나지 않게 반드시 거친다.
pub fn clamp_area(area: Option<[f32; 4]>, size: [u32; 2]) -> Option<[u32; 4]> {
    let Some([x0, y0, x1, y1]) = area else { return Some([0, 0, size[0], size[1]]) };
    let cx = |v: f32| (v.round().max(0.0) as u32).min(size[0]);
    let cy = |v: f32| (v.round().max(0.0) as u32).min(size[1]);
    let r = [cx(x0), cy(y0), cx(x1), cy(y1)];
    (r[2] > r[0] && r[3] > r[1]).then_some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clamp_area_bounds() {
        assert_eq!(clamp_area(None, [100, 50]), Some([0, 0, 100, 50]));
        assert_eq!(clamp_area(Some([-5.0, 10.0, 300.0, 40.0]), [100, 50]), Some([0, 10, 100, 40]));
        assert_eq!(clamp_area(Some([120.0, 10.0, 300.0, 40.0]), [100, 50]), None);
        assert_eq!(clamp_area(Some([0.0, 0.0, 0.0, 0.0]), [100, 50]), None);
    }
}
