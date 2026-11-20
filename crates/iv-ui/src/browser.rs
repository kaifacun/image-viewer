//! 브라우저 화면 (FastStone 형): 메뉴·도구모음·경로 줄 / 폴더 트리 + 미리보기 칸 / 썸네일 격자 / 상태줄.
//!
//! 그리기만 하고 상태를 바꾸지 않는다 — 사용자의 동작은 [`BrowserOut`] 으로 돌려준다.
//! 미리보기 칸은 **투명**으로 비워 두고 그 사각형을 돌려준다: 앱이 렌더러의 이미지 경로(밉맵)로 그 칸에 그린다.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2};
use iv_core::Frame;
use iv_fstree::Row;
use iv_library::{Entry, EntryId};
use iv_viewer::{Action, SortKey};

use crate::dialog::{self, Dialog, DialogResult};

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
}

impl Default for BrowserState {
    fn default() -> Self {
        Self { textures: ThumbTextures::default(), scroll_to: None, tile: 128.0, path_edit: String::new(), path_edit_for: None, last_view: None, last_visible: 0..0, dialog: None }
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
    /// 미리보기 칸에 렌더러가 실제 프레임을 그리는가 (아니면 썸네일로 대신 채움)
    pub preview_has_frame: bool,
}

#[derive(Default)]
pub struct BrowserOut {
    pub actions: Vec<Action>,
    /// 펼침/접기 전환할 트리 노드
    pub tree_toggle: Option<PathBuf>,
    /// 썸네일을 요청할 항목 범위 (보이는 줄 ±2줄)
    pub visible: Range<usize>,
    /// 미리보기 칸 (점 단위, 화면 좌표)
    pub preview_rect: Option<Rect>,
    pub grid_cols: usize,
    pub grid_rows_visible: usize,
}

const GAP: f32 = 8.0;
const LABEL_H: f32 = 34.0;

pub fn draw(ctx: &egui::Context, st: &mut BrowserState, m: &BrowserModel<'_>) -> BrowserOut {
    let mut out = BrowserOut { grid_cols: 1, grid_rows_visible: 1, ..Default::default() };
    // 직전 프레임에 보이던 썸네일은 해제하지 않는다
    let keep: HashSet<u64> = m.entries.get(st.last_visible.clone()).unwrap_or(&[]).iter().map(|e| e.id.0).collect();
    st.textures.flush(ctx, 48, &keep);

    ctx.root_ui(|ui| {
        egui::Panel::top("iv-menu").show(ui, |ui| menu_bar(ui, st, m, &mut out));
        egui::Panel::top("iv-toolbar").show(ui, |ui| toolbar(ui, st, m, &mut out));
        egui::Panel::bottom("iv-status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(m.status).size(12.5).color(Color32::from_gray(210)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(m.scan_status).size(12.0).color(Color32::from_gray(150)));
                });
            });
        });
        egui::Panel::left("iv-side").frame(egui::Frame::NONE).resizable(true).default_size(260.0).size_range(160.0..=600.0).show(ui, |ui| {
            // 미리보기 칸 — 투명 (렌더러가 아래에 이미지를 그린다)
            egui::Panel::bottom("iv-preview").frame(egui::Frame::NONE).resizable(true).default_size(240.0).size_range(80.0..=(ui.available_height() * 0.6).max(80.0)).show(ui, |ui| {
                let rect = ui.max_rect();
                out.preview_rect = Some(rect);
                let painter = ui.painter_at(rect);
                painter.rect_stroke(rect.shrink(0.5), CornerRadius::ZERO, Stroke::new(1.0, Color32::from_gray(60)), StrokeKind::Inside);
                // 프레임이 아직 없으면 썸네일로 채워 깜박임을 없앤다
                if !m.preview_has_frame {
                    if let Some(tex) = m.selected.and_then(|i| m.entries.get(i)).and_then(|e| st.textures.get(e.id.0)) {
                        let r = fit_rect(rect.shrink(4.0), tex.size_vec2(), false);
                        painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                    }
                }
                ui.allocate_rect(rect, Sense::hover());
            });
            egui::CentralPanel::default().show(ui, |ui| folder_tree(ui, st, m, &mut out));
        });
        egui::CentralPanel::default().show(ui, |ui| grid(ui, st, m, &mut out));
    });

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
            if ui.add_enabled(m.dir.is_some(), egui::Button::new("새 폴더…  ⇧⌘N")).clicked() {
                if let Some(d) = m.dir {
                    st.begin_new_folder(d);
                }
            }
            let cur_path = m.selected.and_then(|i| m.entries.get(i)).and_then(|e| e.path());
            if ui.add_enabled(cur_path.is_some(), egui::Button::new("사진 이름 바꾸기…  F2")).clicked() {
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
            if ui.button("상위 폴더  ⌫").clicked() {
                out.actions.push(Action::ParentFolder);
            }
            if ui.button("새로고침  ⌘R").clicked() {
                out.actions.push(Action::Refresh);
            }
            ui.separator();
            if ui.add_enabled(!m.marked.is_empty(), egui::Button::new(trash_label(m))).clicked() {
                out.actions.push(Action::Trash);
            }
            ui.separator();
            if ui.button("끝내기  ⌘Q").clicked() {
                out.actions.push(Action::Quit);
            }
        });
        ui.menu_button("편집", |ui| {
            if ui.add_enabled(!m.entries.is_empty(), egui::Button::new("모두 선택  ⌘A")).clicked() {
                out.actions.push(Action::SelectAll);
            }
        });
        ui.menu_button("보기", |ui| {
            if ui.add_enabled(m.selected.is_some(), egui::Button::new("전체 보기  ⏎")).clicked() {
                out.actions.push(Action::EnterView);
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

/// "휴지통으로 이동" — 여러 장이면 장 수를 붙인다
fn trash_label(m: &BrowserModel<'_>) -> String {
    match m.marked.len() {
        n if n > 1 => format!("휴지통으로 이동 ({n}장)  ⌘⌫"),
        _ => "휴지통으로 이동  ⌘⌫".to_string(),
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
    ui.horizontal(|ui| {
        if ui.button("⬆ 상위").on_hover_text("상위 폴더 (⌫)").clicked() {
            out.actions.push(Action::ParentFolder);
        }
        if ui.button("새로고침").on_hover_text("새로고침 (⌘R)").clicked() {
            out.actions.push(Action::Refresh);
        }
        if ui.add_enabled(m.selected.is_some(), egui::Button::new("전체 보기")).on_hover_text("선택한 사진 크게 보기 (Enter·더블클릭)").clicked() {
            out.actions.push(Action::EnterView);
        }
        if ui.add_enabled(!m.marked.is_empty(), egui::Button::new("휴지통")).on_hover_text("선택한 사진을 휴지통으로 (⌘⌫)").clicked() {
            out.actions.push(Action::Trash);
        }
        ui.separator();
        ui.label("크기");
        ui.add(egui::Slider::new(&mut st.tile, TILE_MIN..=TILE_MAX).show_value(false));
        ui.separator();
        let (cur, asc) = m.sort;
        let name = SORTS.iter().find(|(k, _)| *k == cur).map(|(_, n)| *n).unwrap_or("");
        egui::ComboBox::from_id_salt("iv-sort").selected_text(format!("정렬: {name} {}", if asc { "↑" } else { "↓" })).show_ui(ui, |ui| sort_items(ui, m, out));
        ui.separator();
        // 경로 줄 — 고쳐 쓰고 Enter 로 이동
        let dir_s = m.dir.map(|d| d.display().to_string()).unwrap_or_default();
        if st.path_edit_for.as_deref() != m.dir {
            st.path_edit = dir_s.clone();
            st.path_edit_for = m.dir.map(Path::to_path_buf);
        }
        let r = ui.add(egui::TextEdit::singleline(&mut st.path_edit).desired_width(f32::INFINITY).hint_text("폴더 경로"));
        if r.lost_focus() {
            let p = PathBuf::from(st.path_edit.trim());
            // Enter 로 확정했고 폴더가 있으면 이동, 아니면(다른 곳 클릭·Esc·없는 경로) 현재 경로로 되돌린다
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) && p.is_dir() {
                out.actions.push(Action::OpenFolder(p));
            } else {
                st.path_edit = dir_s;
            }
        }
    });
}

fn folder_tree(ui: &mut egui::Ui, st: &mut BrowserState, m: &BrowserModel<'_>, out: &mut BrowserOut) {
    egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        for (k, row) in m.tree.iter().enumerate() {
            // 줄 id 를 경로로 고정 — 스캔으로 위 줄이 숨겨져 줄이 밀려도 누르던 줄의 클릭이 사라지지 않게
            ui.push_id(&row.path, |ui| ui.horizontal(|ui| {
                ui.add_space(row.depth as f32 * 14.0);
                // 스캔으로 보일 하위 폴더가 없다고 알면 펼침 삼각형을 뺀다
                let counted = m.tree_counts.get(k).copied().flatten();
                if row.has_children && counted.is_none_or(|(_, f)| f > 0) {
                    let arrow = if row.expanded { "▼" } else { "▶" };
                    if ui.add(egui::Button::new(RichText::new(arrow).size(10.0)).frame(false).min_size(Vec2::new(14.0, 14.0))).clicked() {
                        out.tree_toggle = Some(row.path.clone());
                    }
                } else {
                    ui.add_space(18.0);
                }
                let current = m.dir == Some(row.path.as_path());
                let r = ui.selectable_label(current, row.name.as_str());
                if r.clicked() && !current {
                    out.actions.push(Action::OpenFolder(row.path.clone()));
                }
                if r.double_clicked() && row.has_children {
                    out.tree_toggle = Some(row.path.clone());
                }
                //#region 오른쪽 클릭 — 새 폴더 · 이름 바꾸기 (루트는 이름을 바꾸지 않는다)
                r.context_menu(|ui| {
                    if ui.button("새 폴더…").clicked() {
                        st.dialog = Some(Dialog::new_folder(row.path.clone()));
                        ui.close();
                    }
                    if ui.add_enabled(row.depth > 0, egui::Button::new("이름 바꾸기…")).clicked() {
                        st.dialog = Some(Dialog::rename_folder(row.path.clone()));
                        ui.close();
                    }
                });
                //#endregion
                //#region 폴더 옆 숫자 — 사진 N장 · 하위 폴더 N
                if let Some(Some((photos, folders))) = m.tree_counts.get(k) {
                    let mut t = Vec::new();
                    if *photos > 0 {
                        t.push(format!("{photos}장"));
                    }
                    if *folders > 0 {
                        t.push(format!("폴더 {folders}"));
                    }
                    if !t.is_empty() {
                        ui.label(RichText::new(t.join(" · ")).size(11.0).color(Color32::from_gray(130)));
                    }
                }
                //#endregion
            }));
        }
    });
}

fn grid(ui: &mut egui::Ui, st: &mut BrowserState, m: &BrowserModel<'_>, out: &mut BrowserOut) {
    let n = m.entries.len();
    if n == 0 {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new(if m.dir.is_some() { "이 폴더에는 사진이 없습니다" } else { "왼쪽에서 폴더를 고르세요" }).color(Color32::from_gray(150)));
        });
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
        for r in first..last {
            for c in 0..cols {
                let i = r * cols + c;
                let Some(e) = m.entries.get(i) else { break };
                let min = origin + Vec2::new(left_pad + c as f32 * pitch_x, r as f32 * pitch_y);
                let cell = Rect::from_min_size(min, Vec2::new(tile, tile + LABEL_H));
                let resp = ui.interact(cell, ui.scope_id().with(("iv-thumb", e.id.0)), Sense::CLICK);
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
                    if ui.button("이름 바꾸기…  F2").clicked() {
                        if !marked {
                            out.actions.push(Action::Select(i));
                        }
                        if let Some(p) = e.path() {
                            st.begin_rename_file(p);
                        }
                        ui.close();
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
    });
}

#[allow(clippy::too_many_arguments)]
fn draw_tile(painter: &egui::Painter, st: &BrowserState, m: &BrowserModel<'_>, e: &Entry, cell: Rect, tile: f32, selected: bool, current: bool, hovered: bool) {
    let bg = if selected {
        Color32::from_rgb(38, 79, 120)
    } else if hovered {
        Color32::from_gray(48)
    } else {
        Color32::from_gray(34)
    };
    painter.rect_filled(cell, CornerRadius::same(4), bg);
    let img_box = Rect::from_min_size(cell.min, Vec2::splat(tile)).shrink(6.0);
    if let Some(tex) = st.textures.get(e.id.0) {
        let r = fit_rect(img_box, tex.size_vec2(), false);
        painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
    } else if m.failed.contains(&e.id.0) {
        painter.text(img_box.center(), Align2::CENTER_CENTER, "열 수 없음", FontId::proportional(12.0), Color32::from_rgb(230, 160, 90));
    } else {
        painter.rect_filled(img_box.shrink(img_box.width() * 0.2), CornerRadius::same(3), Color32::from_gray(44));
    }
    // 파일명 (한 줄, 말줄임) + 해상도
    let text_w = cell.width() - 8.0;
    let name_color = if selected { Color32::WHITE } else { Color32::from_gray(220) };
    let mut job = egui::text::LayoutJob::simple_singleline(e.name.clone(), FontId::proportional(12.0), name_color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(text_w);
    job.halign = egui::Align::Center;
    let galley = painter.layout_job(job);
    painter.galley(Pos2::new(cell.center().x, cell.min.y + tile + 1.0), galley, name_color);
    if let Some([w, h]) = st.textures.full_size(e.id.0) {
        painter.text(Pos2::new(cell.center().x, cell.min.y + tile + 17.0), Align2::CENTER_TOP, format!("{w}×{h}"), FontId::proportional(11.0), Color32::from_gray(150));
    }
    // 테두리 = 현재 항목 (여러 장 선택 중에도 키보드·미리보기 기준이 어디인지 보이게)
    if current {
        let color = if selected { Color32::from_rgb(90, 150, 220) } else { Color32::from_gray(140) };
        painter.rect_stroke(cell, CornerRadius::same(4), Stroke::new(1.5, color), StrokeKind::Inside);
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
