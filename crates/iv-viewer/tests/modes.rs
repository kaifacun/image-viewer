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

/// 파일 작업 반영: 사진 이름 바꾸기는 선택을 따라가고, 열린 폴더 이름 바꾸기는 새 경로로 다시 연다
#[test]
fn file_ops_follow_selection() {
    use iv_viewer::FileOp;
    let (d, l, mut v) = setup("fops", 4);
    let _ = v.open(&d, &l);
    let _ = v.handle(Action::Select(1), &l);
    let old = d.canonicalize().unwrap().join("img01.png");
    let fx = v.handle(Action::RenameFile { path: old.clone(), name: "zz.png".into() }, &l);
    assert!(fx.app.is_some());
    let op = v.take_file_op().unwrap();
    let new = old.with_file_name("zz.png");
    std::fs::rename(&old, &new).unwrap();
    let _ = v.on_file_op(&op, &Ok(new.clone()), &l);
    // 이름순 정렬로 맨 뒤로 갔고, 현재 항목이 따라갔다
    assert_eq!(v.current().unwrap().name, "zz.png");
    assert_eq!(v.library.index(), 3);
    assert_eq!(v.trash_targets(), vec![new]);
    // 오류는 상태로
    let _ = v.on_file_op(&FileOp::RenameFile { path: old.clone(), name: "x".into() }, &Err("이미 있음".into()), &l);
    assert_eq!(v.error.as_deref(), Some("이미 있음"));
    // 열린 폴더 이름 바꾸기
    let dir = d.canonicalize().unwrap();
    let moved = dir.with_file_name(format!("{}-renamed", dir.file_name().unwrap().to_string_lossy()));
    std::fs::rename(&dir, &moved).unwrap();
    let _ = v.on_file_op(&FileOp::RenameFolder { path: dir.clone(), name: String::new() }, &Ok(moved.clone()), &l);
    assert_eq!(v.library.dir(), Some(moved.as_path()));
    assert_eq!(v.current().unwrap().name, "zz.png");
    let _ = std::fs::remove_dir_all(&moved);
}

/// 선택 상자: 새로 그리면 그 항목들만, ⌘/⇧ 로 시작하면 기존 선택에 더함, 빈 곳 클릭은 모두 풀기
#[test]
fn marquee_select_rules() {
    let (d, l, mut v) = setup("mq", 8);
    let _ = v.open(&d, &l);
    let _ = v.handle(Action::Select(0), &l);
    let _ = v.handle(Action::MarqueeSelect { indices: vec![2, 3], keep: false, begin: true }, &l);
    assert_eq!(v.trash_targets().len(), 2);
    // 끄는 중 상자가 줄면 빠진다
    let _ = v.handle(Action::MarqueeSelect { indices: vec![2], keep: false, begin: false }, &l);
    assert_eq!(v.trash_targets().len(), 1);
    // ⌘ 로 새 상자: 기존(2)에 더함
    let _ = v.handle(Action::MarqueeSelect { indices: vec![5, 6], keep: true, begin: true }, &l);
    assert_eq!(v.trash_targets().len(), 3);
    let _ = v.handle(Action::SelectNone, &l);
    assert!(v.trash_targets().is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

/// 회전: 화면에만 입히고(파이프라인이 프레임을 바꿔 넣어도 유지), 다른 장·목록으로 가면 버리며,
/// ⌘S 는 돌린 양을 파일 작업으로 내고, 저장 성공 뒤에는 새 항목 id 로 같은 화면을 이어 간다.
#[test]
fn rotation_is_view_state_until_saved() {
    use iv_core::Orientation;
    use iv_viewer::FileOp;
    let (d, l, mut v) = setup("rot", 3);
    let _ = v.open(&d, &l);
    let _ = v.handle(Action::Activate(0), &l);
    l.set_window(&v.window());
    let t = std::time::Instant::now();
    while v.shown.is_none() && t.elapsed().as_secs() < 5 {
        for ev in l.drain() {
            let _ = v.on_load_event(ev);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let size = v.shown.as_ref().expect("불러오기").frame.display_full_size();
    assert_ne!(size[0], size[1], "가로·세로가 다른 그림이어야 시험이 된다");
    // 아직 돌리지 않았으면 저장할 것이 없다
    assert!(v.handle(Action::SaveRotation, &l).app.is_none());
    let _ = v.handle(Action::RotateCw, &l);
    assert_eq!(v.turns(), 1);
    let sh = v.shown.as_ref().unwrap();
    assert_eq!(sh.frame.orientation, Orientation::Rotate90);
    assert_eq!(sh.frame.display_full_size(), [size[1], size[0]]);
    assert!(v.status_line("t").contains("90° 돌림"));
    // 반시계 두 번 = 시계 세 번 = 270°
    let _ = v.handle(Action::RotateCcw, &l);
    let _ = v.handle(Action::RotateCcw, &l);
    assert_eq!(v.turns(), 3);
    // 저장 요청: 돌린 양 그대로
    let fx = v.handle(Action::SaveRotation, &l);
    assert!(fx.app.is_some());
    let op = v.take_file_op().unwrap();
    let path = v.current().unwrap().path().unwrap().to_path_buf();
    assert_eq!(op, FileOp::Rotate { path: path.clone(), quarters: 3 });
    // 저장 성공 (파일 mtime 이 바뀐 것으로) → 회전 상태는 파일 몫, 화면 방향은 그대로
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&path, std::fs::read(&path).unwrap()).unwrap();
    let old_key = v.shown.as_ref().unwrap().key;
    let _ = v.on_file_op(&op, &Ok(path.clone()), &l);
    assert_eq!(v.turns(), 0);
    let sh = v.shown.as_ref().unwrap();
    assert_ne!(sh.key, old_key, "파일이 바뀌어 새 id");
    assert_eq!(Some(sh.key), v.current().map(|e| e.id.0));
    assert_eq!(sh.frame.orientation, Orientation::Rotate270);
    // 프레임이 아직 없을 때 저장해도 회전은 파일 몫이 된다 (새 프레임에 두 번 입히지 않게)
    let _ = v.handle(Action::RotateCw, &l);
    let _ = v.handle(Action::SaveRotation, &l);
    let op = v.take_file_op().unwrap();
    v.shown = None;
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&path, std::fs::read(&path).unwrap()).unwrap();
    let _ = v.on_file_op(&op, &Ok(path.clone()), &l);
    assert_eq!(v.turns(), 0);
    // 돌리고 다른 장으로 가면 버린다
    let _ = v.handle(Action::RotateCw, &l);
    let _ = v.handle(Action::Next, &l);
    assert_eq!(v.turns(), 0);
    // 돌리고 목록으로 가도 버린다, 목록에서는 돌리지 않는다
    let _ = v.handle(Action::RotateCw, &l);
    let _ = v.handle(Action::ExitView, &l);
    assert_eq!(v.turns(), 0);
    assert!(!v.handle(Action::RotateCw, &l).redraw);
    let _ = std::fs::remove_dir_all(&d);
}

/// 일괄 회전·붙여넣기 결과 반영과 클립보드 요청 규칙
#[test]
fn batch_rotate_and_paste_results() {
    use iv_viewer::{AppRequest, ClipOp, FileOp};
    let (d, l, mut v) = setup("clip", 4);
    let _ = v.open(&d, &l);
    let dir = d.canonicalize().unwrap();
    // 선택이 없으면 복사 요청 없음, 경로 복사는 폴더 경로
    let _ = v.handle(Action::SelectNone, &l);
    assert!(v.handle(Action::Copy, &l).app.is_none());
    assert_eq!(v.handle(Action::CopyPath, &l).app, Some(AppRequest::Clipboard(ClipOp::CopyPath)));
    assert_eq!(v.path_targets(), vec![dir.clone()]);
    // 두 장 골라 일괄 회전 요청
    let _ = v.handle(Action::Select(1), &l);
    let _ = v.handle(Action::ToggleSelect(2), &l);
    assert_eq!(v.handle(Action::Copy, &l).app, Some(AppRequest::Clipboard(ClipOp::Copy)));
    let _ = v.handle(Action::RotateMarked(1), &l);
    assert_eq!(v.take_file_op(), Some(FileOp::RotateMarked { quarters: 1 }));
    // 회전 저장 결과: 파일이 바뀌면 id 가 바뀌지만 선택은 그대로 두 장
    let targets = v.trash_targets();
    std::thread::sleep(std::time::Duration::from_millis(20));
    for p in &targets {
        std::fs::write(p, std::fs::read(p).unwrap()).unwrap();
    }
    let fx = v.on_rotated(&targets, &[], &l);
    assert!(fx.library_changed);
    assert_eq!(v.trash_targets(), targets);
    assert!(v.browse_status(None, "t").contains("2장 돌려 저장함"));
    // 붙여넣기: 열린 폴더에 새 파일이 생기면 새로고침하고 그것들을 선택
    let _ = v.handle(Action::Paste(None), &l);
    assert_eq!(v.take_file_op(), Some(FileOp::Paste { dest: dir.clone() }));
    let new = dir.join("img01 사본.png");
    std::fs::copy(dir.join("img01.png"), &new).unwrap();
    let _ = v.on_pasted(&dir, &[new.clone()], &[], &[], &l);
    assert_eq!(v.library.len(), 5);
    assert_eq!(v.trash_targets(), vec![new.clone()]);
    // 잘라내기로 다른 폴더에 옮겼으면 목록에서 빠진다
    let sub = dir.join("sub");
    let moved = sub.join("img01 사본.png");
    std::fs::rename(&new, &moved).unwrap();
    let _ = v.on_pasted(&sub, &[moved], &[new], &[], &l);
    assert_eq!(v.library.len(), 4);
    let _ = std::fs::remove_dir_all(&d);
}

/// 확장자 필터: 목록·선택·휴지통 대상이 필터 안의 것만 — 휴지통·옮기기·잘라내기 안전 조건
#[test]
fn extension_filter_limits_everything() {
    let (d, l, mut v) = setup("filt", 3);
    std::fs::copy(d.join("img00.png"), d.join("photo.jpg")).unwrap();
    std::fs::copy(d.join("img00.png"), d.join("other.JPG")).unwrap();
    let _ = v.open(&d, &l);
    assert_eq!(v.library.len(), 5);
    let _ = v.handle(Action::Select(1), &l);
    let cur = v.current().unwrap().name.clone();
    let fx = v.handle(Action::SetFilter(Some(vec!["png"])), &l);
    assert!(fx.library_changed);
    assert_eq!(v.library.len(), 3);
    assert!(v.library.entries().iter().all(|e| e.name.ends_with(".png")));
    assert_eq!(v.current().unwrap().name, cur, "남아 있는 현재 장은 지킨다");
    let _ = v.handle(Action::SelectAll, &l);
    assert!(v.trash_targets().iter().all(|p| p.extension().unwrap() == "png"));
    // jpg 만 (대소문자 무시)
    let _ = v.handle(Action::SetFilter(Some(vec!["jpg"])), &l);
    assert_eq!(v.library.len(), 2);
    // 빈 목록 = 필터 없음
    let _ = v.handle(Action::SetFilter(Some(vec![])), &l);
    assert!(v.filter().is_none());
    assert_eq!(v.library.len(), 5);
    let _ = std::fs::remove_dir_all(&d);
}

/// 종류 전환: 사진 모드엔 비디오가, 비디오 모드엔 사진이 안 보이고, 비디오 파일을 열면 비디오 모드
#[test]
fn media_kind_switch() {
    use iv_core::formats::MediaKind;
    let (d, l, mut v) = setup("kind", 2);
    std::fs::write(d.join("clip.mp4"), b"x").unwrap();
    let img: std::collections::HashSet<&'static str> = ["png"].into_iter().collect();
    let vid: std::collections::HashSet<&'static str> = ["mp4"].into_iter().collect();
    v.set_kinds(vec![(MediaKind::Image, img), (MediaKind::Video, vid)]);
    let _ = v.open(&d, &l);
    assert_eq!(v.library.len(), 2);
    let _ = v.handle(Action::SetFilter(Some(vec!["png"])), &l);
    let fx = v.handle(Action::SetKind(MediaKind::Video), &l);
    assert!(fx.library_changed);
    assert_eq!(v.kind(), MediaKind::Video);
    assert!(v.filter().is_none(), "종류를 바꾸면 필터는 풀린다");
    assert_eq!(v.library.entries().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["clip.mp4"]);
    let _ = v.handle(Action::SetKind(MediaKind::Image), &l);
    assert_eq!(v.library.len(), 2);
    // 비디오 파일을 직접 열면 비디오 모드
    let _ = v.open(&d.join("clip.mp4"), &l);
    assert_eq!(v.kind(), MediaKind::Video);
    let _ = std::fs::remove_dir_all(&d);
}

/// 전체 보기의 다음·이전은 순환 (끝 → 처음, 처음 → 끝), 브라우저 격자 이동은 끝에서 멈춤
#[test]
fn view_navigation_wraps() {
    let (d, l, mut v) = setup("wrap", 3);
    let _ = v.open(&d, &l);
    let _ = v.handle(Action::Activate(2), &l);
    assert_eq!(v.mode, Mode::View);
    let _ = v.handle(Action::Next, &l);
    assert_eq!(v.library.index(), 0, "끝에서 다음 → 처음");
    let _ = v.handle(Action::Prev, &l);
    assert_eq!(v.library.index(), 2, "처음에서 이전 → 끝");
    let _ = v.handle(Action::ExitView, &l);
    let _ = v.handle(Action::GridMove { dx: 1, dy: 0 }, &l);
    assert_eq!(v.library.index(), 2, "격자는 끝에서 멈춤");
    let _ = std::fs::remove_dir_all(&d);
}

/// "모든 파일" 보기: 사진 아닌 파일도 나열, 전체 보기는 볼 수 있는 것만, 폴더 휴지통은 이 보기에서만
#[test]
fn all_files_view_and_folder_trash_gate() {
    let (d, l, mut v) = setup("all", 3);
    std::fs::write(d.join("a-note.txt"), "x").unwrap();
    std::fs::write(d.join("img01b.md"), "# x").unwrap();
    let _ = v.open(&d, &l);
    assert_eq!(v.library.len(), 3);
    // 사진 보기에선 폴더 휴지통 거부 (요청 없음, 안내)
    let fx = v.handle(Action::TrashFolder { path: d.join("sub"), ident: None }, &l);
    assert!(fx.app.is_none() && v.notice.is_some());
    let fx = v.handle(Action::SetAllFiles(true), &l);
    assert!(fx.library_changed && v.all_files());
    assert_eq!(v.library.len(), 5, "txt·md 도 보여야 함");
    // 볼 수 없는 파일: 전체 보기로 가지 않고 안내, 파이프라인에도 넘기지 않음
    let note = v.library.entries().iter().position(|e| e.name == "a-note.txt").unwrap();
    let _ = v.handle(Action::Select(note), &l);
    assert!(v.window().current.is_none());
    let _ = v.handle(Action::EnterView, &l);
    assert_eq!(v.mode, Mode::Browse);
    assert!(v.notice.is_some());
    // 사진에서 전체 보기 → 넘기면 볼 수 없는 파일은 건너뜀 (순환)
    let first = v.library.entries().iter().position(|e| e.name == "img00.png").unwrap();
    let _ = v.handle(Action::Activate(first), &l);
    assert_eq!(v.mode, Mode::View);
    for _ in 0..7 {
        let _ = v.handle(Action::Next, &l);
        let cur = v.library.current().unwrap();
        assert!(cur.name.ends_with(".png"), "건너뛰지 않음: {}", cur.name);
        assert!(v.window().ahead.iter().chain(&v.window().behind).count() <= 3);
    }
    let _ = v.handle(Action::ExitView, &l);
    // 이 보기에선 폴더 휴지통 요청이 앱으로 간다
    let fx = v.handle(Action::TrashFolder { path: d.join("sub"), ident: None }, &l);
    assert!(fx.app.is_some());
    // 종류를 바꿔도 "모든 파일" 토글은 그대로 (비디오 종류에서도 모든 파일이 보인다)
    let fx = v.handle(Action::SetKind(iv_core::formats::MediaKind::Video), &l);
    assert!(fx.library_changed && v.all_files());
    assert_eq!(v.library.len(), 5);
    let _ = v.handle(Action::SetKind(iv_core::formats::MediaKind::Image), &l);
    // ⌘4 로 끄면 그 종류 파일만
    let fx = v.handle(Action::ToggleAllFiles, &l);
    assert!(fx.library_changed && !v.all_files());
    assert_eq!(v.library.len(), 3);
    let _ = std::fs::remove_dir_all(&d);
}
