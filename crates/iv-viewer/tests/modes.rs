//! 브라우저 ↔ 전체 보기 상태 전이 시험 (창·렌더러 없이)

use std::path::PathBuf;
use std::sync::Arc;

use iv_core::DecoderRegistry;
use iv_pipeline::{Loader, LoaderConfig};
use iv_viewer::{Action, Mode, SortKey, ViewerConfig, ViewerState};

fn setup(tag: &str, n: usize) -> (PathBuf, Loader, ViewerState) {
    let d = std::env::temp_dir().join(format!("iv-modes-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("sub")).unwrap();
    for i in 0..n {
        // 크기를 달리해 정렬 시험에 쓴다
        let w = 8 + (n - i) as u32;
        let px = vec![200u8; (w * 8 * 4) as usize];
        image::save_buffer(d.join(format!("img{i:02}.png")), &px, w, 8, image::ExtendedColorType::Rgba8).unwrap();
    }
    let mut r = DecoderRegistry::new();
    r.register(iv_decode_image::ImageCrateDecoder);
    let r = Arc::new(r);
    let ext = r.known_extensions();
    let loader = Loader::new(r, LoaderConfig::default(), || {});
    let v = ViewerState::new([800.0, 600.0], ext, ViewerConfig::default());
    (d, loader, v)
}

#[test]
fn browse_view_round_trip() {
    let (d, l, mut v) = setup("trip", 10);
    let fx = v.open(&d, &l);
    assert_eq!(v.mode, Mode::Browse);
    assert!(fx.library_changed && fx.reveal && fx.window_changed);
    assert_eq!(v.library.subdir_count(), 1);
    // 브라우저에서는 이웃 미리 읽기 없음 (썸네일과 경합 방지)
    let w = v.window();
    assert!(w.ahead.is_empty() && w.behind.is_empty() && w.current.is_some());
    // 격자 이동: 3열에서 아래 = +3, 경계에서 멈춤
    v.grid_cols = 3;
    let fx = v.handle(Action::GridMove { dx: 0, dy: 1 }, &l);
    assert!(fx.reveal);
    assert_eq!(v.library.index(), 3);
    let _ = v.handle(Action::GridMove { dx: 0, dy: 10 }, &l);
    assert_eq!(v.library.index(), 9);
    let _ = v.handle(Action::GridMove { dx: -1, dy: 0 }, &l);
    assert_eq!(v.library.index(), 8);
    // 확대·이동은 브라우저에서 무시
    let fx = v.handle(Action::ZoomIn, &l);
    assert!(!fx.redraw);
    // 클릭 선택은 스크롤하지 않음 (더블클릭 보호)
    let fx = v.handle(Action::Select(2), &l);
    assert!(!fx.reveal);
    // 더블클릭 → 전체 보기, 이웃 미리 읽기 시작
    let fx = v.handle(Action::Activate(5), &l);
    assert_eq!(v.mode, Mode::View);
    assert_eq!(v.library.index(), 5);
    assert!(fx.window_changed);
    assert!(!v.window().ahead.is_empty());
    let _ = v.handle(Action::Next, &l);
    assert_eq!(v.library.index(), 6);
    // Esc → 브라우저, 마지막 본 장이 보이게
    let fx = v.handle(Action::ExitView, &l);
    assert_eq!(v.mode, Mode::Browse);
    assert!(fx.reveal);
    assert_eq!(v.library.index(), 6);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn sort_refresh_and_folder_errors() {
    let (d, l, mut v) = setup("sort", 6);
    let _ = v.open(&d, &l);
    let _ = v.handle(Action::Select(4), &l);
    let name = v.current().unwrap().name.clone();
    let fx = v.handle(Action::Sort(SortKey::Size, true), &l);
    assert!(fx.library_changed && fx.reveal);
    assert_eq!(v.current().unwrap().name, name, "정렬 뒤에도 선택 유지");
    // (PNG 압축 때문에 너비 순 ≠ 바이트 순 — 바이트 순인지만 본다)
    let lens: Vec<u64> = v.library.entries().iter().map(|e| e.len).collect();
    assert!(lens.windows(2).all(|w| w[0] <= w[1]), "크기 오름차순: {lens:?}");
    // 새 파일 + 새로고침: 선택·정렬 유지
    std::fs::copy(d.join("img00.png"), d.join("zz.png")).unwrap();
    let fx = v.handle(Action::Refresh, &l);
    assert!(fx.library_changed);
    assert_eq!(v.library.len(), 7);
    assert_eq!(v.current().unwrap().name, name);
    assert_eq!(v.library.sort_order(), (SortKey::Size, true));
    // 하위 폴더(사진 없음) → 빈 격자, 상위로 복귀
    let _ = v.handle(Action::OpenFolder(d.join("sub")), &l);
    assert!(v.library.is_empty());
    assert_eq!(v.mode, Mode::Browse);
    let _ = v.handle(Action::ParentFolder, &l);
    assert_eq!(v.library.len(), 7);
    // 없는 폴더 → 빈 목록 + 오류 (트리와 격자가 어긋나지 않게)
    let fx = v.handle(Action::OpenFolder(d.join("nope")), &l);
    assert!(fx.library_changed);
    assert!(v.library.is_empty() && v.error.is_some());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn file_arg_opens_view_and_no_browse_without_ui() {
    let (d, l, mut v) = setup("file", 3);
    let _ = v.open(&d.join("img01.png"), &l);
    assert_eq!(v.mode, Mode::View);
    assert_eq!(v.library.index(), 1);
    let _ = v.handle(Action::ExitView, &l);
    assert_eq!(v.mode, Mode::Browse);
    // UI 를 못 그리는 렌더러: 폴더를 열어도 전체 보기, Esc 무시
    v.browse_available = false;
    let _ = v.open(&d, &l);
    assert_eq!(v.mode, Mode::View);
    let _ = v.handle(Action::ExitView, &l);
    assert_eq!(v.mode, Mode::View);
    let _ = std::fs::remove_dir_all(&d);
}

/// 여러 장 선택: ⌘ 토글 · ⇧ 범위 · 그냥 클릭은 하나로 · 정렬해도 유지 · 다 풀면 휴지통 대상 없음
#[test]
fn multi_select_rules() {
    let (d, l, mut v) = setup("sel", 8);
    let _ = v.open(&d, &l);
    let names = |v: &ViewerState| {
        let mut n: Vec<String> = v.trash_targets().iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        n.sort();
        n
    };
    let _ = v.handle(Action::Select(1), &l);
    assert_eq!(names(&v), ["img01.png"]);
    let _ = v.handle(Action::ToggleSelect(3), &l);
    assert_eq!(names(&v), ["img01.png", "img03.png"]);
    // 범위는 마지막 기준(3)부터
    let _ = v.handle(Action::RangeSelect(5), &l);
    assert_eq!(names(&v), ["img03.png", "img04.png", "img05.png"]);
    // 정렬해도 선택 유지
    let _ = v.handle(Action::Sort(SortKey::Size, true), &l);
    assert_eq!(names(&v).len(), 3);
    let _ = v.handle(Action::Sort(SortKey::Name, true), &l);
    // 이미 현재 항목을 그냥 클릭해도 여러 장 선택이 풀린다
    let cur = v.library.index();
    let fx = v.handle(Action::Select(cur), &l);
    assert!(fx.redraw);
    assert_eq!(names(&v).len(), 1);
    // 마지막 하나도 풀면 휴지통 대상이 없다 (회색 테두리 항목을 지우지 않음)
    let _ = v.handle(Action::ToggleSelect(cur), &l);
    assert!(v.trash_targets().is_empty());
    assert_eq!(v.handle(Action::Trash, &l).app, None);
    // 모두 선택
    let _ = v.handle(Action::SelectAll, &l);
    assert_eq!(v.trash_targets().len(), 8);
    // 전체 보기에서는 보고 있는 장만
    let _ = v.handle(Action::EnterView, &l);
    assert_eq!(v.trash_targets().len(), 1);
    let _ = std::fs::remove_dir_all(&d);
}

/// 휴지통 결과 반영: 성공한 것만 빠지고 다음 장으로, 실패는 오류로 남는다
#[test]
fn on_trashed_removes_only_successes() {
    let (d, l, mut v) = setup("trash", 6);
    let _ = v.open(&d, &l);
    let _ = v.handle(Action::Select(1), &l);
    let _ = v.handle(Action::RangeSelect(3), &l);
    let t = v.trash_targets();
    assert_eq!(t.len(), 3);
    // 두 장만 성공했다고 알린다
    let fx = v.on_trashed(&t[..2], &["img03.png (권한 없음)".to_string()], &l);
    assert!(fx.library_changed);
    assert_eq!(v.library.len(), 4);
    assert_eq!(v.current().unwrap().name, "img03.png");
    assert!(v.error.as_deref().unwrap_or("").contains("1개"));
    let _ = std::fs::remove_dir_all(&d);
}
