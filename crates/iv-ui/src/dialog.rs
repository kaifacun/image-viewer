//! 이름 입력 대화 상자 (새 폴더 · 폴더 이름 바꾸기 · 사진 이름 바꾸기).
//! 상태를 바꾸지 않고, 확인하면 [`Action`] 을 돌려준다.

use std::path::{Path, PathBuf};

use egui::text::{CCursor, CCursorRange};
use egui::{Color32, Id, RichText};
use iv_viewer::Action;

#[derive(Clone, Debug)]
pub enum DialogKind {
    NewFolder { parent: PathBuf },
    RenameFolder { path: PathBuf },
    RenameFile { path: PathBuf },
}

#[derive(Clone, Debug)]
pub struct Dialog {
    pub kind: DialogKind,
    text: String,
    /// 첫 프레임: 입력 칸에 포커스 + 선택 범위 지정
    fresh: bool,
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

impl Dialog {
    /// 새 폴더 — 이름이 겹치지 않게 "새 폴더", "새 폴더 2" …
    pub fn new_folder(parent: PathBuf) -> Self {
        let mut text = "새 폴더".to_string();
        let mut k = 2;
        while parent.join(&text).symlink_metadata().is_ok() {
            text = format!("새 폴더 {k}");
            k += 1;
        }
        Self { kind: DialogKind::NewFolder { parent }, text, fresh: true }
    }

    pub fn rename_folder(path: PathBuf) -> Self {
        Self { text: name_of(&path), kind: DialogKind::RenameFolder { path }, fresh: true }
    }

    pub fn rename_file(path: PathBuf) -> Self {
        Self { text: name_of(&path), kind: DialogKind::RenameFile { path }, fresh: true }
    }

    fn title(&self) -> &'static str {
        match self.kind {
            DialogKind::NewFolder { .. } => "새 폴더",
            DialogKind::RenameFolder { .. } => "폴더 이름 바꾸기",
            DialogKind::RenameFile { .. } => "사진 이름 바꾸기",
        }
    }

    /// 처음 선택할 글자 범위 — 사진은 확장자를 뺀 부분 (Finder 와 같이)
    fn initial_selection(&self) -> (usize, usize) {
        let n = self.text.chars().count();
        match &self.kind {
            DialogKind::RenameFile { .. } => match self.text.rfind('.') {
                Some(dot) if dot > 0 => (0, self.text[..dot].chars().count()),
                _ => (0, n),
            },
            _ => (0, n),
        }
    }

    fn action(&self, name: String) -> Action {
        // 사진 이름에서 확장자를 지웠으면 원래 확장자를 붙인다 (없으면 격자에서 사라진다)
        let name = match &self.kind {
            DialogKind::RenameFile { path } => match path.extension().and_then(|e| e.to_str()) {
                Some(ext) if Path::new(&name).extension().is_none() => format!("{name}.{ext}"),
                _ => name,
            },
            _ => name,
        };
        match &self.kind {
            DialogKind::NewFolder { parent } => Action::NewFolder { parent: parent.clone(), name },
            DialogKind::RenameFolder { path } => Action::RenameFolder { path: path.clone(), name },
            DialogKind::RenameFile { path } => Action::RenameFile { path: path.clone(), name },
        }
    }

    /// 지금 이름과 같은가 (바꿀 것 없음)
    fn unchanged(&self, name: &str) -> bool {
        match &self.kind {
            DialogKind::RenameFolder { path } | DialogKind::RenameFile { path } => name_of(path) == name,
            DialogKind::NewFolder { .. } => false,
        }
    }
}

pub enum DialogResult {
    Open,
    Cancel,
    Done(Action),
}

/// 대화 상자를 그린다 (모달 — 뒤의 격자·트리는 막힌다)
pub fn draw(ctx: &egui::Context, d: &mut Dialog) -> DialogResult {
    let mut result = DialogResult::Open;
    let resp = egui::Modal::new(Id::unique("iv-dialog")).show(ctx, |ui| {
        ui.set_width(360.0);
        ui.label(RichText::new(d.title()).size(15.0).strong());
        ui.add_space(6.0);
        let mut out = egui::TextEdit::singleline(&mut d.text).desired_width(f32::INFINITY).id(Id::unique("iv-dialog-text")).show(ui);
        //#region 첫 프레임: 포커스 + 선택 범위 (사진은 확장자 앞까지)
        if d.fresh {
            d.fresh = false;
            out.response.request_focus();
            let (a, b) = d.initial_selection();
            out.state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
            out.state.store(ui.ctx(), out.response.id);
        }
        //#endregion
        let check = iv_fsops::validate_name(&d.text).map(str::to_string);
        if let Err(e) = &check {
            ui.label(RichText::new(e).size(12.0).color(Color32::from_rgb(230, 160, 90)));
        }
        let enter = out.response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let ok = ui.add_enabled(check.is_ok(), egui::Button::new("확인")).clicked();
            if ui.button("취소").clicked() {
                result = DialogResult::Cancel;
            }
            if let (true, Ok(name)) = (ok || enter, &check) {
                result = if d.unchanged(name) { DialogResult::Cancel } else { DialogResult::Done(d.action(name.clone())) };
            }
        });
        // Enter 를 눌렀지만 이름이 잘못됐으면 다시 입력 칸으로
        if enter && check.is_err() {
            out.response.request_focus();
        }
    });
    if matches!(result, DialogResult::Open) && (resp.should_close() || ctx.input(|i| i.key_pressed(egui::Key::Escape))) {
        result = DialogResult::Cancel;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_keeps_extension_when_dropped() {
        let d = Dialog::rename_file(PathBuf::from("/x/p00.JPG"));
        assert_eq!(d.action("바다".into()), Action::RenameFile { path: PathBuf::from("/x/p00.JPG"), name: "바다.JPG".into() });
        assert_eq!(d.action("바다.png".into()), Action::RenameFile { path: PathBuf::from("/x/p00.JPG"), name: "바다.png".into() });
        // 폴더는 그대로
        let f = Dialog::rename_folder(PathBuf::from("/x/여행"));
        assert_eq!(f.action("2026".into()), Action::RenameFolder { path: PathBuf::from("/x/여행"), name: "2026".into() });
        // 처음 선택은 확장자 앞까지
        assert_eq!(d.initial_selection(), (0, 3));
    }
}
