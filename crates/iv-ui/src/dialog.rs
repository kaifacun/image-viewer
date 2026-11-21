//! 대화 상자: 이름 입력(새 폴더 · 폴더 이름 바꾸기 · 사진 이름 바꾸기)과 폴더 휴지통 확인.
//! 상태를 바꾸지 않고, 확인하면 [`Action`] 을 돌려준다.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use egui::text::{CCursor, CCursorRange};
use egui::{Color32, Id, RichText};
use iv_viewer::Action;

#[derive(Clone, Debug)]
pub enum DialogKind {
    NewFolder { parent: PathBuf },
    RenameFolder { path: PathBuf },
    RenameFile { path: PathBuf },
    /// 폴더를 휴지통으로 — 확인 창 (하위를 백그라운드로 센다)
    TrashFolder { path: PathBuf, count: Arc<FolderCount>, ident: Option<(u64, u64)> },
}

/// 폴더 안 세기 결과 (참고용 — 세는 중에도 확인할 수 있다)
#[derive(Clone, Copy, Debug, Default)]
pub struct CountSummary {
    pub files: u64,
    pub folders: u64,
    pub bytes: u64,
    pub done: bool,
}

/// 백그라운드 세기. 마지막 참조가 사라지면(대화 상자 닫힘) 세기를 멈춘다.
#[derive(Debug)]
pub struct FolderCount {
    summary: Arc<Mutex<CountSummary>>,
    cancel: Arc<AtomicBool>,
}

impl FolderCount {
    fn start(root: PathBuf) -> Arc<Self> {
        let summary = Arc::new(Mutex::new(CountSummary::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (s, c) = (summary.clone(), cancel.clone());
        let _ = std::thread::Builder::new().name("iv-count".into()).spawn(move || {
            // 링크는 따라가지 않는다 (휴지통도 링크 자체만 옮긴다)
            let mut stack = vec![root];
            let mut local = CountSummary::default();
            let mut n = 0u32;
            while let Some(d) = stack.pop() {
                if c.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(rd) = std::fs::read_dir(&d) else { continue };
                for e in rd.flatten() {
                    let Ok(m) = e.path().symlink_metadata() else { continue };
                    if m.is_dir() {
                        local.folders += 1;
                        stack.push(e.path());
                    } else {
                        local.files += 1;
                        local.bytes += m.len();
                    }
                    n += 1;
                    if n % 256 == 0 {
                        *s.lock().unwrap_or_else(|e| e.into_inner()) = local;
                        if c.load(Ordering::Relaxed) {
                            return;
                        }
                    }
                }
            }
            local.done = true;
            *s.lock().unwrap_or_else(|e| e.into_inner()) = local;
        });
        Arc::new(Self { summary, cancel })
    }

    pub fn get(&self) -> CountSummary {
        *self.summary.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for FolderCount {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone, Debug)]
pub struct Dialog {
    pub kind: DialogKind,
    text: String,
    /// 첫 프레임: 입력 칸에 포커스 + 선택 범위 지정
    fresh: bool,
}

/// 폴더의 (dev, ino) — 링크를 따라가지 않는다
pub fn folder_ident(p: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        p.symlink_metadata().ok().map(|m| (m.dev(), m.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = p;
        None
    }
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

    /// 폴더 휴지통 확인 — 열자마자 안을 세기 시작한다
    pub fn trash_folder(path: PathBuf) -> Self {
        let count = FolderCount::start(path.clone());
        let ident = folder_ident(&path);
        Self { text: name_of(&path), kind: DialogKind::TrashFolder { path, count, ident }, fresh: true }
    }

    fn title(&self) -> &'static str {
        match self.kind {
            DialogKind::NewFolder { .. } => "새 폴더",
            DialogKind::RenameFolder { .. } => "폴더 이름 바꾸기",
            DialogKind::RenameFile { .. } => "사진 이름 바꾸기",
            DialogKind::TrashFolder { .. } => "폴더를 휴지통으로",
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
            DialogKind::TrashFolder { path, ident, .. } => Action::TrashFolder { path: path.clone(), ident: *ident },
        }
    }

    /// 지금 이름과 같은가 (바꿀 것 없음)
    fn unchanged(&self, name: &str) -> bool {
        match &self.kind {
            DialogKind::RenameFolder { path } | DialogKind::RenameFile { path } => name_of(path) == name,
            DialogKind::NewFolder { .. } | DialogKind::TrashFolder { .. } => false,
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
    if let DialogKind::TrashFolder { path, count, .. } = &d.kind {
        let (path, c) = (path.clone(), count.get());
        return draw_trash_folder(ctx, d, &path, c);
    }
    let mut result = DialogResult::Open;
    let resp = crate::modal::modal("iv-dialog").show(ctx, |ui| {
        crate::modal::begin(ui, 380.0);
        crate::modal::title(ui, None, d.title());
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
        crate::modal::buttons(ui, |ui| {
            let ok = crate::modal::primary_enabled(ui, check.is_ok(), "확인").clicked();
            if crate::modal::secondary(ui, "취소").clicked() {
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

/// 폴더 휴지통 확인 창 — 입력 칸 없음. 처음 포커스는 "취소" (Enter 로 실수하지 않게).
fn draw_trash_folder(ctx: &egui::Context, d: &mut Dialog, path: &Path, c: CountSummary) -> DialogResult {
    let mut result = DialogResult::Open;
    let resp = crate::modal::modal("iv-dialog").show(ctx, |ui| {
        crate::modal::begin(ui, 460.0);
        crate::modal::title(ui, Some(crate::icons::Icon::Trash), format!("'{}' 폴더를 휴지통으로 옮길까요?", name_of(path)));
        crate::modal::note(ui, path.display().to_string());
        ui.add_space(6.0);
        let what = format!("안에 파일 {}개 · 폴더 {}개 · {}", c.files, c.folders, iv_viewer::human_bytes(c.bytes));
        let line = if c.done { what } else { format!("세는 중…  {what}") };
        ui.label(RichText::new(line).size(13.0));
        crate::modal::note(ui, "사진이 아닌 파일도 모두 함께 옮겨집니다. 휴지통에서 되살릴 수 있습니다.");
        crate::modal::buttons(ui, |ui| {
            if crate::modal::danger(ui, "휴지통으로 이동").clicked() {
                result = DialogResult::Done(d.action(String::new()));
            }
            let cancel = crate::modal::secondary(ui, "취소");
            if d.fresh {
                d.fresh = false;
                cancel.request_focus();
            }
            if cancel.clicked() {
                result = DialogResult::Cancel;
            }
        });
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
