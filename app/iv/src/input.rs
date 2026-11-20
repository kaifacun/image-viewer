//! 키 → Action 매핑 (단축키 표). 새 단축키는 여기에만 추가한다.

use iv_viewer::{Action, Mode};
use winit::keyboard::{Key, ModifiersState, NamedKey};

pub fn key_action(key: &Key, mods: ModifiersState, fullscreen: bool, mode: Mode) -> Option<Action> {
    let cmd = mods.super_key() || mods.control_key();
    match key {
        Key::Named(n) => match (mode, n) {
            (_, NamedKey::Escape) if fullscreen => Some(Action::ToggleFullscreen),
            // 브라우저: 격자 이동
            (Mode::Browse, NamedKey::ArrowRight) => Some(Action::GridMove { dx: 1, dy: 0 }),
            (Mode::Browse, NamedKey::ArrowLeft) => Some(Action::GridMove { dx: -1, dy: 0 }),
            (Mode::Browse, NamedKey::ArrowDown) => Some(Action::GridMove { dx: 0, dy: 1 }),
            (Mode::Browse, NamedKey::ArrowUp) => Some(Action::GridMove { dx: 0, dy: -1 }),
            (Mode::Browse, NamedKey::PageDown) => Some(Action::GridPage(1)),
            (Mode::Browse, NamedKey::PageUp) => Some(Action::GridPage(-1)),
            (Mode::Browse, NamedKey::Enter | NamedKey::Space) => Some(Action::EnterView),
            (Mode::Browse, NamedKey::Backspace) => Some(Action::ParentFolder),
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
            ("f", false, _) => Some(Action::ToggleFullscreen),
            ("=" | "+", _, Mode::View) => Some(Action::ZoomIn),
            ("-" | "_", _, Mode::View) => Some(Action::ZoomOut),
            ("0", _, Mode::View) => Some(Action::Fit),
            ("9", _, Mode::View) => Some(Action::FitAlways),
            ("1", _, Mode::View) => Some(Action::ActualSize),
            ("i", false, Mode::View) => Some(Action::ToggleInfo),
            _ => None,
        },
        _ => None,
    }
}

/// 자가 시험용 키 이름 → winit 키
pub fn named(name: &str) -> Option<Key> {
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
        _ => return None,
    }))
}
