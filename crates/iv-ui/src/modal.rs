//! 대화 상자 공통 틀 — 여백·모서리·그림자·글 간격·단추 크기를 한 곳에서 (macOS 대화 상자 감각)
//!
//! 모든 대화 상자는 [`modal`] 로 열고, 내용 첫 줄에서 [`begin`] 을 부르고, 단추는 [`buttons`] 안에서
//! 오른쪽부터(주 단추가 맨 오른쪽) [`primary`]·[`secondary`] 로 놓는다.

use crate::{icons, theme};
use egui::{Color32, CornerRadius, Frame, Margin, RichText, Shadow, Stroke, Vec2};

/// 대화 상자 안쪽 여백 (좌우, 위아래)
const PAD: Margin = Margin { left: 28, right: 28, top: 24, bottom: 22 };

pub fn modal(id: &'static str) -> egui::Modal {
    let frame = Frame::new()
        .fill(theme::BG_POPUP)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(PAD)
        .shadow(Shadow { offset: [0, 10], blur: 32, spread: 0, color: Color32::from_black_alpha(120) });
    egui::Modal::new(egui::Id::unique(id)).frame(frame)
}

/// 너비·간격·단추 크기
pub fn begin(ui: &mut egui::Ui, width: f32) {
    ui.set_width(width);
    let s = ui.spacing_mut();
    s.item_spacing = Vec2::new(10.0, 8.0);
    s.button_padding = Vec2::new(16.0, 6.0);
    s.interact_size.y = 30.0;
}

/// 제목 줄 (아이콘 선택) + 아래 간격
pub fn title(ui: &mut egui::Ui, icon: Option<icons::Icon>, text: impl Into<String>) {
    ui.horizontal(|ui| {
        if let Some(i) = icon {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::hover());
            icons::paint(ui.painter(), i, r, theme::ACCENT);
            ui.add_space(2.0);
        }
        ui.label(RichText::new(text).size(16.0).strong().color(theme::TEXT));
    });
    ui.add_space(6.0);
}

/// 본문 글 (줄바꿈)
pub fn body(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(egui::Label::new(RichText::new(text).size(theme::FONT_BODY).color(theme::TEXT_DIM)).wrap());
}

/// 보조 설명 (작게)
pub fn note(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(egui::Label::new(RichText::new(text).size(theme::FONT_SMALL).color(theme::TEXT_FAINT)).wrap());
}

/// 단추 줄 — 본문과 띄우고 오른쪽 정렬 (먼저 놓은 단추가 맨 오른쪽)
pub fn buttons<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.add_space(14.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add).inner
}

fn sized(b: egui::Button<'_>) -> egui::Button<'_> {
    b.min_size(Vec2::new(84.0, 30.0)).corner_radius(CornerRadius::same(7))
}

/// 주 단추 (파랑)
pub fn primary(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(sized(egui::Button::new(RichText::new(label).strong().color(Color32::WHITE)).fill(theme::ACCENT_BG)))
}

/// 위험한 주 단추 (빨강 — 휴지통 등)
pub fn danger(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(sized(egui::Button::new(RichText::new(label).strong().color(Color32::WHITE)).fill(Color32::from_rgb(190, 60, 55))))
}

pub fn secondary(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(sized(egui::Button::new(RichText::new(label).color(theme::TEXT)).fill(theme::CONTROL)))
}

pub fn secondary_enabled(ui: &mut egui::Ui, enabled: bool, label: &str) -> egui::Response {
    ui.add_enabled(enabled, sized(egui::Button::new(RichText::new(label).color(theme::TEXT)).fill(theme::CONTROL)))
}

pub fn primary_enabled(ui: &mut egui::Ui, enabled: bool, label: &str) -> egui::Response {
    ui.add_enabled(enabled, sized(egui::Button::new(RichText::new(label).strong().color(Color32::WHITE)).fill(theme::ACCENT_BG)))
}
