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
