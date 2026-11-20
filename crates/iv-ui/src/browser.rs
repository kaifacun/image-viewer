//! 브라우저 화면 (FastStone 형): 메뉴·도구모음·경로 줄 / 폴더 트리 + 미리보기 칸 / 썸네일 격자 / 상태줄.
//!
//! 그리기만 하고 상태를 바꾸지 않는다 — 사용자의 동작은 [`BrowserOut`] 으로 돌려준다.
//! 미리보기 칸은 **투명**으로 비워 두고 그 사각형을 돌려준다: 앱이 렌더러의 이미지 경로(밉맵)로 그 칸에 그린다.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, FontId, Margin, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2};
use iv_core::Frame;
use iv_fstree::Row;
use iv_library::{Entry, EntryId};
use iv_viewer::{Action, SortKey};

use crate::dialog::{self, Dialog, DialogResult};
use crate::icons::{Icon, icon_button, icon_toggle};
use crate::theme;

/// 썸네일 GPU 텍스처 묶음 (바이트 목표치 — 보이는 것 + 여유를 밑돌지 않음, 보이는 범위 밖부터 해제).
/// 삽입은 egui 패스 안에서만 한다 ([`ThumbTextures::flush`]) — 텍스처 delta 가 그 패스 출력에 실리도록.
pub struct ThumbTextures {
    map: HashMap<u64, TextureHandle>,
    bytes: usize,
    budget: usize,
    pending: VecDeque<(u64, Arc<Frame>)>,
    /// 원본 크기 (텍스처를 해제해도 남겨 둔다 — 작음)
    sizes: HashMap<u64, [u32; 2]>,
}

impl Default for ThumbTextures {
    fn default() -> Self {
        Self { map: HashMap::new(), bytes: 0, budget: 96 << 20, pending: VecDeque::new(), sizes: HashMap::new() }
    }
}

impl ThumbTextures {
    /// 다음 패스에서 텍스처로 올릴 썸네일 (방향 적용된 꽉 찬 RGBA)
    pub fn push(&mut self, key: u64, image: Arc<Frame>, full_size: [u32; 2]) {
        self.sizes.insert(key, full_size);
        if !self.map.contains_key(&key) {
            self.pending.push_back((key, image));
        }
    }

    pub fn has(&self, key: u64) -> bool {
        self.map.contains_key(&key) || self.pending.iter().any(|(k, _)| *k == key)
    }

    pub fn full_size(&self, key: u64) -> Option<[u32; 2]> {
        self.sizes.get(&key).copied()
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// 폴더가 바뀜·렌더러 교체 — 모두 해제 (핸들 drop → 다음 패스의 textures_delta.free)
    pub fn clear(&mut self) {
        self.map.clear();
        self.pending.clear();
        self.sizes.clear();
        self.bytes = 0;
    }

    /// 대기 중인 것을 최대 `max` 장 올리고, 바이트 상한을 넘으면 `keep` 밖부터 해제
    fn flush(&mut self, ctx: &egui::Context, max: usize, keep: &HashSet<u64>) {
        for _ in 0..max {
            let Some((key, f)) = self.pending.pop_front() else { break };
            if self.map.contains_key(&key) {
                continue;
            }
            let img = egui::ColorImage::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.pixels[..f.width as usize * f.height as usize * 4]);
            let h = ctx.load_texture(format!("thumb-{key}"), img, TextureOptions::LINEAR);
            self.bytes += f.width as usize * f.height as usize * 4;
            self.map.insert(key, h);
        }
        // 보이는 것은 해제하지 않는다 — 상한은 "보이는 것 + 여유"를 밑돌지 않게 (큰 모니터·작은 타일에서 반복 해제·재업로드 방지)
        let keep_bytes: usize = self.map.iter().filter(|(k, _)| keep.contains(k)).map(|(_, h)| h.size()[0] * h.size()[1] * 4).sum();
        let budget = self.budget.max(keep_bytes + (32 << 20));
        if self.bytes > budget {
            let drop: Vec<u64> = self.map.keys().filter(|k| !keep.contains(k)).copied().collect();
            for k in drop {
                if self.bytes <= budget * 3 / 4 {
                    break;
                }
                if let Some(h) = self.map.remove(&k) {
                    let [w, hh] = h.size();
                    self.bytes -= w * hh * 4;
                }
            }
        }
    }

    fn get(&self, key: u64) -> Option<&TextureHandle> {
        self.map.get(&key)
    }
}

/// 썸네일을 끌 때 실어 나르는 표식 (옮길 대상은 뷰어의 선택)
#[derive(Clone, Copy, Debug)]
pub struct DragPhotos {
    pub count: usize,
}

/// 빈 공간에서 끌어 그리는 선택 상자 (내용 좌표 — 스크롤해도 시작점이 따라간다)
#[derive(Clone, Debug)]
struct Marquee {
    start: Vec2,
    /// ⌘/⇧ 를 누르고 시작 — 기존 선택에 더한다
    keep: bool,
    /// 마지막으로 보낸 항목 (바뀔 때만 다시 보낸다)
    last: Vec<usize>,
    sent_begin: bool,
}

/// 프레임 사이에 유지되는 UI 상태 (앱이 소유)
pub struct BrowserState {
    pub textures: ThumbTextures,
    /// 다음 프레임에 이 항목이 보이도록 스크롤 (한 번만)
    pub scroll_to: Option<usize>,
    /// 썸네일 칸 크기 (점)
    pub tile: f32,
    /// 경로 줄 편집 중인 글
    path_edit: String,
    path_edit_for: Option<PathBuf>,
    /// 직전 격자 보이는 영역 (내용 좌표: y 오프셋, 높이)
    last_view: Option<(f32, f32)>,
    /// 직전 프레임 썸네일 요청 범위 (텍스처 해제에서 제외)
    last_visible: Range<usize>,
    /// 열려 있는 이름 입력 대화 상자
    pub dialog: Option<Dialog>,
    /// 사진 없는 폴더도 트리에 보이기 (옮길 곳으로 쓰려고)
    pub show_empty_folders: bool,
    marquee: Option<Marquee>,
    /// 오른쪽 정보 패널 (미리보기 + 파일 정보)
    pub inspector: bool,
    /// 경로 표시를 글로 고쳐 쓰는 중 (경로 칸을 누르면)
    path_editing: bool,
}

impl Default for BrowserState {
    fn default() -> Self {
        Self { textures: ThumbTextures::default(), scroll_to: None, tile: 128.0, path_edit: String::new(), path_edit_for: None, last_view: None, last_visible: 0..0, dialog: None, show_empty_folders: false, marquee: None, inspector: true, path_editing: false }
    }
}

impl BrowserState {
    /// egui 컨텍스트가 새로 만들어졌을 때(렌더러 교체) — 스크롤 위치가 사라졌으니 선택 항목으로 다시 스크롤
    /// 새 폴더 대화 상자 (현재 폴더 안)
    pub fn begin_new_folder(&mut self, parent: &Path) {
        self.dialog = Some(Dialog::new_folder(parent.to_path_buf()));
    }

    /// 사진 이름 바꾸기 대화 상자
    pub fn begin_rename_file(&mut self, path: &Path) {
        self.dialog = Some(Dialog::rename_file(path.to_path_buf()));
    }

    pub fn reset_context(&mut self, selected: Option<usize>) {
        self.textures.clear();
        self.last_view = None;
        self.scroll_to = selected;
    }
}

pub const TILE_MIN: f32 = 80.0;
pub const TILE_MAX: f32 = 200.0;

/// 브라우저가 그릴 내용
pub struct BrowserModel<'a> {
    pub entries: &'a [Entry],
    /// 현재 항목 (미리보기·키보드 기준)
    pub selected: Option<usize>,
    /// 여러 장 선택 (현재 항목 포함)
    pub marked: &'a HashSet<EntryId>,
    pub dir: Option<&'a Path>,
    pub tree: &'a [Row],
    /// 트리 줄마다 (이 폴더의 사진 수, 보일 하위 폴더 수) — 스캔 전이면 None
    pub tree_counts: &'a [Option<(u32, u32)>],
    pub status: &'a str,
    pub sort: (SortKey, bool),
    pub failed: &'a HashSet<u64>,
    /// 백그라운드 스캔 상태 (상태줄 오른쪽)
    pub scan_status: &'a str,
    /// 잘라낸(붙여넣으면 옮겨질) 파일 — 흐리게 그린다
    pub cut: &'a HashSet<PathBuf>,
    /// 확장자 필터 메뉴의 형식 묶음
    pub groups: &'a [iv_core::formats::FormatGroup],
    /// 지금 보는 종류 (사진·비디오)
    pub kind: iv_core::formats::MediaKind,
    /// 추가 코덱 엔진: 찾았으면 어디서 왔는지 ("Homebrew" 등), 없으면 None
    pub codec_engine: Option<&'a str>,
    /// 지금 걸린 확장자 필터 (None = 전부)
    pub filter: Option<&'a HashSet<&'static str>>,
    /// 미리보기 칸에 렌더러가 실제 프레임을 그리는가 (아니면 썸네일로 대신 채움)
    pub preview_has_frame: bool,
}

#[derive(Default)]
pub struct BrowserOut {
    pub actions: Vec<Action>,
    /// 클립보드에 넣을 글 (트리 폴더 경로 복사) — 앱이 쓴다
    pub copy_text: Option<String>,
    /// 펼침/접기 전환할 트리 노드
    pub tree_toggle: Option<PathBuf>,
    /// 트리 표시 설정이 바뀜 (다시 만들어야 함)
    pub tree_changed: bool,
    /// 썸네일을 요청할 항목 범위 (보이는 줄 ±2줄)
    pub visible: Range<usize>,
    /// 미리보기 칸 (점 단위, 화면 좌표)
    pub preview_rect: Option<Rect>,
    pub grid_cols: usize,
    pub grid_rows_visible: usize,
}

const GAP: f32 = 10.0;
const LABEL_H: f32 = 36.0;

pub fn draw(ctx: &egui::Context, st: &mut BrowserState, m: &BrowserModel<'_>) -> BrowserOut {
    let mut out = BrowserOut { grid_cols: 1, grid_rows_visible: 1, ..Default::default() };
    // 직전 프레임에 보이던 썸네일은 해제하지 않는다
    let keep: HashSet<u64> = m.entries.get(st.last_visible.clone()).unwrap_or(&[]).iter().map(|e| e.id.0).collect();
    st.textures.flush(ctx, 48, &keep);

    let panel = |fill| egui::Frame::new().fill(fill);
    ctx.root_ui(|ui| {
        egui::Panel::top("iv-menu").frame(panel(theme::BG_PANEL).inner_margin(Margin::symmetric(8, 2))).show(ui, |ui| menu_bar(ui, st, m, &mut out));
        egui::Panel::top("iv-toolbar")
            .frame(panel(theme::BG_PANEL).inner_margin(Margin::symmetric(10, 6)).stroke(Stroke::new(1.0, theme::BORDER)))
            .exact_size(theme::TOOLBAR_H)
            .show(ui, |ui| toolbar(ui, st, m, &mut out));
        egui::Panel::bottom("iv-status")
            .frame(panel(theme::BG_PANEL).inner_margin(Margin::symmetric(12, 4)).stroke(Stroke::new(1.0, theme::BORDER)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // 오른쪽(스캔 상태)을 먼저 놓고 남은 폭에 왼쪽 글을 한 줄로 (넘치면 말줄임)
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(m.scan_status).size(theme::FONT_SMALL).color(theme::TEXT_FAINT));
                        ui.add_space(16.0);
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(egui::Label::new(RichText::new(m.status).size(theme::FONT_SMALL).color(theme::TEXT_DIM)).truncate()).on_hover_text(m.status);
                        });
                    });
                });
            });
        egui::Panel::left("iv-side")
            .frame(panel(theme::BG_PANEL).stroke(Stroke::new(1.0, theme::BORDER)))
            .resizable(true)
            .default_size(250.0)
            .size_range(170.0..=520.0)
            .show(ui, |ui| folder_tree(ui, st, m, &mut out));
        if st.inspector {
            // 바탕은 inspector() 가 미리보기 칸만 비우고 직접 칠한다 (렌더러가 그 칸 아래에 이미지를 그린다)
            egui::Panel::right("iv-inspector")
                .frame(egui::Frame::NONE)
                .resizable(true)
                .default_size(290.0)
                .size_range(220.0..=560.0)
                .show(ui, |ui| inspector(ui, st, m, &mut out));
        }
        egui::CentralPanel::default().frame(panel(theme::BG_CANVAS)).show(ui, |ui| grid(ui, st, m, &mut out));
    });

    //#region 끄는 중 표시 — 커서 옆에 "N장 옮기기"
    if let Some(p) = egui::DragAndDrop::payload::<DragPhotos>(ctx) {
        if let Some(pos) = ctx.pointer_latest_pos() {
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("iv-drag-ghost")));
            let text = format!("{}{} 옮기기 — 왼쪽 폴더에 놓으세요", p.count, m.kind.unit());
            let galley = painter.layout_no_wrap(text, FontId::proportional(theme::FONT_BODY), Color32::WHITE);
            let r = Rect::from_min_size(pos + Vec2::new(14.0, 10.0), galley.size() + Vec2::new(18.0, 10.0));
            painter.rect_filled(r, CornerRadius::same(7), theme::ACCENT_BG);
            painter.galley(r.min + Vec2::new(9.0, 5.0), galley, Color32::WHITE);
        }
    }
    //#endregion
    //#region 이름 입력 대화 상자
    if let Some(d) = &mut st.dialog {
        match dialog::draw(ctx, d) {
            DialogResult::Open => {}
            DialogResult::Cancel => st.dialog = None,
            DialogResult::Done(a) => {
                out.actions.push(a);
                st.dialog = None;
            }
        }
    }
    //#endregion
    st.last_visible = out.visible.clone();
    // 위젯 포커스가 단축키(화살표·Enter)를 삼키지 않게: 글 입력 중이 아니면 포커스를 놓는다
    if !ctx.text_edit_focused() {
        ctx.memory_mut(|mem| {
            if let Some(id) = mem.focused() {
                mem.surrender_focus(id);
            }
        });
    }
    out
}

fn menu_bar(ui: &mut egui::Ui, st: &mut BrowserState, m: &BrowserModel<'_>, out: &mut BrowserOut) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("파일", |ui| {
            if ui.add_enabled(m.dir.is_some(), egui::Button::new("새 폴더…").shortcut_text("⇧⌘N")).clicked() {
                if let Some(d) = m.dir {
                    st.begin_new_folder(d);
                }
            }
            let cur_path = m.selected.and_then(|i| m.entries.get(i)).and_then(|e| e.path());
            if ui.add_enabled(cur_path.is_some(), egui::Button::new("사진 이름 바꾸기…").shortcut_text("F2")).clicked() {
                if let Some(p) = cur_path {
                    st.begin_rename_file(p);
                }
            }
            if ui.add_enabled(m.dir.is_some(), egui::Button::new("폴더 이름 바꾸기…")).clicked() {
                if let Some(d) = m.dir {
                    st.dialog = Some(Dialog::rename_folder(d.to_path_buf()));
                }
            }
            ui.separator();
            if ui.add(egui::Button::new("상위 폴더").shortcut_text("⌫")).clicked() {
                out.actions.push(Action::ParentFolder);
            }
            if ui.add(egui::Button::new("새로고침").shortcut_text("⌘R")).clicked() {
                out.actions.push(Action::Refresh);
            }
            ui.separator();
            if ui.add_enabled(!m.marked.is_empty(), egui::Button::new(trash_label(m)).shortcut_text("⌘⌫")).clicked() {
                out.actions.push(Action::Trash);
            }
            ui.separator();
            if ui.add(egui::Button::new("끝내기").shortcut_text("⌘Q")).clicked() {
                out.actions.push(Action::Quit);
            }
        });
        ui.menu_button("편집", |ui| {
            let some = !m.marked.is_empty();
            clip_items(ui, some, m.dir.is_some(), None, out);
            ui.separator();
            if m.kind == iv_core::formats::MediaKind::Image {
                rotate_items(ui, some, out);
                ui.separator();
            }
            if ui.add_enabled(!m.entries.is_empty(), egui::Button::new("모두 선택").shortcut_text("⌘A")).clicked() {
                out.actions.push(Action::SelectAll);
            }
        });
        ui.menu_button("보기", |ui| {
            if ui.add_enabled(m.selected.is_some(), egui::Button::new("전체 보기").shortcut_text("⏎")).clicked() {
                out.actions.push(Action::EnterView);
            }
            ui.separator();
            //#region 추가 코덱 엔진 (FFmpeg) — MKV·WebM·AVI… 재생·썸네일
            match m.codec_engine {
                Some(src) => {
                    ui.add_enabled(false, egui::Button::new(format!("추가 코덱 엔진: {src}")));
                }
                None => {
                    if ui.button("추가 코덱 엔진 받기…").on_hover_text("MKV·WebM·AVI·WMV·FLV 등 재생에 필요 (FFmpeg)").clicked() {
                        out.actions.push(Action::InstallCodecs);
                        ui.close();
                    }
                }
            }
            ui.separator();
            //#endregion
            if ui.checkbox(&mut st.show_empty_folders, "사진 없는 폴더도 보이기").changed() {
                out.tree_changed = true;
            }
            ui.separator();
            for (label, t) in [("썸네일 작게", 96.0), ("썸네일 보통", 128.0), ("썸네일 크게", 176.0)] {
                if ui.selectable_label((st.tile - t).abs() < 1.0, label).clicked() {
                    st.tile = t;
                }
            }
        });
        ui.menu_button("정렬", |ui| sort_items(ui, m, out));
    });
}

/// 복사·잘라내기·붙여넣기·경로 복사 메뉴 항목 (`paste_to` = 붙여넣을 폴더, None 이면 열린 폴더)
fn clip_items(ui: &mut egui::Ui, some: bool, can_paste: bool, paste_to: Option<&Path>, out: &mut BrowserOut) {
    if ui.add_enabled(some, egui::Button::new("복사").shortcut_text("⌘C")).clicked() {
        out.actions.push(Action::Copy);
        ui.close();
    }
    if ui.add_enabled(some, egui::Button::new("잘라내기").shortcut_text("⌘X")).clicked() {
        out.actions.push(Action::Cut);
        ui.close();
    }
    if ui.add_enabled(can_paste, egui::Button::new("붙여넣기").shortcut_text("⌘V")).clicked() {
        out.actions.push(Action::Paste(paste_to.map(Path::to_path_buf)));
        ui.close();
    }
    if ui.add(egui::Button::new("경로 복사").shortcut_text("⌥⌘C")).clicked() {
        out.actions.push(Action::CopyPath);
        ui.close();
    }
}

/// 선택한 사진 일괄 회전 (바로 저장 — 방향 태그만)
fn rotate_items(ui: &mut egui::Ui, some: bool, out: &mut BrowserOut) {
    for (label, q) in [("왼쪽으로 90° 돌리기", 3), ("오른쪽으로 90° 돌리기", 1), ("180° 돌리기", 2)] {
        if ui.add_enabled(some, egui::Button::new(label)).on_hover_text("원본에 바로 저장 — 화질 손실 없음").clicked() {
            out.actions.push(Action::RotateMarked(q));
            ui.close();
        }
    }
}

/// "휴지통으로 이동" — 여러 장이면 장 수를 붙인다
fn trash_label(m: &BrowserModel<'_>) -> String {
    match m.marked.len() {
        n if n > 1 => format!("휴지통으로 이동 ({n}{})", m.kind.unit()),
        _ => "휴지통으로 이동".to_string(),
    }
}

const SORTS: [(SortKey, &str); 4] = [(SortKey::Name, "이름"), (SortKey::Modified, "수정한 날짜"), (SortKey::Size, "크기"), (SortKey::Type, "형식")];

fn sort_items(ui: &mut egui::Ui, m: &BrowserModel<'_>, out: &mut BrowserOut) {
    let (cur, asc) = m.sort;
    for (k, label) in SORTS {
        if ui.selectable_label(cur == k, label).clicked() {
            out.actions.push(Action::Sort(k, asc));
        }
    }
    ui.separator();
    if ui.selectable_label(asc, "오름차순").clicked() {
        out.actions.push(Action::Sort(cur, true));
    }
    if ui.selectable_label(!asc, "내림차순").clicked() {
        out.actions.push(Action::Sort(cur, false));
    }
}

fn toolbar(ui: &mut egui::Ui, st: &mut BrowserState, m: &BrowserModel<'_>, out: &mut BrowserOut) {
    use iv_core::formats::MediaKind;
    ui.horizontal_centered(|ui| {
        //#region 왼쪽 — 종류 전환(사진·비디오) · 상위 · 새로고침
        segmented_kind(ui, m, out);
        ui.add_space(6.0);
        if icon_button(ui, Icon::Up, "상위 폴더 (⌫)", m.dir.is_some_and(|d| d.parent().is_some()), false).clicked() {
            out.actions.push(Action::ParentFolder);
        }
        if icon_button(ui, Icon::Refresh, "새로고침 (⌘R)", m.dir.is_some(), false).clicked() {
            out.actions.push(Action::Refresh);
        }
        ui.add_space(4.0);
        //#endregion
        //#region 오른쪽 — 오른쪽부터 거꾸로 쌓고, 남은 폭을 경로 표시가 쓴다
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_toggle(ui, Icon::Inspector, "정보 패널 보이기·숨기기", true, false, st.inspector).clicked() {
                st.inspector = !st.inspector;
            }
            toolbar_sep(ui);
            let some = !m.marked.is_empty();
            if icon_button(ui, Icon::Trash, "휴지통으로 이동 (⌘⌫)", some, false).clicked() {
                out.actions.push(Action::Trash);
            }
            if m.kind == MediaKind::Image {
                if icon_button(ui, Icon::RotateRight, "오른쪽으로 90° 돌려 저장", some, false).clicked() {
                    out.actions.push(Action::RotateMarked(1));
                }
                if icon_button(ui, Icon::RotateLeft, "왼쪽으로 90° 돌려 저장", some, false).clicked() {
                    out.actions.push(Action::RotateMarked(3));
                }
            }
            if icon_button(ui, Icon::Expand, "크게 보기 (Enter · 더블클릭)", m.selected.is_some(), false).clicked() {
                out.actions.push(Action::EnterView);
            }
            toolbar_sep(ui);
            // 썸네일 크기
            ui.add(egui::Slider::new(&mut st.tile, TILE_MIN..=TILE_MAX).show_value(false)).on_hover_text("썸네일 크기");
            paint_small_icon(ui, Icon::Grid);
            toolbar_sep(ui);
            let (cur, asc) = m.sort;
            let name = SORTS.iter().find(|(k, _)| *k == cur).map(|(_, n)| *n).unwrap_or("");
            egui::ComboBox::from_id_salt("iv-sort").width(118.0).selected_text(format!("{name} {}", if asc { "↑" } else { "↓" })).show_ui(ui, |ui| sort_items(ui, m, out)).response.on_hover_text("정렬");
            filter_menu(ui, m, out);
            toolbar_sep(ui);
            if icon_button(ui, Icon::Copy, &copy_path_tip(m), m.dir.is_some(), false).clicked() {
                out.actions.push(Action::CopyPath);
            }
            let w = ui.available_width();
            ui.allocate_ui_with_layout(Vec2::new(w, 28.0), egui::Layout::left_to_right(egui::Align::Center), |ui| breadcrumb(ui, st, m, out));
        });
        //#endregion
    });
}

fn toolbar_sep(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(9.0, 20.0), Sense::hover());
    ui.painter().line_segment([r.center_top(), r.center_bottom()], Stroke::new(1.0, theme::BORDER));
}

fn paint_small_icon(ui: &mut egui::Ui, icon: Icon) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(16.0, 16.0), Sense::hover());
    crate::icons::paint(ui.painter(), icon, r.shrink(1.5), theme::TEXT_FAINT);
}

fn copy_path_tip(m: &BrowserModel<'_>) -> String {
    match m.marked.len() {
        0 => "열린 폴더 경로 복사 (Option+⌘C)".to_string(),
        1 => "선택한 파일의 절대 경로 복사 (Option+⌘C)".to_string(),
        n => format!("선택한 {n}개의 절대 경로 복사 — 한 줄에 하나 (Option+⌘C)"),
    }
}

/// 사진 · 비디오 — 붙어 있는 두 칸 버튼
fn segmented_kind(ui: &mut egui::Ui, m: &BrowserModel<'_>, out: &mut BrowserOut) {
    egui::Frame::new().fill(theme::BG_CANVAS).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(2)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (i, k) in iv_core::formats::MediaKind::ALL.into_iter().enumerate() {
            let on = m.kind == k;
            let icon = if k == iv_core::formats::MediaKind::Image { Icon::Photo } else { Icon::Video };
            let galley = ui.painter().layout_no_wrap(k.label().to_string(), FontId::proportional(theme::FONT_BODY), Color32::WHITE);
            let size = Vec2::new(galley.size().x + 40.0, 26.0);
            let (r, resp) = ui.allocate_exact_size(size, Sense::click());
            let bg = if on { theme::ACCENT_BG } else if resp.hovered() { theme::HOVER } else { Color32::TRANSPARENT };
            ui.painter().rect_filled(r, CornerRadius::same(6), bg);
            let fg = if on { Color32::WHITE } else { theme::TEXT_DIM };
            crate::icons::paint(ui.painter(), icon, Rect::from_center_size(Pos2::new(r.min.x + 17.0, r.center().y), Vec2::splat(14.0)), fg);
            ui.painter().galley(Pos2::new(r.min.x + 30.0, r.center().y - galley.size().y / 2.0), galley, fg);
            if resp.on_hover_text(format!("{} 보기 (⌘{})", k.label(), i + 1)).clicked() && !on {
                out.actions.push(Action::SetKind(k));
            }
        }
    });
}

//#region 경로 표시 — 폴더 이름을 누르면 그 폴더로, 빈 곳을 누르면 글로 고쳐 쓰기
fn breadcrumb(ui: &mut egui::Ui, st: &mut BrowserState, m: &BrowserModel<'_>, out: &mut BrowserOut) {
    let dir_s = m.dir.map(|d| d.display().to_string()).unwrap_or_default();
    if st.path_edit_for.as_deref() != m.dir {
        st.path_edit = dir_s.clone();
        st.path_edit_for = m.dir.map(Path::to_path_buf);
        st.path_editing = false;
    }
    let full = Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), 28.0));
    if st.path_editing {
        let r = ui.add_sized(full.size(), egui::TextEdit::singleline(&mut st.path_edit).hint_text("폴더 경로"));
        if !r.has_focus() && !r.lost_focus() {
            r.request_focus();
        }
        if r.lost_focus() {
            let p = PathBuf::from(st.path_edit.trim());
            // Enter 로 확정했고 폴더가 있으면 이동, 아니면 되돌린다
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) && p.is_dir() {
                out.actions.push(Action::OpenFolder(p));
            } else {
                st.path_edit = dir_s;
            }
            st.path_editing = false;
        }
        return;
    }
    // 바탕 (빈 곳 클릭 = 고쳐 쓰기) — 조각 버튼보다 먼저 등록해 아래에 깔린다
    let bg = ui.interact(full, ui.scope_id().with("iv-crumb-bg"), Sense::click());
    ui.painter().rect_filled(full, CornerRadius::same(7), if bg.hovered() { theme::CONTROL } else { theme::BG_CANVAS });
    if bg.on_hover_text("경로를 글로 입력하려면 누르세요").clicked() {
        st.path_editing = true;
    }
    let Some(dir) = m.dir else { return };
    // 보일 조각: 뒤에서부터 폭이 될 때까지 (넘치면 앞을 … 로)
    let parts: Vec<PathBuf> = dir.ancestors().map(Path::to_path_buf).collect::<Vec<_>>().into_iter().rev().collect();
    let label = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".to_string());
    let font = FontId::proportional(theme::FONT_BODY);
    let avail = full.width() - 40.0;
    let mut used = 0.0;
    let mut first = parts.len();
    for (i, p) in parts.iter().enumerate().rev() {
        let w = ui.painter().layout_no_wrap(label(p), font.clone(), theme::TEXT).size().x + 14.0;
        if used + w > avail && i + 1 < parts.len() {
            break;
        }
        used += w;
        first = i;
    }
    ui.add_space(8.0);
    paint_small_icon(ui, Icon::Folder);
    ui.spacing_mut().item_spacing.x = 0.0;
    if first > 0 && ui.add(egui::Button::new(RichText::new("…").color(theme::TEXT_DIM)).frame(false)).on_hover_text(parts[first - 1].display().to_string()).clicked() {
        out.actions.push(Action::OpenFolder(parts[first - 1].clone()));
    }
    for (i, p) in parts.iter().enumerate().skip(first) {
        if i > first || first > 0 {
            let (r, _) = ui.allocate_exact_size(Vec2::new(12.0, 16.0), Sense::hover());
            crate::icons::paint(ui.painter(), Icon::ChevronRight, r.shrink2(Vec2::new(2.0, 4.0)), theme::TEXT_FAINT);
        }
        let last = i + 1 == parts.len();
        let text = RichText::new(label(p)).color(if last { theme::TEXT } else { theme::TEXT_DIM });
        let text = if last { text.strong() } else { text };
        if ui.add(egui::Button::new(text).frame(false)).on_hover_text(p.display().to_string()).clicked() && !last {
            out.actions.push(Action::OpenFolder(p.clone()));
        }
    }
}
//#endregion

//#region 확장자 필터 — 형식 묶음별 체크, 고른 것의 사진·폴더만 보인다
fn filter_menu(ui: &mut egui::Ui, m: &BrowserModel<'_>, out: &mut BrowserOut) {
    use egui::containers::menu::{MenuButton, MenuConfig};
    let on = |g: &iv_core::formats::FormatGroup| m.filter.is_some_and(|f| g.exts.iter().any(|e| f.contains(e)));
    let chosen: Vec<&str> = m.groups.iter().filter(|g| on(g)).map(|g| g.name).collect();
    // 필터가 걸려 있으면 눈에 띄게 (폴더·사진이 사라진 것처럼 보이지 않게)
    let label = if chosen.is_empty() {
        RichText::new("필터: 전체")
    } else {
        RichText::new(format!("필터: {}", chosen.join("·"))).color(Color32::from_rgb(255, 200, 90)).strong()
    };
    let button = egui::Button::new(label).selected(!chosen.is_empty());
    let config = MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    MenuButton::from_button(button).config(config).ui(ui, |ui| {
        ui.label(RichText::new(format!("고른 형식의 {0}과(와) 그 {0}이(가) 든 폴더만 보입니다", m.kind.label())).size(11.0).color(Color32::from_gray(150)));
        ui.separator();
        let mut changed = None;
        for (i, g) in m.groups.iter().enumerate() {
            let mut checked = on(g);
            let tip = g.exts.join(", ");
            if ui.checkbox(&mut checked, g.name).on_hover_text(tip).changed() {
                changed = Some(i);
            }
        }
        if let Some(i) = changed {
            let exts: Vec<&'static str> = m.groups.iter().enumerate().filter(|(k, g)| if *k == i { !on(g) } else { on(g) }).flat_map(|(_, g)| g.exts.iter().copied()).collect();
            out.actions.push(Action::SetFilter(Some(exts)));
        }
        ui.separator();
        if ui.add_enabled(!chosen.is_empty(), egui::Button::new("필터 해제 — 모두 보기")).clicked() {
            out.actions.push(Action::SetFilter(None));
            ui.close();
        }
    });
}
//#endregion

fn folder_tree(ui: &mut egui::Ui, st: &mut BrowserState, m: &BrowserModel<'_>, out: &mut BrowserOut) {
    ui.add_space(4.0);
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        let mut section = None;
        for (k, row) in m.tree.iter().enumerate() {
            //#region 구역 제목 — Finder 처럼 즐겨찾기 · 위치 (구역이 바뀌는 루트 앞에)
            if let Some(rk) = row.root {
                let sec = rk.section();
                if section != Some(sec) {
                    section = Some(sec);
                    ui.add_space(if k == 0 { 4.0 } else { 10.0 });
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        let title = match sec {
                            iv_fstree::Section::Favorites => "즐겨찾기",
                            iv_fstree::Section::Locations => "위치",
                        };
                        ui.label(RichText::new(title).size(theme::FONT_CAPTION).strong().color(theme::TEXT_FAINT));
                    });
                    ui.add_space(2.0);
                }
            }
            //#endregion
            // 줄 id 를 경로로 고정 — 스캔으로 위 줄이 숨겨져 줄이 밀려도 누르던 줄의 클릭이 사라지지 않게
            ui.push_id(&row.path, |ui| {
                let w = ui.available_width();
                let (row_r, r) = ui.allocate_exact_size(Vec2::new(w, 24.0), Sense::click());
                let row_r = row_r.shrink2(Vec2::new(6.0, 0.0));
                let x0 = row_r.min.x + 4.0 + row.depth as f32 * 14.0;
                let counted = m.tree_counts.get(k).copied().flatten();
                let current = m.dir == Some(row.path.as_path());
                //#region 펼침 삼각형 — 스캔으로 보일 하위가 없다고 알면 뺀다
                let arrow_r = Rect::from_min_size(Pos2::new(x0, row_r.center().y - 8.0), Vec2::splat(16.0));
                let has_arrow = row.has_children && counted.is_none_or(|(_, f)| f > 0);
                let arrow = has_arrow.then(|| ui.interact(arrow_r, ui.scope_id().with("arrow"), Sense::click()));
                //#endregion
                let bg = if current {
                    theme::ACCENT_BG
                } else if r.hovered() || arrow.as_ref().is_some_and(|a| a.hovered()) {
                    theme::HOVER
                } else {
                    Color32::TRANSPARENT
                };
                let p = ui.painter();
                p.rect_filled(row_r, CornerRadius::same(5), bg);
                let fg = if current { Color32::WHITE } else { theme::TEXT };
                if has_arrow {
                    let c = arrow_r.center();
                    let s = 3.5;
                    let pts = if row.expanded { vec![c + Vec2::new(-s, -s * 0.5), c + Vec2::new(0.0, s * 0.6), c + Vec2::new(s, -s * 0.5)] } else { vec![c + Vec2::new(-s * 0.5, -s), c + Vec2::new(s * 0.6, 0.0), c + Vec2::new(-s * 0.5, s)] };
                    p.add(egui::Shape::line(pts, Stroke::new(1.5, if current { Color32::WHITE } else { theme::TEXT_DIM })));
                }
                let icon = match row.root {
                    Some(iv_fstree::RootKind::Home) => Icon::Home,
                    Some(iv_fstree::RootKind::Desktop) => Icon::Desktop,
                    Some(iv_fstree::RootKind::Documents) => Icon::Document,
                    Some(iv_fstree::RootKind::Downloads) => Icon::Download,
                    Some(iv_fstree::RootKind::Pictures) => Icon::Photo,
                    Some(iv_fstree::RootKind::Movies) => Icon::Video,
                    Some(iv_fstree::RootKind::Cloud) => Icon::Cloud,
                    Some(iv_fstree::RootKind::Disk) => Icon::Disk,
                    _ => Icon::Folder,
                };
                crate::icons::paint(p, icon, Rect::from_min_size(Pos2::new(x0 + 18.0, row_r.center().y - 7.0), Vec2::splat(14.0)), if current { Color32::WHITE } else { theme::ACCENT });
                //#region 이름 (말줄임) + 오른쪽 숫자
                let mut tail = String::new();
                if let Some((photos, folders)) = counted {
                    if photos > 0 {
                        tail = format!("{photos}");
                    } else if folders > 0 {
                        tail = String::new();
                    }
                }
                let tail_g = p.layout_no_wrap(tail.clone(), FontId::proportional(theme::FONT_SMALL), if current { Color32::from_white_alpha(200) } else { theme::TEXT_FAINT });
                let name_x = x0 + 38.0;
                let name_w = (row_r.max.x - 8.0 - tail_g.size().x - 8.0 - name_x).max(20.0);
                let mut job = egui::text::LayoutJob::simple_singleline(row.name.clone(), FontId::proportional(theme::FONT_BODY), fg);
                job.wrap = egui::text::TextWrapping::truncate_at_width(name_w);
                let g = p.layout_job(job);
                p.galley(Pos2::new(name_x, row_r.center().y - g.size().y / 2.0), g, fg);
                p.galley(Pos2::new(row_r.max.x - 8.0 - tail_g.size().x, row_r.center().y - tail_g.size().y / 2.0), tail_g, theme::TEXT_FAINT);
                //#endregion
                if let Some((photos, folders)) = counted {
                    r.clone().on_hover_text(format!("{} {photos}{} · 하위 폴더 {folders}", m.kind.label(), m.kind.unit()));
                }
                //#region 놓기 — 끌고 있는 사진을 이 폴더로 옮긴다 (열린 폴더 자신은 제외)
                if !current {
                    if r.dnd_hover_payload::<DragPhotos>().is_some() {
                        ui.painter().rect_stroke(row_r, CornerRadius::same(5), Stroke::new(2.0, theme::ACCENT), StrokeKind::Inside);
                    }
                    if r.dnd_release_payload::<DragPhotos>().is_some() {
                        out.actions.push(Action::MoveMarkedTo(row.path.clone()));
                    }
                }
                //#endregion
                if arrow.as_ref().is_some_and(|a| a.clicked()) {
                    out.tree_toggle = Some(row.path.clone());
                } else if r.clicked() && !current {
                    out.actions.push(Action::OpenFolder(row.path.clone()));
                }
                if r.double_clicked() && row.has_children {
                    out.tree_toggle = Some(row.path.clone());
                }
                //#region 오른쪽 클릭 — 새 폴더 · 이름 바꾸기 (루트는 이름을 바꾸지 않는다) · 붙여넣기 · 경로 복사
                r.context_menu(|ui| {
                    if ui.button("새 폴더…").clicked() {
                        st.dialog = Some(Dialog::new_folder(row.path.clone()));
                        ui.close();
                    }
                    if ui.add_enabled(row.depth > 0, egui::Button::new("이름 바꾸기…")).clicked() {
                        st.dialog = Some(Dialog::rename_folder(row.path.clone()));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("여기에 붙여넣기").clicked() {
                        out.actions.push(Action::Paste(Some(row.path.clone())));
                        ui.close();
                    }
                    if ui.button("경로 복사").clicked() {
                        out.copy_text = Some(row.path.display().to_string());
                        ui.close();
                    }
                });
                //#endregion
            });
        }
    });
}

//#region 정보 패널 — 미리보기(렌더러가 그림) + 파일 정보
fn inspector(ui: &mut egui::Ui, st: &mut BrowserState, m: &BrowserModel<'_>, out: &mut BrowserOut) {
    let w = ui.available_width();
    let pad = 12.0;
    // 미리보기 칸 — 투명 (렌더러가 아래에 이미지를 그린다)
    let top = ui.cursor().min;
    let rect = Rect::from_min_size(top + Vec2::new(pad, pad), Vec2::new(w - pad * 2.0, ((w - pad * 2.0) * 0.8).clamp(140.0, 420.0)));
    //#region 패널 바탕 — 미리보기 칸을 뺀 네 조각 + 왼쪽 경계선
    let all = ui.max_rect();
    let bgp = ui.painter();
    for r in [
        Rect::from_min_max(all.min, Pos2::new(all.max.x, rect.min.y)),
        Rect::from_min_max(Pos2::new(all.min.x, rect.max.y), all.max),
        Rect::from_min_max(Pos2::new(all.min.x, rect.min.y), Pos2::new(rect.min.x, rect.max.y)),
        Rect::from_min_max(Pos2::new(rect.max.x, rect.min.y), Pos2::new(all.max.x, rect.max.y)),
    ] {
        bgp.rect_filled(r, CornerRadius::ZERO, theme::BG_PANEL);
    }
    bgp.line_segment([all.left_top(), all.left_bottom()], Stroke::new(1.0, theme::BORDER));
    //#endregion
    ui.allocate_rect(Rect::from_min_max(top, Pos2::new(top.x + w, rect.max.y)), Sense::hover());
    out.preview_rect = Some(rect);
    let painter = ui.painter();
    painter.rect_stroke(rect.expand(1.0), CornerRadius::same(4), Stroke::new(1.0, theme::BORDER), StrokeKind::Outside);
    let cur = m.selected.and_then(|i| m.entries.get(i));
    // 프레임이 아직 없으면 썸네일로 채워 깜박임을 없앤다
    if !m.preview_has_frame {
        match cur.and_then(|e| st.textures.get(e.id.0)) {
            Some(tex) => {
                let r = fit_rect(rect.shrink(4.0), tex.size_vec2(), false);
                painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                let text = match cur {
                    Some(e) if m.failed.contains(&e.id.0) => "미리보기를 만들 수 없음",
                    Some(_) => "",
                    None => "미리보기",
                };
                painter.rect_filled(rect, CornerRadius::same(4), theme::BG_CANVAS);
                painter.text(rect.center(), Align2::CENTER_CENTER, text, FontId::proportional(theme::FONT_SMALL), theme::TEXT_FAINT);
            }
        }
    }
    ui.add_space(10.0);
    egui::Frame::new().inner_margin(Margin::symmetric(pad as i8, 0)).show(ui, |ui| {
        let Some(e) = cur else {
            ui.label(RichText::new("선택한 항목 없음").color(theme::TEXT_FAINT));
            return;
        };
        if m.marked.len() > 1 {
            let bytes: u64 = m.entries.iter().filter(|x| m.marked.contains(&x.id)).map(|x| x.len).sum();
            egui::Frame::new().fill(theme::ACCENT_BG).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(8, 4)).show(ui, |ui| {
                ui.label(RichText::new(format!("{}{} 선택 · {}", m.marked.len(), m.kind.unit(), iv_viewer::human_bytes(bytes))).color(Color32::WHITE).size(theme::FONT_SMALL));
            });
            ui.add_space(6.0);
        }
        ui.add(egui::Label::new(RichText::new(&e.name).size(14.0).strong().color(theme::TEXT)).wrap());
        ui.add_space(6.0);
        let ext = e.path().and_then(|p| p.extension()).map(|x| x.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let group = m.groups.iter().find(|g| g.exts.iter().any(|x| *x == ext)).map_or_else(|| ext.to_uppercase(), |g| g.name.to_string());
        let mut rows: Vec<(&str, String)> = vec![("형식", group)];
        if let Some([w, h]) = st.textures.full_size(e.id.0) {
            rows.push(("크기", format!("{w} × {h}")));
            let mp = w as f64 * h as f64 / 1e6;
            if mp >= 0.1 {
                rows.push(("화소", format!("{mp:.1} MP")));
            }
        }
        rows.push(("용량", iv_viewer::human_bytes(e.len)));
        rows.push(("수정일", format_time(e.mtime)));
        egui::Grid::new("iv-inspect").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            for (k, v) in rows {
                ui.label(RichText::new(k).size(theme::FONT_SMALL).color(theme::TEXT_FAINT));
                ui.label(RichText::new(v).size(theme::FONT_SMALL).color(theme::TEXT));
                ui.end_row();
            }
        });
        ui.add_space(8.0);
        ui.label(RichText::new("위치").size(theme::FONT_SMALL).color(theme::TEXT_FAINT));
        if let Some(dir) = m.dir {
            ui.add(egui::Label::new(RichText::new(dir.display().to_string()).size(theme::FONT_SMALL).color(theme::TEXT_DIM)).wrap());
        }
        ui.add_space(8.0);
        if ui.add(egui::Button::new(RichText::new("경로 복사").size(theme::FONT_SMALL))).on_hover_text("이 파일의 절대 경로를 클립보드에").clicked() {
            if let Some(p) = e.path() {
                out.copy_text = Some(p.display().to_string());
            }
        }
    });
}

/// 수정 시각 → "2026-11-20 13:45" (현지 시각)
fn format_time(t: std::time::SystemTime) -> String {
    let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) else { return "-".into() };
    let secs = d.as_secs() as i64 + local_offset_secs(d.as_secs() as i64);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // 1970-01-01 기준 일수 → 날짜 (Howard Hinnant civil_from_days)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year}-{month:02}-{day:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

#[cfg(unix)]
fn local_offset_secs(t: i64) -> i64 {
    // SAFETY: localtime_r 는 넘긴 버퍼에만 쓴다
    unsafe {
        let tt = t as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&tt, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff
    }
}

#[cfg(not(unix))]
fn local_offset_secs(_t: i64) -> i64 {
    0
}
//#endregion

fn grid(ui: &mut egui::Ui, st: &mut BrowserState, m: &BrowserModel<'_>, out: &mut BrowserOut) {
    let n = m.entries.len();
    if n == 0 {
        let r = ui.centered_and_justified(|ui| {
            let text = match (m.dir.is_some(), m.filter.is_some()) {
                (true, true) => format!("필터에 맞는 {}이(가) 없습니다 — 위 '필터' 에서 해제할 수 있습니다", m.kind.label()),
                (true, false) => format!("이 폴더에는 {}이(가) 없습니다", m.kind.label()),
                _ => "왼쪽에서 폴더를 고르세요".to_string(),
            };
            ui.label(RichText::new(text).color(Color32::from_gray(150)));
        });
        if m.dir.is_some() {
            ui.interact(r.response.rect, ui.scope_id().with("iv-empty-bg"), Sense::click()).context_menu(|ui| background_menu(ui, st, m, out));
        }
        return;
    }
    let tile = st.tile.clamp(TILE_MIN, TILE_MAX);
    let avail_w = ui.available_width() - 14.0; // 스크롤바 자리
    let pitch_x = tile + GAP;
    let pitch_y = tile + LABEL_H + GAP;
    let cols = (((avail_w + GAP) / pitch_x).floor() as usize).max(1);
    let rows = n.div_ceil(cols);
    out.grid_cols = cols;
    // 열이 남는 공간은 양옆 여백으로 고르게
    let used = cols as f32 * pitch_x - GAP;
    let left_pad = ((avail_w - used) / 2.0).max(0.0);

    let mut sa = egui::ScrollArea::vertical().auto_shrink(false).id_salt("iv-grid");
    if let Some(i) = st.scroll_to.take() {
        let row_top = (i / cols) as f32 * pitch_y;
        let (off, h) = st.last_view.unwrap_or((0.0, ui.available_height()));
        // 화면 밖일 때만 최소 이동
        let target = if row_top < off {
            Some(row_top)
        } else if row_top + pitch_y > off + h {
            Some(row_top + pitch_y - h)
        } else {
            None
        };
        if let Some(t) = target {
            sa = sa.vertical_scroll_offset(t.max(0.0));
        }
    }
    sa.show_viewport(ui, |ui, vp| {
        ui.set_height(rows as f32 * pitch_y);
        ui.set_width(ui.available_width());
        st.last_view = Some((vp.min.y, vp.height()));
        out.grid_rows_visible = ((vp.height() / pitch_y).floor() as usize).max(1);
        let first = (vp.min.y / pitch_y).floor().max(0.0) as usize;
        let last = ((vp.max.y / pitch_y).ceil() as usize).min(rows);
        out.visible = (first.saturating_sub(2) * cols).min(n)..((last + 2) * cols).min(n);
        let origin = ui.max_rect().min;
        let painter = ui.painter().clone();
        let mods = ui.input(|i| i.modifiers);
        //#region 빈 공간 — 타일보다 먼저 등록해 타일 아래에 깔린다 (타일 위 클릭·끌기는 타일 몫)
        let bg_rect = Rect::from_min_size(origin, Vec2::new(ui.max_rect().width(), (rows as f32 * pitch_y).max(vp.max.y)));
        let bg = ui.interact(bg_rect, ui.scope_id().with("iv-grid-bg"), Sense::CLICK | Sense::DRAG);
        if bg.clicked() && !mods.command && !mods.shift {
            out.actions.push(Action::SelectNone);
        }
        bg.context_menu(|ui| background_menu(ui, st, m, out));
        if bg.drag_started() {
            if let Some(p) = bg.interact_pointer_pos() {
                st.marquee = Some(Marquee { start: p - origin, keep: mods.command || mods.shift, last: Vec::new(), sent_begin: false });
            }
        }
        //#endregion
        for r in first..last {
            for c in 0..cols {
                let i = r * cols + c;
                let Some(e) = m.entries.get(i) else { break };
                let min = origin + Vec2::new(left_pad + c as f32 * pitch_x, r as f32 * pitch_y);
                let cell = Rect::from_min_size(min, Vec2::new(tile, tile + LABEL_H));
                let resp = ui.interact(cell, ui.scope_id().with(("iv-thumb", e.id.0)), Sense::CLICK | Sense::DRAG);
                //#region 끌기 시작 — 선택에 없는 사진을 끌면 그 사진만 고르고, 선택된 사진을 끌면 선택 전부
                if resp.drag_started() {
                    let marked = m.marked.contains(&e.id);
                    if !marked {
                        out.actions.push(Action::Select(i));
                    }
                    resp.dnd_set_drag_payload(DragPhotos { count: if marked { m.marked.len() } else { 1 } });
                }
                //#endregion
                // 체크 상자는 칸보다 나중에 등록 → 위에 놓여 그 자리 클릭을 가져간다
                let check = Rect::from_min_size(cell.min + Vec2::splat(6.0), Vec2::splat(CHECK));
                let check_resp = ui.interact(check, ui.scope_id().with(("iv-check", e.id.0)), Sense::CLICK);
                if check_resp.clicked() {
                    out.actions.push(Action::ToggleSelect(i));
                }
                //#region 클릭 — ⌘ 토글 · ⇧ 범위 · 그냥 클릭은 한 장만 (이미 현재 항목이어도 여러 장 선택을 푼다)
                if resp.double_clicked() {
                    out.actions.push(Action::Activate(i));
                } else if resp.clicked() {
                    if mods.command {
                        out.actions.push(Action::ToggleSelect(i));
                    } else if mods.shift {
                        out.actions.push(Action::RangeSelect(i));
                    } else if m.selected != Some(i) || m.marked.len() > 1 || !m.marked.contains(&e.id) {
                        out.actions.push(Action::Select(i));
                    }
                }
                //#endregion
                let marked = m.marked.contains(&e.id);
                //#region 오른쪽 클릭 — 이 사진 기준 (선택에 없으면 이 사진만 고른다)
                resp.context_menu(|ui| {
                    let marked = m.marked.contains(&e.id);
                    if ui.button("전체 보기").clicked() {
                        out.actions.push(Action::Activate(i));
                        ui.close();
                    }
                    if ui.add(egui::Button::new("이름 바꾸기…").shortcut_text("F2")).clicked() {
                        if !marked {
                            out.actions.push(Action::Select(i));
                        }
                        if let Some(p) = e.path() {
                            st.begin_rename_file(p);
                        }
                        ui.close();
                    }
                    ui.separator();
                    // 아래 동작은 선택 기준 — 선택에 없는 사진이면 먼저 그 사진만 고른다
                    let before = out.actions.len();
                    clip_items(ui, true, m.dir.is_some(), None, out);
                    if m.kind == iv_core::formats::MediaKind::Image {
                        ui.separator();
                        rotate_items(ui, true, out);
                    }
                    if out.actions.len() > before && !marked {
                        out.actions.insert(before, Action::Select(i));
                    }
                    ui.separator();
                    let n = if marked { m.marked.len() } else { 1 };
                    if ui.button(if n > 1 { format!("휴지통으로 이동 ({n}장)") } else { "휴지통으로 이동".to_string() }).clicked() {
                        if !marked {
                            out.actions.push(Action::Select(i));
                        }
                        out.actions.push(Action::Trash);
                        ui.close();
                    }
                });
                //#endregion
                let hovered = resp.contains_pointer() || check_resp.contains_pointer();
                draw_tile(&painter, st, m, e, cell, tile, marked, m.selected == Some(i), hovered);
                // 마우스를 올렸거나, 선택됐거나, 여러 장 고르는 중이면 체크 상자를 보인다
                if hovered || marked || m.marked.len() > 1 {
                    draw_check(&painter, check, marked, check_resp.hovered());
                }
            }
        }
        //#region 선택 상자 — 닿는 타일을 고르고, 위·아래 끝에 가면 저절로 스크롤
        if let Some(mq) = &mut st.marquee {
            match ui.ctx().pointer_latest_pos() {
                Some(p) if bg.dragged() => {
                    let a = origin + mq.start;
                    let sel = Rect::from_two_pos(a, p);
                    // 내용 좌표에서 닿는 칸 계산 (화면 밖 줄도 포함)
                    let (c0, c1) = (sel.min - origin, sel.max - origin);
                    let r0 = (c0.y / pitch_y).floor().max(0.0) as usize;
                    let r1 = ((c1.y / pitch_y).floor().max(0.0) as usize).min(rows.saturating_sub(1));
                    let mut hit = Vec::new();
                    for r in r0..=r1 {
                        for c in 0..cols {
                            let i = r * cols + c;
                            if i >= n {
                                break;
                            }
                            let cell = Rect::from_min_size(Pos2::new(left_pad + c as f32 * pitch_x, r as f32 * pitch_y), Vec2::new(tile, tile + LABEL_H));
                            if cell.intersects(Rect::from_min_max(c0.to_pos2(), c1.to_pos2())) {
                                hit.push(i);
                            }
                        }
                    }
                    if !mq.sent_begin || hit != mq.last {
                        out.actions.push(Action::MarqueeSelect { indices: hit.clone(), keep: mq.keep, begin: !mq.sent_begin });
                        mq.sent_begin = true;
                        mq.last = hit;
                    }
                    painter.rect_filled(sel, CornerRadius::same(2), Color32::from_rgba_unmultiplied(70, 130, 220, 40));
                    painter.rect_stroke(sel, CornerRadius::same(2), Stroke::new(1.0, Color32::from_rgb(100, 160, 240)), StrokeKind::Inside);
                    let edge = 30.0;
                    let view = ui.clip_rect();
                    let dy = if p.y < view.min.y + edge { 12.0 } else if p.y > view.max.y - edge { -12.0 } else { 0.0 };
                    if dy != 0.0 {
                        ui.scroll_with_delta(Vec2::new(0.0, dy));
                        ui.ctx().request_repaint();
                    }
                }
                _ => st.marquee = None,
            }
        }
        //#endregion
    });
}

/// 격자 빈 곳 오른쪽 클릭 — 이 폴더에 붙여넣기 · 새 폴더
fn background_menu(ui: &mut egui::Ui, st: &mut BrowserState, m: &BrowserModel<'_>, out: &mut BrowserOut) {
    if ui.add(egui::Button::new("붙여넣기").shortcut_text("⌘V")).clicked() {
        out.actions.push(Action::Paste(None));
        ui.close();
    }
    if ui.add(egui::Button::new("새 폴더…").shortcut_text("⇧⌘N")).clicked() {
        if let Some(d) = m.dir {
            st.begin_new_folder(d);
        }
        ui.close();
    }
    if ui.button("폴더 경로 복사").clicked() {
        out.copy_text = m.dir.map(|d| d.display().to_string());
        ui.close();
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_tile(painter: &egui::Painter, st: &BrowserState, m: &BrowserModel<'_>, e: &Entry, cell: Rect, tile: f32, selected: bool, current: bool, hovered: bool) {
    let bg = if selected {
        theme::SELECTED_BG
    } else if hovered {
        theme::TILE_HOVER
    } else {
        theme::TILE
    };
    painter.rect_filled(cell, CornerRadius::same(8), bg);
    let img_box = Rect::from_min_size(cell.min, Vec2::splat(tile)).shrink(8.0);
    if let Some(tex) = st.textures.get(e.id.0) {
        let r = fit_rect(img_box, tex.size_vec2(), false);
        let cut = e.path().is_some_and(|p| m.cut.contains(p));
        let tint = if cut { Color32::WHITE.gamma_multiply(0.4) } else { Color32::WHITE };
        // 사진 가장자리를 살짝 띄우는 그림자
        painter.rect_filled(r.translate(Vec2::new(0.0, 1.5)), CornerRadius::same(2), Color32::from_black_alpha(90));
        painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), tint);
        //#region 비디오 표시 — 왼쪽 아래 재생 표식
        if m.kind == iv_core::formats::MediaKind::Video {
            let b = Rect::from_min_size(Pos2::new(r.min.x + 5.0, r.max.y - 21.0), Vec2::new(24.0, 16.0));
            painter.rect_filled(b, CornerRadius::same(4), Color32::from_black_alpha(170));
            let c = b.center();
            painter.add(egui::Shape::convex_polygon(vec![c + Vec2::new(-3.0, -4.5), c + Vec2::new(5.0, 0.0), c + Vec2::new(-3.0, 4.5)], Color32::WHITE, Stroke::NONE));
        }
        //#endregion
    } else if m.failed.contains(&e.id.0) {
        painter.text(img_box.center(), Align2::CENTER_CENTER, "열 수 없음", FontId::proportional(theme::FONT_SMALL), theme::ERROR);
    } else {
        painter.rect_filled(img_box.shrink(img_box.width() * 0.22), CornerRadius::same(4), theme::CONTROL);
    }
    // 파일명 (한 줄, 말줄임) + 해상도
    let text_w = cell.width() - 12.0;
    let name_color = if selected { Color32::WHITE } else { theme::TEXT };
    let mut job = egui::text::LayoutJob::simple_singleline(e.name.clone(), FontId::proportional(12.0), name_color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(text_w);
    job.halign = egui::Align::Center;
    let galley = painter.layout_job(job);
    painter.galley(Pos2::new(cell.center().x, cell.min.y + tile + 1.0), galley, name_color);
    if let Some([w, h]) = st.textures.full_size(e.id.0) {
        painter.text(Pos2::new(cell.center().x, cell.min.y + tile + 18.0), Align2::CENTER_TOP, format!("{w}×{h}"), FontId::proportional(theme::FONT_CAPTION), if selected { Color32::from_white_alpha(170) } else { theme::TEXT_FAINT });
    }
    // 테두리: 선택 = 강조색, 현재 항목(선택 아님) = 옅은 선
    if selected {
        painter.rect_stroke(cell, CornerRadius::same(8), Stroke::new(if current { 2.0 } else { 1.0 }, theme::ACCENT), StrokeKind::Inside);
    } else if current {
        painter.rect_stroke(cell, CornerRadius::same(8), Stroke::new(1.0, theme::TEXT_FAINT), StrokeKind::Inside);
    }
}

const CHECK: f32 = 20.0;

/// 썸네일 왼쪽 위 체크 상자 (글꼴에 기대지 않고 선으로 그린다)
fn draw_check(painter: &egui::Painter, r: Rect, checked: bool, hovered: bool) {
    if checked {
        painter.rect_filled(r, CornerRadius::same(4), Color32::from_rgb(52, 120, 220));
        let p = |x: f32, y: f32| Pos2::new(r.min.x + r.width() * x, r.min.y + r.height() * y);
        painter.line_segment([p(0.24, 0.52), p(0.43, 0.70)], Stroke::new(2.2, Color32::WHITE));
        painter.line_segment([p(0.43, 0.70), p(0.77, 0.32)], Stroke::new(2.2, Color32::WHITE));
    } else {
        // 밝은 사진 위에서도 보이게: 어두운 바깥 테두리 + 흰 안쪽 테두리
        painter.rect_filled(r, CornerRadius::same(4), Color32::from_black_alpha(if hovered { 190 } else { 150 }));
        painter.rect_stroke(r.expand(1.0), CornerRadius::same(5), Stroke::new(1.0, Color32::from_black_alpha(160)), StrokeKind::Inside);
        painter.rect_stroke(r, CornerRadius::same(4), Stroke::new(1.5, Color32::from_gray(if hovered { 255 } else { 235 })), StrokeKind::Inside);
    }
}

/// 상자 안에 비율 유지로 맞춤 (`upscale` false 면 원래보다 키우지 않음)
fn fit_rect(bx: Rect, size: Vec2, upscale: bool) -> Rect {
    let mut s = (bx.width() / size.x).min(bx.height() / size.y);
    if !upscale {
        s = s.min(1.0);
    }
    Rect::from_center_size(bx.center(), size * s)
}
