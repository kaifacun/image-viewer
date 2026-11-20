//! egui 오버레이. 그리기만 하고 상태는 바꾸지 않는다 — 버튼은 [`Action`] 을 돌려준다.
//! 새 UI 요소(썸네일 줄·메뉴 등)는 이 crate 에 함수로 추가한다.

pub mod browser;
pub mod dialog;
pub mod icons;
pub mod theme;

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
    /// 저장하지 않은 회전이 있는가 (저장 버튼)
    pub rotated: bool,
    /// 돌릴 수 있는가 (사진 모드만 — 비디오는 회전 버튼을 숨긴다)
    pub can_rotate: bool,
}

fn panel_frame() -> Frame {
    Frame::new()
        .fill(Color32::from_rgba_unmultiplied(34, 36, 40, 238))
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(12, 10))
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
            break;
        }
    }
    // 키 기호(⌥ ⌃ 등)는 한글 글꼴에도 없다 — 기호 글꼴을 마지막 대체로 (없으면 건너뜀)
    if let Ok(bytes) = std::fs::read("/System/Library/Fonts/Apple Symbols.ttf") {
        ctx.add_font(egui::epaint::text::FontInsert::new(
            "system-symbols",
            egui::FontData::from_owned(bytes),
            vec![egui::epaint::text::InsertFontFamily { family: egui::FontFamily::Proportional, priority: egui::epaint::text::FontPriority::Lowest }],
        ));
    }
}

/// 오버레이를 그리고, 사용자가 누른 버튼의 Action 을 돌려준다.
pub fn draw(ctx: &egui::Context, m: &UiModel<'_>) -> Vec<Action> {
    let mut actions = Vec::new();

    //#region 아래쪽 떠 있는 막대 — 목록 · 이전/다음 · 맞춤/1:1 · 정보 · 회전/저장 | 상태
    egui::Area::new(egui::Id::unique("iv-view-bar")).anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -14.0)).show(ctx, |ui| {
        Frame::new()
            .fill(Color32::from_rgba_unmultiplied(34, 36, 40, 238))
            .stroke(egui::Stroke::new(1.0, theme::BORDER))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::symmetric(8, 5))
            .shadow(egui::epaint::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(110) })
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    use icons::{Icon, icon_button, icon_toggle};
                    let sep = |ui: &mut egui::Ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::new(11.0, 18.0), egui::Sense::hover());
                        ui.painter().line_segment([r.center_top(), r.center_bottom()], egui::Stroke::new(1.0, theme::BORDER));
                    };
                    if m.can_browse && icon_button(ui, Icon::Grid, "목록으로 (Esc)", true, false).clicked() {
                        actions.push(Action::ExitView);
                    }
                    if m.can_browse {
                        sep(ui);
                    }
                    if icon_button(ui, Icon::ChevronLeft, "이전 (←)", m.has_image, false).clicked() {
                        actions.push(Action::Prev);
                    }
                    if icon_button(ui, Icon::ChevronRight, "다음 (→)", m.has_image, false).clicked() {
                        actions.push(Action::Next);
                    }
                    sep(ui);
                    if icon_button(ui, Icon::Expand, "화면에 맞춤 (0)", m.has_image, false).clicked() {
                        actions.push(Action::Fit);
                    }
                    if ui.add_enabled(m.has_image, egui::Button::new(RichText::new("1:1").size(theme::FONT_SMALL).strong()).frame(false).min_size(Vec2::new(28.0, 26.0))).on_hover_text("원본 크기 (1)").clicked() {
                        actions.push(Action::ActualSize);
                    }
                    if icon_toggle(ui, Icon::Info, "정보 (I)", m.has_image, false, m.show_info).clicked() {
                        actions.push(Action::ToggleInfo);
                    }
                    if m.can_rotate {
                        sep(ui);
                        if icon_button(ui, Icon::RotateLeft, "왼쪽으로 90° (L)", m.has_image, false).clicked() {
                            actions.push(Action::RotateCcw);
                        }
                        if icon_button(ui, Icon::RotateRight, "오른쪽으로 90° (R)", m.has_image, false).clicked() {
                            actions.push(Action::RotateCw);
                        }
                        if m.rotated && icon_toggle(ui, Icon::Save, "돌린 방향을 원본에 저장 (⌘S) — 화질 손실 없음", true, false, true).clicked() {
                            actions.push(Action::SaveRotation);
                        }
                    }
                    sep(ui);
                    ui.add_space(4.0);
                    ui.label(RichText::new(m.status).color(theme::TEXT_DIM).size(theme::FONT_SMALL));
                    ui.add_space(6.0);
                });
            });
    });
    //#endregion

    // 정보 창
    if m.show_info && !m.info.is_empty() {
        egui::Area::new(egui::Id::unique("info")).anchor(Align2::LEFT_TOP, Vec2::new(12.0, 12.0)).show(ctx, |ui| {
            panel_frame().show(ui, |ui| {
                egui::Grid::new("info-grid").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                    for (k, v) in m.info {
                        ui.label(RichText::new(k).size(theme::FONT_SMALL).color(theme::TEXT_FAINT));
                        ui.label(RichText::new(v).size(theme::FONT_SMALL).color(theme::TEXT));
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
