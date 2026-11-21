//! egui 오버레이. 그리기만 하고 상태는 바꾸지 않는다 — 버튼은 [`Action`] 을 돌려준다.
//! 새 UI 요소(썸네일 줄·메뉴 등)는 이 crate 에 함수로 추가한다.

pub mod browser;
pub mod dialog;
pub mod modal;
pub mod icons;
pub mod markdown;
pub mod theme;

use egui::{Align2, Color32, CornerRadius, Frame, Margin, RichText, Vec2};
use iv_viewer::{Action, PageCmd, VideoCmd};

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
    /// 비디오 재생 중이면 조작 막대 내용
    pub video: Option<VideoUi>,
    /// PDF 를 보는 중이면 (지금 쪽 0부터, 쪽 수)
    pub pages: Option<(usize, usize)>,
    /// 글 문서를 보는 중이면 (이미지 대신 글)
    pub text: Option<TextView<'a>>,
}

/// 전체 보기의 글 문서
pub struct TextView<'a> {
    pub name: &'a str,
    pub text: &'a str,
    /// 줄 번호·고정폭
    pub code: bool,
    /// 마크다운 파일인가 (보기/원문 전환 버튼)
    pub markdown: bool,
    /// 마크다운을 꾸며 보이는 중
    pub rendered: bool,
    /// 인코딩·잘림 안내
    pub note: &'a str,
}

/// 비디오 조작 막대에 보일 상태
#[derive(Clone, Copy, Debug)]
pub struct VideoUi {
    pub playing: bool,
    pub position: f64,
    pub duration: f64,
    pub volume: f32,
    pub muted: bool,
}

/// 초 → "1:05" / "1:02:03"
pub fn format_clock(t: f64) -> String {
    let t = t.max(0.0) as u64;
    let (h, m, s) = (t / 3600, t / 60 % 60, t % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
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
    if let Some(t) = &m.text {
        text_view(ctx, t, &mut actions);
    }

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
                    // 떠 있는 막대 위에서도 막대(슬라이더) 홈이 보이게
                    ui.visuals_mut().extreme_bg_color = theme::PRESSED;
                    ui.visuals_mut().widgets.inactive.bg_fill = theme::PRESSED;
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
                    let (prev_tip, next_tip) = if m.video.is_some() { ("이전 영상 (PageUp)", "다음 영상 (PageDown)") } else { ("이전 (←)", "다음 (→)") };
                    if icon_button(ui, Icon::ChevronLeft, prev_tip, m.has_image, false).clicked() {
                        actions.push(Action::Prev);
                    }
                    //#region 비디오 — 재생/정지 · 시간 · 위치 막대 · 소리
                    if let Some(v) = m.video {
                        let (icon, tip) = if v.playing { (Icon::Pause, "일시 정지 (Space)") } else { (Icon::Play, "재생 (Space)") };
                        if icon_toggle(ui, icon, tip, true, false, false).clicked() {
                            actions.push(Action::Video(VideoCmd::Toggle));
                        }
                    }
                    //#endregion
                    if icon_button(ui, Icon::ChevronRight, next_tip, m.has_image, false).clicked() {
                        actions.push(Action::Next);
                    }
                    sep(ui);
                    //#region 문서 쪽 — 이전·다음 쪽, "3 / 12", 쪽 막대
                    if let Some((page, pages)) = m.pages {
                        if icon_button(ui, Icon::ChevronUp, "이전 쪽 (←)", page > 0, false).clicked() {
                            actions.push(Action::Page(PageCmd::Prev));
                        }
                        ui.label(RichText::new(format!("{} / {pages}", page + 1)).size(theme::FONT_SMALL).color(theme::TEXT).monospace());
                        if icon_button(ui, Icon::ChevronDown, "다음 쪽 (→ · Space)", page + 1 < pages, false).clicked() {
                            actions.push(Action::Page(PageCmd::Next));
                        }
                        if pages > 2 {
                            let mut p = page + 1;
                            ui.spacing_mut().slider_width = 160.0;
                            if ui.add(egui::Slider::new(&mut p, 1..=pages).show_value(false).trailing_fill(true)).on_hover_text("쪽으로 가기 (Home · End)").changed() {
                                actions.push(Action::Page(PageCmd::Go(p - 1)));
                            }
                        }
                        sep(ui);
                    }
                    //#endregion
                    if let Some(v) = m.video {
                        ui.add_space(4.0);
                        ui.label(RichText::new(format_clock(v.position)).size(theme::FONT_SMALL).color(theme::TEXT).monospace());
                        let mut pos = v.position;
                        let dur = v.duration.max(0.01);
                        ui.spacing_mut().slider_width = 280.0;
                        let r = ui.add(egui::Slider::new(&mut pos, 0.0..=dur).show_value(false).trailing_fill(true)).on_hover_text("위치 (←/→ 5초, ⇧ 1초)");
                        if r.changed() {
                            actions.push(Action::Video(VideoCmd::Seek(pos)));
                        }
                        ui.label(RichText::new(format_clock(v.duration)).size(theme::FONT_SMALL).color(theme::TEXT_DIM).monospace());
                        sep(ui);
                        let (icon, tip) = if v.muted || v.volume <= 0.0 { (Icon::Mute, "소리 켜기 (M)") } else { (Icon::Volume, "소리 끄기 (M)") };
                        if icon_button(ui, icon, tip, true, false).clicked() {
                            actions.push(Action::Video(VideoCmd::ToggleMute));
                        }
                        let mut vol = if v.muted { 0.0 } else { v.volume };
                        ui.spacing_mut().slider_width = 80.0;
                        if ui.add(egui::Slider::new(&mut vol, 0.0..=1.0).show_value(false).trailing_fill(true)).on_hover_text("소리 크기 (↑/↓)").changed() {
                            actions.push(Action::Video(VideoCmd::Volume(vol)));
                        }
                        sep(ui);
                    }
                    // 글 문서엔 맞춤·1:1 이 없다
                    if m.text.is_none() && icon_button(ui, Icon::Expand, "화면에 맞춤 (0)", m.has_image, false).clicked() {
                        actions.push(Action::Fit);
                    }
                    if m.text.is_none() && ui.add_enabled(m.has_image, egui::Button::new(RichText::new("1:1").size(theme::FONT_SMALL).strong()).frame(false).min_size(Vec2::new(28.0, 26.0))).on_hover_text("원본 크기 (1)").clicked() {
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
                    // 지금 보는 파일을 휴지통으로 (⌘⌫ 와 같은 길 — 되살릴 수 있음)
                    if icon_button(ui, Icon::Trash, "휴지통으로 이동 (⌘⌫)", m.has_image, false).clicked() {
                        actions.push(Action::Trash);
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

/// 추가 코덱 엔진 받기 동의 창 — 답이 있으면 그 Action
pub fn codec_prompt(ctx: &egui::Context) -> Option<Action> {
    let mut out = None;
    modal::modal("iv-codec-prompt").show(ctx, |ui| {
        modal::begin(ui, 440.0);
        modal::title(ui, Some(icons::Icon::Video), "추가 코덱 엔진이 필요합니다");
        modal::body(ui, "이 영상은 macOS 기본 엔진으로 열 수 없는 형식입니다 (MKV·WebM·AVI·WMV·FLV·VP9·MPEG-2 등). 무료 코덱 엔진 FFmpeg 를 받아 별도 프로그램으로 사용합니다.");
        ui.add_space(4.0);
        modal::note(ui, "• Homebrew 가 있으면 brew 로 설치, 없으면 검증된 빌드(약 57MB)를 받아 SHA-256 이 맞을 때만 씁니다.\n• 한 번 받으면 이후 필요한 영상은 자동으로 처리합니다.");
        modal::buttons(ui, |ui| {
            if modal::primary(ui, "받기").clicked() {
                out = Some(Action::InstallCodecs);
            }
            if modal::secondary(ui, "나중에").clicked() {
                out = Some(Action::DismissCodecPrompt);
            }
        });
    });
    out
}

/// 업데이트 창에 보일 상태 (앱이 iv-update 상태를 옮겨 담는다 — iv-ui 는 iv-update 를 모른다)
pub enum UpdateView<'a> {
    Checking,
    UpToDate,
    /// 새 판. `blocked` = 자동 설치를 못 하는 까닭 (있으면 배포 쪽 열기만)
    Available { version: &'a str, notes: &'a str, blocked: Option<&'a str> },
    Downloading { version: &'a str, got: u64, total: u64 },
    Ready { version: &'a str },
    Failed { message: &'a str },
}

pub struct UpdateUi<'a> {
    pub current: &'a str,
    pub view: UpdateView<'a>,
    /// 하루 한 번 자동 확인
    pub auto: bool,
}

/// 인앱 업데이트 창 — 답이 있으면 그 Action
pub fn update_prompt(ctx: &egui::Context, u: &UpdateUi<'_>) -> Option<Action> {
    let mut out = None;
    let shown = modal::modal("iv-update-prompt").show(ctx, |ui| {
        modal::begin(ui, 480.0);
        let title = match &u.view {
            UpdateView::Checking => "업데이트 확인 중…".to_string(),
            UpdateView::UpToDate => "최신 판입니다".to_string(),
            UpdateView::Available { version, .. } => format!("새 판 KaiIV {version} 이 나왔습니다"),
            UpdateView::Downloading { version, .. } => format!("KaiIV {version} 받는 중…"),
            UpdateView::Ready { version } => format!("KaiIV {version} 설치 준비가 끝났습니다"),
            UpdateView::Failed { .. } => "업데이트하지 못했습니다".to_string(),
        };
        modal::title(ui, Some(icons::Icon::Refresh), title);
        match &u.view {
            UpdateView::Checking => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    modal::body(ui, "배포처에서 새 판을 찾고 있습니다.");
                });
            }
            UpdateView::UpToDate => modal::body(ui, format!("지금 쓰시는 KaiIV {} 이 최신 판입니다.", u.current)),
            UpdateView::Available { notes, blocked, .. } => {
                modal::body(ui, format!("지금 판은 {} 입니다.", u.current));
                if !notes.is_empty() {
                    ui.add_space(4.0);
                    Frame::new().fill(theme::BG_CANVAS).corner_radius(8).inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
                        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            // 알림 글은 평문 그대로 (링크·마크다운 해석 안 함)
                            ui.label(RichText::new(*notes).size(theme::FONT_SMALL).color(theme::TEXT));
                        });
                    });
                }
                if let Some(b) = blocked {
                    ui.add_space(4.0);
                    ui.add(egui::Label::new(RichText::new(format!("자동 설치 불가: {b}")).size(theme::FONT_SMALL).color(theme::WARN)).wrap());
                }
            }
            UpdateView::Downloading { got, total, .. } => {
                let f = if *total > 0 { (*got as f32 / *total as f32).clamp(0.0, 1.0) } else { 0.0 };
                ui.add(egui::ProgressBar::new(f).show_percentage());
                modal::note(ui, format!("{} / {} · 받은 뒤 서명과 해시를 확인합니다", iv_viewer::human_bytes(*got), iv_viewer::human_bytes(*total)));
            }
            UpdateView::Ready { .. } => modal::body(ui, "서명과 해시를 확인했습니다. KaiIV 를 다시 시작하면 새 판으로 바뀝니다."),
            UpdateView::Failed { message } => {
                ui.add(egui::Label::new(RichText::new(*message).color(theme::ERROR)).wrap());
                modal::note(ui, "배포 쪽에서 직접 받으실 수도 있습니다.");
            }
        }
        ui.add_space(8.0);
        let mut auto = u.auto;
        if ui.checkbox(&mut auto, RichText::new("하루 한 번 자동으로 확인").size(theme::FONT_SMALL).color(theme::TEXT_DIM)).changed() {
            out = Some(Action::SetUpdateAuto(auto));
        }
        modal::buttons(ui, |ui| match &u.view {
            UpdateView::Checking | UpdateView::Downloading { .. } => {
                if modal::secondary(ui, "닫기").clicked() {
                    out = Some(Action::UpdateLater);
                }
            }
            UpdateView::UpToDate => {
                if modal::primary(ui, "확인").clicked() {
                    out = Some(Action::UpdateLater);
                }
            }
            UpdateView::Available { blocked, .. } => {
                let main = if blocked.is_some() { ("배포 쪽 열기", Action::UpdateOpenPage) } else { ("지금 업데이트", Action::UpdateNow) };
                if modal::primary(ui, main.0).clicked() {
                    out = Some(main.1);
                }
                if modal::secondary(ui, "나중에").clicked() {
                    out = Some(Action::UpdateLater);
                }
                if modal::secondary(ui, "이 판 건너뛰기").clicked() {
                    out = Some(Action::UpdateSkip);
                }
            }
            UpdateView::Ready { .. } => {
                if modal::primary(ui, "다시 시작해 설치").clicked() {
                    out = Some(Action::UpdateRestart);
                }
                if modal::secondary(ui, "나중에").clicked() {
                    out = Some(Action::UpdateLater);
                }
            }
            UpdateView::Failed { .. } => {
                if modal::primary(ui, "닫기").clicked() {
                    out = Some(Action::UpdateLater);
                }
                if modal::secondary(ui, "배포 쪽 열기").clicked() {
                    out = Some(Action::UpdateOpenPage);
                }
            }
        });
    });
    // 바깥을 누르거나 Esc 면 '나중에'
    if out.is_none() && shown.should_close() {
        out = Some(Action::UpdateLater);
    }
    out
}

//#region 글 문서 보기 — 마크다운(꾸민 보기/원문)·글·코드(줄 번호)
fn text_view(ctx: &egui::Context, t: &TextView<'_>, actions: &mut Vec<Action>) {
    ctx.root_ui(|ui| {
        egui::CentralPanel::default().frame(Frame::new().fill(theme::BG_CANVAS)).show(ui, |ui| {
            //#region 머리 — 파일 이름 · 안내 · 보기/원문
            egui::Frame::new().fill(theme::BG_PANEL).inner_margin(Margin::symmetric(16, 8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), egui::Sense::hover());
                    icons::paint(ui.painter(), icons::Icon::Document, r, theme::ACCENT);
                    ui.label(RichText::new(t.name).strong().color(theme::TEXT));
                    ui.label(RichText::new(t.note).size(theme::FONT_SMALL).color(theme::TEXT_FAINT));
                    if t.markdown {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            for (label, on) in [("원문", !t.rendered), ("보기", t.rendered)] {
                                let b = egui::Button::new(RichText::new(label).color(if on { Color32::WHITE } else { theme::TEXT_DIM })).selected(on);
                                if ui.add(b).on_hover_text("마크다운 보기 ↔ 원문 (⌘/)").clicked() && !on {
                                    actions.push(Action::ToggleMarkdown);
                                }
                            }
                        });
                    }
                });
            });
            //#endregion
            let bottom_pad = 80.0; // 아래 떠 있는 막대에 가리지 않게
            if t.code {
                code_lines(ui, t.text, bottom_pad);
                return;
            }
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                // 읽기 좋은 폭 (가운데)
                let w = ui.available_width().min(860.0);
                let pad = ((ui.available_width() - w) / 2.0).max(16.0);
                ui.horizontal_top(|ui| {
                    ui.add_space(pad);
                    ui.vertical(|ui| {
                        ui.set_max_width(w);
                        ui.add_space(18.0);
                        if t.markdown && t.rendered {
                            markdown::show(ui, t.text);
                        } else {
                            ui.add(egui::Label::new(RichText::new(t.text).size(14.0).color(theme::TEXT).line_height(Some(22.0))).selectable(true).wrap());
                        }
                        ui.add_space(bottom_pad);
                    });
                });
            });
        });
    });
}

/// 코드: 줄 번호 + 고정폭, 보이는 줄만 그린다 (큰 파일도 가볍게)
fn code_lines(ui: &mut egui::Ui, text: &str, bottom_pad: f32) {
    let lines: Vec<&str> = text.lines().collect();
    let font = egui::FontId::monospace(13.0);
    let row_h = 19.0;
    let num_w = format!("{}", lines.len()).len() as f32 * 8.0 + 20.0;
    egui::ScrollArea::both().auto_shrink(false).show_rows(ui, row_h, lines.len() + (bottom_pad / row_h) as usize, |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for i in range {
            ui.horizontal(|ui| {
                ui.set_height(row_h);
                let Some(l) = lines.get(i) else { return };
                let (r, _) = ui.allocate_exact_size(Vec2::new(num_w, row_h), egui::Sense::hover());
                ui.painter().text(r.right_center() - Vec2::new(10.0, 0.0), Align2::RIGHT_CENTER, format!("{}", i + 1), font.clone(), theme::TEXT_FAINT);
                ui.add(egui::Label::new(RichText::new(*l).font(font.clone()).color(theme::TEXT)).selectable(true).extend());
            });
        }
    });
}
//#endregion
