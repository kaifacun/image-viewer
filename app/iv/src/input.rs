//! 키 → Action 매핑 (단축키 표). 새 단축키는 여기에만 추가한다.

use iv_viewer::{Action, Mode, VideoCmd};
use winit::keyboard::{Key, ModifiersState, NamedKey};

pub fn key_action(key: &Key, mods: ModifiersState, fullscreen: bool, mode: Mode, video: bool) -> Option<Action> {
    let cmd = mods.super_key() || mods.control_key();
    let shift = mods.shift_key();
    //#region 비디오 전체 보기: Space 재생/정지 · ←/→ 5초(⇧ 1초) · ↑/↓ 소리 · M 소리 끄기 · PageUp/PageDown 이전·다음 영상
    if video && mode == Mode::View && !cmd {
        let step = if shift { 1.0 } else { 5.0 };
        let a = match key {
            Key::Named(NamedKey::Space) => Some(VideoCmd::Toggle),
            Key::Named(NamedKey::ArrowRight) => Some(VideoCmd::SeekBy(step)),
            Key::Named(NamedKey::ArrowLeft) => Some(VideoCmd::SeekBy(-step)),
            Key::Named(NamedKey::ArrowUp) => Some(VideoCmd::VolumeBy(0.1)),
            Key::Named(NamedKey::ArrowDown) => Some(VideoCmd::VolumeBy(-0.1)),
            Key::Character(c) if matches!(c.as_str(), "m" | "M" | "ㅡ") => Some(VideoCmd::ToggleMute),
            Key::Character(c) if matches!(c.as_str(), "k" | "K" | "ㅏ") => Some(VideoCmd::Toggle),
            _ => None,
        };
        if let Some(a) = a {
            return Some(Action::Video(a));
        }
    }
    //#endregion
    match key {
        Key::Named(n) => match (mode, n) {
            (_, NamedKey::Escape) if fullscreen => Some(Action::ToggleFullscreen),
            // 휴지통: ⌘⌫ (Finder 와 같음) · 앞으로 지우기(Delete) — 그냥 ⌫(상위 폴더·이전 장)보다 먼저 본다
            (_, NamedKey::Backspace) if cmd => Some(Action::Trash),
            (_, NamedKey::Delete) => Some(Action::Trash),
            // 브라우저: ⇧+화살표 = 선택 범위 넓히기
            (Mode::Browse, NamedKey::ArrowRight) if shift => Some(Action::GridExtend { dx: 1, dy: 0 }),
            (Mode::Browse, NamedKey::ArrowLeft) if shift => Some(Action::GridExtend { dx: -1, dy: 0 }),
            (Mode::Browse, NamedKey::ArrowDown) if shift => Some(Action::GridExtend { dx: 0, dy: 1 }),
            (Mode::Browse, NamedKey::ArrowUp) if shift => Some(Action::GridExtend { dx: 0, dy: -1 }),
            // 브라우저: 격자 이동
            (Mode::Browse, NamedKey::ArrowRight) => Some(Action::GridMove { dx: 1, dy: 0 }),
            (Mode::Browse, NamedKey::ArrowLeft) => Some(Action::GridMove { dx: -1, dy: 0 }),
            (Mode::Browse, NamedKey::ArrowDown) => Some(Action::GridMove { dx: 0, dy: 1 }),
            (Mode::Browse, NamedKey::ArrowUp) => Some(Action::GridMove { dx: 0, dy: -1 }),
            (Mode::Browse, NamedKey::PageDown) => Some(Action::GridPage(1)),
            (Mode::Browse, NamedKey::PageUp) => Some(Action::GridPage(-1)),
            (Mode::Browse, NamedKey::Enter | NamedKey::Space) => Some(Action::EnterView),
            (Mode::Browse, NamedKey::Backspace) => Some(Action::ParentFolder),
            (Mode::Browse, NamedKey::F2) => Some(Action::BeginRename),
            // 전체 보기
            (Mode::View, NamedKey::Escape) => Some(Action::ExitView),
            (Mode::View, NamedKey::ArrowRight | NamedKey::ArrowDown | NamedKey::PageDown | NamedKey::Space) => Some(Action::Next),
            (Mode::View, NamedKey::ArrowLeft | NamedKey::ArrowUp | NamedKey::PageUp | NamedKey::Backspace) => Some(Action::Prev),
            (Mode::View, NamedKey::Enter) => Some(Action::ToggleFullscreen),
            (_, NamedKey::Home) => Some(Action::First),
            (_, NamedKey::End) => Some(Action::Last),
            _ => None,
        },
        Key::Character(c) => match (c.as_str(), cmd, mode) {
            ("q" | "w", true, _) => Some(Action::Quit),
            ("r", true, _) => Some(Action::Refresh),
            ("a", true, Mode::Browse) => Some(Action::SelectAll),
            // 종류: ⌘1 사진 · ⌘2 비디오 · ⌘3 문서
            ("1", true, _) => Some(Action::SetKind(iv_core::formats::MediaKind::Image)),
            ("2", true, _) => Some(Action::SetKind(iv_core::formats::MediaKind::Video)),
            ("3", true, _) => Some(Action::SetKind(iv_core::formats::MediaKind::Document)),
            // 클립보드: ⌘C 복사 · ⌘X 잘라내기 · ⌘V 붙여넣기 · ⌥⌘C 경로 복사 (⌥ 면 글자가 ç 로 온다, 한글 입력이면 ㅊ·ㅌ·ㅍ)
            ("c" | "C" | "ç" | "Ç" | "ㅊ", true, _) if mods.alt_key() => Some(Action::CopyPath),
            ("c" | "ㅊ", true, _) => Some(Action::Copy),
            ("x" | "ㅌ", true, _) => Some(Action::Cut),
            ("v" | "ㅍ", true, _) => Some(Action::Paste(None)),
            ("n" | "N", true, Mode::Browse) if shift => Some(Action::BeginNewFolder),
            ("f", false, _) => Some(Action::ToggleFullscreen),
            ("=" | "+", _, Mode::View) => Some(Action::ZoomIn),
            ("-" | "_", _, Mode::View) => Some(Action::ZoomOut),
            ("0", _, Mode::View) => Some(Action::Fit),
            ("9", _, Mode::View) => Some(Action::FitAlways),
            ("1", _, Mode::View) => Some(Action::ActualSize),
            ("i", false, Mode::View) => Some(Action::ToggleInfo),
            // 회전 (전체 보기): R 시계 · L/⇧R 반시계 · ⌘S 저장 (⌘R 은 새로고침)
            // 한글 입력 상태면 같은 자리 글자(ㄱ·ㄲ·ㅣ·ㄴ)로 들어온다
            ("r" | "ㄱ", false, Mode::View) => Some(Action::RotateCw),
            ("R" | "l" | "L" | "ㄲ" | "ㅣ", false, Mode::View) => Some(Action::RotateCcw),
            ("s" | "ㄴ", true, Mode::View) => Some(Action::SaveRotation),
            _ => None,
        },
        _ => None,
    }
}

/// 자가 시험용 키 이름 → winit 키
pub fn named(name: &str) -> Option<Key> {
    // 글자 한 개는 글자 키 (r·l 회전 등)
    if name.chars().count() == 1 {
        return Some(Key::Character(name.into()));
    }
    Some(Key::Named(match name {
        "left" => NamedKey::ArrowLeft,
        "right" => NamedKey::ArrowRight,
        "up" => NamedKey::ArrowUp,
        "down" => NamedKey::ArrowDown,
        "enter" => NamedKey::Enter,
        "escape" | "esc" => NamedKey::Escape,
        "backspace" => NamedKey::Backspace,
        "home" => NamedKey::Home,
        "end" => NamedKey::End,
        "pageup" => NamedKey::PageUp,
        "pagedown" => NamedKey::PageDown,
        "space" => NamedKey::Space,
        "delete" => NamedKey::Delete,
        "f2" => NamedKey::F2,
        _ => return None,
    }))
}
