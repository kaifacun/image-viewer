//! 화면 테마 — 색·글자 크기·간격·모서리를 한곳에서 정한다 (어두운 회색 + 파란 강조색 하나).
//! 그리기 코드는 여기 상수만 쓰고 색을 직접 만들지 않는다.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Vec2};

//#region 색
/// 격자·이미지 뒤 (가장 어두움)
pub const BG_CANVAS: Color32 = Color32::from_rgb(24, 25, 28);
/// 사이드바·도구 막대·패널
pub const BG_PANEL: Color32 = Color32::from_rgb(32, 33, 37);
/// 메뉴·팝업
pub const BG_POPUP: Color32 = Color32::from_rgb(40, 42, 47);
/// 패널 경계선
pub const BORDER: Color32 = Color32::from_rgb(48, 50, 56);
/// 버튼 바탕
pub const CONTROL: Color32 = Color32::from_rgb(44, 46, 52);
pub const HOVER: Color32 = Color32::from_rgb(54, 57, 64);
pub const PRESSED: Color32 = Color32::from_rgb(64, 68, 76);
/// 강조색 (선택·켜짐)
pub const ACCENT: Color32 = Color32::from_rgb(64, 140, 255);
pub const ACCENT_BG: Color32 = Color32::from_rgb(38, 92, 178);
/// 선택된 타일·줄 바탕
pub const SELECTED_BG: Color32 = Color32::from_rgb(34, 66, 112);
pub const TILE: Color32 = Color32::from_rgb(34, 35, 40);
pub const TILE_HOVER: Color32 = Color32::from_rgb(44, 46, 52);
pub const TEXT: Color32 = Color32::from_rgb(222, 224, 228);
pub const TEXT_DIM: Color32 = Color32::from_rgb(150, 154, 162);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(112, 116, 124);
pub const TEXT_DISABLED: Color32 = Color32::from_rgb(88, 91, 98);
/// 필터가 걸려 있음 등 주의
pub const WARN: Color32 = Color32::from_rgb(255, 196, 92);
pub const ERROR: Color32 = Color32::from_rgb(240, 120, 110);
//#endregion

//#region 글자 크기
pub const FONT_BODY: f32 = 13.0;
pub const FONT_SMALL: f32 = 11.5;
pub const FONT_CAPTION: f32 = 10.5;
//#endregion

/// 상단 막대 높이 (도구 막대)
pub const TOOLBAR_H: f32 = 40.0;

/// egui 전체 모양을 테마에 맞춘다 (시작할 때 한 번)
pub fn install(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Heading, FontId::new(15.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(FONT_BODY, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(FONT_BODY, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(FONT_SMALL, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
        ]
        .into();
        let sp = &mut s.spacing;
        sp.item_spacing = Vec2::new(6.0, 5.0);
        sp.button_padding = Vec2::new(10.0, 4.0);
        sp.interact_size = Vec2::new(28.0, 26.0);
        sp.menu_margin = Margin::same(6);
        sp.window_margin = Margin::same(12);
        sp.indent = 16.0;
        let v = &mut s.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = BG_PANEL;
        v.window_fill = BG_POPUP;
        v.extreme_bg_color = BG_CANVAS;
        v.faint_bg_color = CONTROL;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(8);
        v.selection.bg_fill = ACCENT_BG;
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.hyperlink_color = ACCENT;
        v.override_text_color = None;
        let r = CornerRadius::same(6);
        for (w, bg) in [
            (&mut v.widgets.noninteractive, BG_PANEL),
            (&mut v.widgets.inactive, CONTROL),
            (&mut v.widgets.hovered, HOVER),
            (&mut v.widgets.active, PRESSED),
            (&mut v.widgets.open, HOVER),
        ] {
            w.bg_fill = bg;
            w.weak_bg_fill = bg;
            w.corner_radius = r;
            w.bg_stroke = Stroke::NONE;
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        v.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        v.widgets.hovered.bg_stroke = Stroke::NONE;
    });
}
