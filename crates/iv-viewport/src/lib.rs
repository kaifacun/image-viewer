//! 확대·이동·화면 맞춤 상태와 수학.
//!
//! 좌표계 규칙
//! - "화면 좌표": 창의 물리 픽셀, 원점 = 왼쪽 위.
//! - "이미지 좌표": 원본(full size) 이미지 픽셀, 원점 = 왼쪽 위.
//! - `scale` = 화면 픽셀 / 이미지 픽셀. 축소 디코딩된 텍스처라도 원본 기준으로 계산한다.
//! - `center` = 화면 중앙에 오는 이미지 좌표.

/// 확대 모드 — 창 크기가 바뀌거나 새 이미지가 오면 다시 계산할지 결정한다.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ZoomMode {
    /// 화면에 맞춤 (작은 이미지는 확대하지 않음)
    Fit,
    /// 화면에 맞춤 (작은 이미지도 확대)
    FitAlways,
    /// 사용자가 정한 고정 배율
    Fixed,
}

#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    /// 창 크기 (물리 픽셀)
    pub screen: [f32; 2],
    /// 원본 이미지 크기 (픽셀)
    pub image: [f32; 2],
    pub scale: f32,
    pub center: [f32; 2],
    pub mode: ZoomMode,
}

pub const MIN_SCALE: f32 = 0.01;
pub const MAX_SCALE: f32 = 64.0;

impl Viewport {
    pub fn new(screen: [f32; 2]) -> Self {
        Self { screen, image: [1.0, 1.0], scale: 1.0, center: [0.5, 0.5], mode: ZoomMode::Fit }
    }

    /// 새 이미지를 보여줄 때 — 맞춤 모드로 되돌린다.
    pub fn set_image(&mut self, image: [u32; 2]) {
        self.image = [image[0].max(1) as f32, image[1].max(1) as f32];
        self.mode = ZoomMode::Fit;
        self.refit();
    }

    /// 창 크기가 바뀌었을 때
    pub fn set_screen(&mut self, screen: [f32; 2]) {
        self.screen = [screen[0].max(1.0), screen[1].max(1.0)];
        if self.mode == ZoomMode::Fixed {
            self.clamp_center();
        } else {
            self.refit();
        }
    }

    /// 화면에 꼭 맞는 배율
    pub fn fit_scale(&self) -> f32 {
        (self.screen[0] / self.image[0]).min(self.screen[1] / self.image[1])
    }

    fn refit(&mut self) {
        let fit = self.fit_scale();
        self.scale = match self.mode {
            ZoomMode::Fit => fit.min(1.0),
            ZoomMode::FitAlways => fit,
            ZoomMode::Fixed => self.scale,
        };
        self.center = [self.image[0] / 2.0, self.image[1] / 2.0];
    }

    pub fn fit(&mut self) {
        self.mode = ZoomMode::Fit;
        self.refit();
    }

    pub fn fit_always(&mut self) {
        self.mode = ZoomMode::FitAlways;
        self.refit();
    }

    /// 1:1 (원본 픽셀 = 화면 픽셀), 화면 중앙 기준
    pub fn actual_size(&mut self) {
        let anchor = [self.screen[0] / 2.0, self.screen[1] / 2.0];
        self.zoom_to(1.0, anchor);
    }

    /// `anchor`(화면 좌표) 아래의 이미지 지점이 움직이지 않도록 배율을 바꾼다.
    pub fn zoom_to(&mut self, new_scale: f32, anchor: [f32; 2]) {
        let new_scale = new_scale.clamp(MIN_SCALE, MAX_SCALE);
        let before = self.screen_to_image(anchor);
        self.scale = new_scale;
        self.mode = ZoomMode::Fixed;
        let after = self.screen_to_image(anchor);
        self.center[0] += before[0] - after[0];
        self.center[1] += before[1] - after[1];
        self.clamp_center();
    }

    /// 배율에 곱한다 (휠·핀치·단축키)
    pub fn zoom_by(&mut self, factor: f32, anchor: [f32; 2]) {
        self.zoom_to(self.scale * factor, anchor);
    }

    /// 화면 픽셀 단위로 이동 (드래그)
    /// 반환: 실제로 움직였는가. 이미지가 화면 안에 다 들어오면 아무것도 안 함 (맞춤 모드 유지).
    pub fn pan_by(&mut self, delta_screen: [f32; 2]) -> bool {
        let movable = (0..2).any(|i| self.image[i] * self.scale > self.screen[i] + 0.5);
        if !movable {
            return false;
        }
        let before = self.center;
        self.center[0] -= delta_screen[0] / self.scale;
        self.center[1] -= delta_screen[1] / self.scale;
        self.mode = ZoomMode::Fixed;
        self.clamp_center();
        before != self.center
    }

    /// 이미지가 화면보다 작으면 가운데 고정, 크면 가장자리 밖으로 못 나가게.
    fn clamp_center(&mut self) {
        for i in 0..2 {
            let half_view = self.screen[i] / self.scale / 2.0;
            if half_view * 2.0 >= self.image[i] {
                self.center[i] = self.image[i] / 2.0;
            } else {
                self.center[i] = self.center[i].clamp(half_view, self.image[i] - half_view);
            }
        }
    }

    pub fn screen_to_image(&self, p: [f32; 2]) -> [f32; 2] {
        [
            (p[0] - self.screen[0] / 2.0) / self.scale + self.center[0],
            (p[1] - self.screen[1] / 2.0) / self.scale + self.center[1],
        ]
    }

    pub fn image_to_screen(&self, p: [f32; 2]) -> [f32; 2] {
        [
            (p[0] - self.center[0]) * self.scale + self.screen[0] / 2.0,
            (p[1] - self.center[1]) * self.scale + self.screen[1] / 2.0,
        ]
    }

    /// 이미지가 그려질 화면 사각형 [x0, y0, x1, y1]
    pub fn image_rect_on_screen(&self) -> [f32; 4] {
        let a = self.image_to_screen([0.0, 0.0]);
        let b = self.image_to_screen(self.image);
        [a[0], a[1], b[0], b[1]]
    }

    /// 지금 화면에 필요한 디코드 해상도(긴 변, 픽셀). 원본보다 크면 원본.
    /// 축소 디코딩 hint 로 쓴다.
    pub fn needed_max_side(&self) -> u32 {
        let w = (self.image[0] * self.scale).min(self.image[0]);
        let h = (self.image[1] * self.scale).min(self.image[1]);
        w.max(h).ceil() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_small_image_not_upscaled() {
        let mut v = Viewport::new([1000.0, 800.0]);
        v.set_image([100, 50]);
        assert_eq!(v.scale, 1.0);
        v.fit_always();
        assert_eq!(v.scale, 10.0);
    }

    #[test]
    fn fit_large_image() {
        let mut v = Viewport::new([1000.0, 800.0]);
        v.set_image([4000, 2000]);
        assert_eq!(v.scale, 0.25);
        let r = v.image_rect_on_screen();
        assert_eq!(r, [0.0, 150.0, 1000.0, 650.0]);
    }

    #[test]
    fn zoom_keeps_anchor_fixed() {
        let mut v = Viewport::new([1000.0, 800.0]);
        v.set_image([4000, 3000]);
        let anchor = [700.0, 300.0];
        let before = v.screen_to_image(anchor);
        v.zoom_by(2.0, anchor);
        let after = v.screen_to_image(anchor);
        assert!((before[0] - after[0]).abs() < 0.01 && (before[1] - after[1]).abs() < 0.01);
    }

    #[test]
    fn pan_is_clamped() {
        let mut v = Viewport::new([1000.0, 800.0]);
        v.set_image([4000, 3000]);
        v.actual_size();
        assert!(v.pan_by([1e6, 1e6]));
        let r = v.image_rect_on_screen();
        assert_eq!([r[0], r[1]], [0.0, 0.0]);
    }

    #[test]
    fn pan_when_fitted_keeps_fit_mode() {
        let mut v = Viewport::new([1000.0, 800.0]);
        v.set_image([4000, 3000]);
        assert!(!v.pan_by([50.0, 50.0]));
        assert_eq!(v.mode, ZoomMode::Fit);
    }

    #[test]
    fn needed_side_caps_at_original() {
        let mut v = Viewport::new([1000.0, 800.0]);
        v.set_image([4000, 3000]);
        assert_eq!(v.needed_max_side(), 1000);
        v.zoom_to(4.0, [500.0, 400.0]);
        assert_eq!(v.needed_max_side(), 4000);
    }
}
