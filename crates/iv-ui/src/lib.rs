//! egui 오버레이. 그리기만 하고 상태는 바꾸지 않는다 — 버튼은 [`Action`] 을 돌려준다.
//! 새 UI 요소(썸네일 줄·메뉴 등)는 이 crate 에 함수로 추가한다.

pub mod browser;
pub mod dialog;

use egui::{Align2, Color32, CornerRadius, Frame, Margin, RichText, Vec2};
use iv_viewer::Action;

/// UI 가 그릴 내용 (뷰어 상태에서 앱이 만든다)
pub struct UiModel<'a> {
    pub status: &'a str,
    pub show_info: bool,
    /// 정보 창 (이름, 값)
    pub info: &'a [(String, String)],
    pub has_image: bool,
    /// 목록(브라우저)으로 돌아갈 수 있는가
    pub can_browse: bool,
}

fn panel_frame() -> Frame {
    Frame::new()
        .fill(Color32::from_black_alpha(170))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 6))
}

/// 한글 등 기본 폰트에 없는 글자를 위해 OS 폰트를 폴백으로 추가한다 (없으면 조용히 건너뜀).
pub fn install_fonts(ctx: &egui::Context) {
    const CANDIDATES: &[(&str, u32)] = &[
        // macOS
        ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 0),
        ("/System/Library/Fonts/Supplemental/AppleGothic.ttf", 0),
        // Windows
        ("C:\\Windows\\Fonts\\malgun.ttf", 0),
        // Linux
        ("/usr/share/fonts/truetype/nanum/NanumGothic.ttf", 0),
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
    ];
    for (path, index) in CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            let mut data = egui::FontData::from_owned(bytes);
            data.index = *index;
            ctx.add_font(egui::epaint::text::FontInsert::new(
                "system-cjk",
                data,
                vec![
                    egui::epaint::text::InsertFontFamily { family: egui::FontFamily::Proportional, priority: egui::epaint::text::FontPriority::Lowest },
                    egui::epaint::text::InsertFontFamily { family: egui::FontFamily::Monospace, priority: egui::epaint::text::FontPriority::Lowest },
                ],
            ));
            return;
        }
    }
}

/// 오버레이를 그리고, 사용자가 누른 버튼의 Action 을 돌려준다.
pub fn draw(ctx: &egui::Context, m: &UiModel<'_>) -> Vec<Action> {
    let mut actions = Vec::new();

    // 아래쪽 상태 표시줄 + 조작 버튼
    egui::Area::new(egui::Id::unique("status")).anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -12.0)).show(ctx, |ui| {
        panel_frame().show(ui, |ui| {
            ui.horizontal(|ui| {
                if m.can_browse && ui.add(egui::Button::new(RichText::new("▦ 목록").size(14.0)).frame(false)).on_hover_text("목록으로 (Esc)").clicked() {
                    actions.push(Action::ExitView);
                }
                let mut btn = |ui: &mut egui::Ui, label: &str, tip: &str, a: Action| {
                    if ui.add_enabled(m.has_image, egui::Button::new(RichText::new(label).size(14.0)).frame(false))
                        .on_hover_text(tip)
                        .clicked()
                    {
                        actions.push(a);
                    }
                };
                btn(ui, "◀", "이전 (←)", Action::Prev);
                btn(ui, "▶", "다음 (→)", Action::Next);
                ui.separator();
                btn(ui, "맞춤", "화면 맞춤 (0)", Action::Fit);
                btn(ui, "1:1", "원본 크기 (1)", Action::ActualSize);
                btn(ui, "정보", "정보 (I)", Action::ToggleInfo);
                ui.separator();
                ui.label(RichText::new(m.status).color(Color32::from_gray(225)).size(13.0));
            });
        });
    });

    // 정보 창
    if m.show_info && !m.info.is_empty() {
        egui::Area::new(egui::Id::unique("info")).anchor(Align2::LEFT_TOP, Vec2::new(12.0, 12.0)).show(ctx, |ui| {
            panel_frame().show(ui, |ui| {
                egui::Grid::new("info-grid").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                    for (k, v) in m.info {
                        ui.label(RichText::new(k).color(Color32::from_gray(160)));
                        ui.label(RichText::new(v).color(Color32::from_gray(235)));
                        ui.end_row();
                    }
                });
            });
        });
    }
    // 버튼 포커스가 화살표·Enter 단축키를 삼키지 않게
    if !ctx.text_edit_focused() {
        ctx.memory_mut(|mem| {
            if let Some(id) = mem.focused() {
                mem.surrender_focus(id);
            }
        });
    }
    actions
}
