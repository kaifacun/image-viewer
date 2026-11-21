//! macOS 메뉴 막대 (화면 맨 위) — muda NSMenu. 창 안 egui 메뉴 대신.
//!
//! - 항목 id → [`Cmd`] 하나로 모은다. 실행은 앱(`window_app::on_menu`)이 기존 `handle()` 경로로.
//! - 단축키는 **⌘ 조합만** 메뉴에 건다. 그냥 키(⌫·⏎·F2·Space·r/l…)를 걸면 이름 입력 칸에서 글자를 빼앗는다.
//!   메뉴가 ⌘키를 먼저 가져가므로(winit 에는 안 옴) 두 번 실행되지 않는다.
//! - 글 입력 중의 ⌘C/X/V/A·⌘⌫ 는 앱이 egui 입력으로 돌려준다 (메뉴가 가져가 입력 칸이 못 받기 때문).
//! - 항목 사용 가능·체크는 [`MenuState`] 가 바뀔 때만 다시 맞춘다.

use iv_core::formats::MediaKind;
use iv_library::SortKey;
use iv_viewer::Action;
use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{AboutMetadata, CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// 메뉴가 고른 일
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    Act(Action),
    RenameFolder,
    TrashFolder,
    ToggleEmptyFolders,
    Tile(f32),
    Help,
    /// 회전 (브라우저면 표식 사진 파일에, 전체 보기면 화면에)
    Rotate(u8),
}

/// 메뉴 모양을 정하는 상태 (바뀔 때만 반영)
#[derive(Clone, Debug, PartialEq)]
pub struct MenuState {
    pub browse: bool,
    pub has_current: bool,
    pub marked: usize,
    pub has_dir: bool,
    pub kind: MediaKind,
    pub all_files: bool,
    pub sort: (SortKey, bool),
    pub tile: u8,
    pub show_empty: bool,
    pub codec_engine: Option<String>,
    pub can_trash_folder: bool,
    pub rotated: bool,
    /// 대화 상자가 열려 있다 (끝내기·글 편집 말고는 막는다)
    pub modal: bool,
    /// 글 입력 칸에 초점
    pub text_editing: bool,
}

pub const TILES: [(f32, &str); 3] = [(96.0, "썸네일 작게"), (128.0, "썸네일 보통"), (176.0, "썸네일 크게")];
const SORTS: [(SortKey, &str); 4] = [(SortKey::Name, "이름"), (SortKey::Modified, "수정한 날짜"), (SortKey::Size, "크기"), (SortKey::Type, "형식")];

fn cmd(m: Modifiers, c: Code) -> Option<Accelerator> {
    Some(Accelerator::new(Some(m), c))
}
const SUPER: Modifiers = Modifiers::SUPER;

struct Items {
    new_folder: MenuItem,
    rename: MenuItem,
    rename_folder: MenuItem,
    parent: MenuItem,
    refresh: MenuItem,
    trash: MenuItem,
    trash_folder: MenuItem,
    cut: MenuItem,
    copy: MenuItem,
    paste: MenuItem,
    copy_path: MenuItem,
    select_all: MenuItem,
    rot_l: MenuItem,
    rot_r: MenuItem,
    rot_180: MenuItem,
    save_rot: MenuItem,
    kinds: [CheckMenuItem; 3],
    all_files: CheckMenuItem,
    enter: MenuItem,
    exit: MenuItem,
    info: MenuItem,
    zoom_in: MenuItem,
    zoom_out: MenuItem,
    fit: MenuItem,
    actual: MenuItem,
    show_empty: CheckMenuItem,
    tiles: [CheckMenuItem; 3],
    codec: MenuItem,
    sorts: [CheckMenuItem; 4],
    asc: CheckMenuItem,
    desc: CheckMenuItem,
    /// 창 닫기(⌘W) — 대화 상자 중에도 켜 둔다 (끝내기 규칙)
    close: MenuItem,
    /// 대화 상자가 열려 있을 때 통째로 끄는 하위 메뉴 (⌘키가 없는 것만 — 파일·편집은 항목별로)
    gated: Vec<Submenu>,
}

pub struct MacMenu {
    _menu: Menu,
    items: Items,
    last: Option<MenuState>,
    queue: Arc<Mutex<VecDeque<String>>>,
}

impl MacMenu {
    /// 메뉴를 만들어 NSApp 에 건다. 고른 항목은 `wake` 로 알리고 [`MacMenu::take`] 로 꺼낸다.
    pub fn install(wake: impl Fn() + Send + Sync + 'static) -> Result<Self, String> {
        let e = |r: Result<(), muda::Error>| r.map_err(|e| e.to_string());
        let item = |id: &str, text: &str, acc: Option<Accelerator>| MenuItem::with_id(id, text, true, acc);
        let check = |id: &str, text: &str, acc: Option<Accelerator>| CheckMenuItem::with_id(id, text, true, false, acc);
        let sep = PredefinedMenuItem::separator;

        //#region KaiIV (앱 메뉴 — 제목은 묶음 이름)
        let about = PredefinedMenuItem::about(
            Some("KaiIV 정보"),
            Some(AboutMetadata { name: Some("KaiIV".into()), version: Some(env!("CARGO_PKG_VERSION").into()), copyright: Some("© 2026 kaifacun · MIT 라이선스".into()), ..Default::default() }),
        );
        let app = Submenu::with_items(
            "KaiIV",
            true,
            &[
                &about,
                &sep(),
                &item("update", "업데이트 확인…", None),
                &sep(),
                &PredefinedMenuItem::services(Some("서비스")),
                &sep(),
                &PredefinedMenuItem::hide(Some("KaiIV 가리기")),
                &PredefinedMenuItem::hide_others(Some("기타 가리기")),
                &PredefinedMenuItem::show_all(Some("모두 보기")),
                &sep(),
                &item("quit", "KaiIV 종료", cmd(SUPER, Code::KeyQ)),
            ],
        )
        .map_err(|e| e.to_string())?;
        //#endregion

        let it = Items {
            new_folder: item("new_folder", "새 폴더…", cmd(SUPER | Modifiers::SHIFT, Code::KeyN)),
            rename: item("rename", "이름 바꾸기… (F2)", None),
            rename_folder: item("rename_folder", "폴더 이름 바꾸기…", None),
            parent: item("parent", "상위 폴더", cmd(SUPER, Code::ArrowUp)),
            refresh: item("refresh", "새로고침", cmd(SUPER, Code::KeyR)),
            trash: item("trash", "휴지통으로 이동", cmd(SUPER, Code::Backspace)),
            trash_folder: item("trash_folder", "이 폴더를 휴지통으로…", None),
            cut: item("cut", "잘라내기", cmd(SUPER, Code::KeyX)),
            copy: item("copy", "복사", cmd(SUPER, Code::KeyC)),
            paste: item("paste", "붙여넣기", cmd(SUPER, Code::KeyV)),
            copy_path: item("copy_path", "경로 복사", cmd(SUPER | Modifiers::ALT, Code::KeyC)),
            select_all: item("select_all", "모두 선택", cmd(SUPER, Code::KeyA)),
            rot_l: item("rot_l", "왼쪽으로 90° 돌리기", None),
            rot_r: item("rot_r", "오른쪽으로 90° 돌리기", None),
            rot_180: item("rot_180", "180° 돌리기", None),
            save_rot: item("save_rot", "돌린 방향 저장", cmd(SUPER, Code::KeyS)),
            kinds: [check("kind_image", "사진", cmd(SUPER, Code::Digit1)), check("kind_video", "비디오", cmd(SUPER, Code::Digit2)), check("kind_doc", "문서", cmd(SUPER, Code::Digit3))],
            all_files: check("all_files", "모든 파일", cmd(SUPER, Code::Digit4)),
            enter: item("enter", "전체 보기 (⏎)", None),
            exit: item("exit", "브라우저로 돌아가기 (esc)", None),
            info: item("info", "정보 보기 (I)", None),
            zoom_in: item("zoom_in", "확대", cmd(SUPER, Code::Equal)),
            zoom_out: item("zoom_out", "축소", cmd(SUPER, Code::Minus)),
            fit: item("fit", "화면에 맞추기", cmd(SUPER, Code::Digit0)),
            actual: item("actual", "실제 크기 (1)", None),
            show_empty: check("show_empty", "사진 없는 폴더도 보이기", None),
            tiles: [check("tile_0", TILES[0].1, None), check("tile_1", TILES[1].1, None), check("tile_2", TILES[2].1, None)],
            codec: item("codec", "추가 코덱 엔진 받기…", None),
            sorts: [check("sort_0", SORTS[0].1, None), check("sort_1", SORTS[1].1, None), check("sort_2", SORTS[2].1, None), check("sort_3", SORTS[3].1, None)],
            asc: check("asc", "오름차순", None),
            desc: check("desc", "내림차순", None),
            close: item("close", "창 닫기", cmd(SUPER, Code::KeyW)),
            gated: Vec::new(),
        };

        let file = Submenu::with_items(
            "파일",
            true,
            &[&it.new_folder, &it.rename, &it.rename_folder, &sep(), &it.parent, &it.refresh, &sep(), &it.trash, &it.trash_folder, &sep(), &it.close],
        )
        .map_err(|e| e.to_string())?;
        let edit = Submenu::with_items(
            "편집",
            true,
            &[&it.cut, &it.copy, &it.paste, &it.copy_path, &it.select_all, &sep(), &it.rot_l, &it.rot_r, &it.rot_180, &it.save_rot],
        )
        .map_err(|e| e.to_string())?;
        let view = Submenu::with_items(
            "보기",
            true,
            &[
                &it.kinds[0],
                &it.kinds[1],
                &it.kinds[2],
                &it.all_files,
                &sep(),
                &it.enter,
                &it.exit,
                &it.info,
                &sep(),
                &it.zoom_in,
                &it.zoom_out,
                &it.fit,
                &it.actual,
                &sep(),
                &it.show_empty,
                &it.tiles[0],
                &it.tiles[1],
                &it.tiles[2],
                &sep(),
                &it.codec,
                &sep(),
                &item("fullscreen", "전체 화면", cmd(SUPER | Modifiers::CONTROL, Code::KeyF)),
            ],
        )
        .map_err(|e| e.to_string())?;
        let sort = Submenu::with_items("정렬", true, &[&it.sorts[0], &it.sorts[1], &it.sorts[2], &it.sorts[3], &sep(), &it.asc, &it.desc]).map_err(|e| e.to_string())?;
        let window = Submenu::with_items(
            "윈도우",
            true,
            &[&PredefinedMenuItem::minimize(Some("최소화")), &PredefinedMenuItem::maximize(Some("확대/축소")), &sep(), &PredefinedMenuItem::bring_all_to_front(Some("모두 앞으로 가져오기"))],
        )
        .map_err(|e| e.to_string())?;
        let help = Submenu::with_items("도움말", true, &[&item("help", "KaiIV 도움말", None)]).map_err(|e| e.to_string())?;

        let menu = Menu::new();
        for s in [&app, &file, &edit, &view, &sort, &window, &help] {
            e(menu.append(s))?;
        }
        menu.init_for_nsapp();
        window.set_as_windows_menu_for_nsapp();
        help.set_as_help_menu_for_nsapp();

        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let q = queue.clone();
        MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
            q.lock().unwrap_or_else(|e| e.into_inner()).push_back(ev.id().0.clone());
            wake();
        }));
        let mut items = it;
        items.gated = vec![sort];
        Ok(MacMenu { _menu: menu, items, last: None, queue })
    }

    /// 고른 항목 id 들
    pub fn take(&self) -> Vec<String> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect()
    }

    /// 상태가 바뀌었으면 사용 가능·체크를 맞춘다
    pub fn sync(&mut self, s: &MenuState) {
        if self.last.as_ref() == Some(s) {
            return;
        }
        let it = &self.items;
        let ok = !s.modal;
        let b = ok && s.browse;
        let v = ok && !s.browse;
        for g in &it.gated {
            g.set_enabled(ok);
        }
        it.new_folder.set_enabled(b && s.has_dir);
        it.rename.set_enabled(b && s.has_current);
        it.rename_folder.set_enabled(b && s.has_dir);
        it.parent.set_enabled(b && s.has_dir);
        it.refresh.set_enabled(ok);
        // 글 입력 중엔 ⌘⌫ 가 입력 칸의 '줄 앞까지 지우기' 로 가도록 켜 둔다
        it.trash.set_enabled(s.text_editing || (ok && (s.marked > 0 || (!s.browse && s.has_current))));
        it.close.set_enabled(true);
        it.trash.set_text(if s.browse && s.marked > 1 { format!("휴지통으로 이동 ({}{})", s.marked, s.kind.unit()) } else { "휴지통으로 이동".into() });
        it.trash_folder.set_enabled(b && s.can_trash_folder);
        // 글 입력 중엔 편집 항목을 켜 둔다 (앱이 입력 칸으로 돌려준다)
        let te = s.text_editing;
        let some = s.marked > 0 || (!s.browse && s.has_current);
        it.cut.set_enabled(te || (b && some));
        it.copy.set_enabled(te || (ok && some));
        it.paste.set_enabled(te || (b && s.has_dir));
        it.copy_path.set_enabled(ok && s.has_current);
        it.select_all.set_enabled(te || b);
        let image = s.kind == MediaKind::Image && !s.all_files;
        let rot = ok && image && (if s.browse { s.marked > 0 } else { s.has_current });
        it.rot_l.set_enabled(rot);
        it.rot_r.set_enabled(rot);
        it.rot_180.set_enabled(rot && s.browse);
        it.save_rot.set_enabled(v && s.rotated);
        let kinds = [MediaKind::Image, MediaKind::Video, MediaKind::Document];
        for (i, k) in kinds.iter().enumerate() {
            it.kinds[i].set_checked(s.kind == *k);
            it.kinds[i].set_enabled(ok);
        }
        it.all_files.set_checked(s.all_files);
        it.all_files.set_enabled(ok);
        it.enter.set_enabled(b && s.has_current);
        it.exit.set_enabled(v);
        it.info.set_enabled(v);
        for z in [&it.zoom_in, &it.zoom_out, &it.fit, &it.actual] {
            z.set_enabled(v);
        }
        it.show_empty.set_checked(s.show_empty || s.all_files);
        it.show_empty.set_enabled(b && !s.all_files);
        for (i, t) in it.tiles.iter().enumerate() {
            t.set_checked(s.tile as usize == i);
            t.set_enabled(b);
        }
        match &s.codec_engine {
            Some(src) => {
                it.codec.set_text(format!("추가 코덱 엔진: {src}"));
                it.codec.set_enabled(false);
            }
            None => {
                it.codec.set_text("추가 코덱 엔진 받기…");
                it.codec.set_enabled(ok);
            }
        }
        for (i, (k, _)) in SORTS.iter().enumerate() {
            it.sorts[i].set_checked(s.sort.0 == *k);
        }
        it.asc.set_checked(s.sort.1);
        it.desc.set_checked(!s.sort.1);
        self.last = Some(s.clone());
    }
}

/// 체크 항목은 누르면 macOS 가 체크를 뒤집으므로, 다음 sync 가 반드시 다시 맞추게 한다
impl MacMenu {
    pub fn invalidate(&mut self) {
        self.last = None;
    }
}

/// 항목 id → 할 일. `s` 는 지금 상태 (정렬 방향 유지·회전 대상 등).
pub fn command(id: &str, s: &MenuState) -> Option<Cmd> {
    use Action as A;
    let act = |a| Some(Cmd::Act(a));
    match id {
        "update" => act(A::UpdateCheck),
        "quit" | "close" => act(A::Quit),
        "new_folder" => act(A::BeginNewFolder),
        "rename" => act(A::BeginRename),
        "rename_folder" => Some(Cmd::RenameFolder),
        "parent" => act(A::ParentFolder),
        "refresh" => act(A::Refresh),
        "trash" => act(A::Trash),
        "trash_folder" => Some(Cmd::TrashFolder),
        "cut" => act(A::Cut),
        "copy" => act(A::Copy),
        "paste" => act(A::Paste(None)),
        "copy_path" => act(A::CopyPath),
        "select_all" => act(A::SelectAll),
        "rot_l" => Some(Cmd::Rotate(3)),
        "rot_r" => Some(Cmd::Rotate(1)),
        "rot_180" => Some(Cmd::Rotate(2)),
        "save_rot" => act(A::SaveRotation),
        "kind_image" => act(A::SetKind(MediaKind::Image)),
        "kind_video" => act(A::SetKind(MediaKind::Video)),
        "kind_doc" => act(A::SetKind(MediaKind::Document)),
        "all_files" => act(A::ToggleAllFiles),
        "enter" => act(A::EnterView),
        "exit" => act(A::ExitView),
        "info" => act(A::ToggleInfo),
        "zoom_in" => act(A::ZoomIn),
        "zoom_out" => act(A::ZoomOut),
        "fit" => act(A::Fit),
        "actual" => act(A::ActualSize),
        "show_empty" => Some(Cmd::ToggleEmptyFolders),
        "tile_0" => Some(Cmd::Tile(TILES[0].0)),
        "tile_1" => Some(Cmd::Tile(TILES[1].0)),
        "tile_2" => Some(Cmd::Tile(TILES[2].0)),
        "codec" => act(A::InstallCodecs),
        "fullscreen" => act(A::ToggleFullscreen),
        "asc" => act(A::Sort(s.sort.0, true)),
        "desc" => act(A::Sort(s.sort.0, false)),
        "help" => Some(Cmd::Help),
        _ => {
            let i: usize = id.strip_prefix("sort_")?.parse().ok()?;
            act(A::Sort(SORTS.get(i)?.0, s.sort.1))
        }
    }
}

/// 대화 상자가 열려 있어도 되는 항목 (끝내기 — 글 편집은 앱이 입력 칸으로 먼저 돌린다)
pub fn allowed_in_modal(id: &str) -> bool {
    matches!(id, "quit" | "close")
}

/// 글 입력 중이면 입력 칸으로 돌려줄 항목
pub fn is_text_edit(id: &str) -> bool {
    matches!(id, "cut" | "copy" | "paste" | "select_all" | "trash")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st() -> MenuState {
        MenuState {
            browse: true,
            has_current: true,
            marked: 1,
            has_dir: true,
            kind: MediaKind::Image,
            all_files: false,
            sort: (SortKey::Size, false),
            tile: 1,
            show_empty: false,
            codec_engine: None,
            can_trash_folder: false,
            rotated: false,
            modal: false,
            text_editing: false,
        }
    }

    #[test]
    fn ids_map() {
        let s = st();
        assert_eq!(command("sort_1", &s), Some(Cmd::Act(Action::Sort(SortKey::Modified, false))));
        assert_eq!(command("asc", &s), Some(Cmd::Act(Action::Sort(SortKey::Size, true))));
        assert_eq!(command("tile_2", &s), Some(Cmd::Tile(176.0)));
        assert_eq!(command("quit", &s), Some(Cmd::Act(Action::Quit)));
        assert_eq!(command("nope", &s), None);
        assert_eq!(command("sort_9", &s), None);
        assert!(allowed_in_modal("quit") && !allowed_in_modal("copy") && !allowed_in_modal("new_folder"));
        assert!(is_text_edit("paste") && !is_text_edit("refresh"));
    }
}
