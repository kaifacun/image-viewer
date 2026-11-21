# KaiIV (빠른 이미지 뷰어, 실행 파일 `kaiiv`) — 프로젝트 지식

## 환경
- Rust 는 rustup 으로 `~/.cargo` 에 설치(`--no-modify-path`) → 셸에서 `export PATH="$HOME/.cargo/bin:$PATH"` 필요.
- 버전 고정 규칙: egui/egui-wgpu/egui-winit 0.37 이 끌고 오는 **wgpu 30 / winit 0.30** 에 맞춘다. 기억 말고 `~/.cargo/registry/src/*/<crate>-<ver>/` 소스를 보고 API 를 쓴다.

## ★ 모듈 독립 규칙
- 새 포맷 = 새 `iv-decode-*` crate + `app/iv/src/decoders.rs` 등록 한 줄. 파이프라인·렌더러 수정 금지.
- 새 사용자 기능 = `iv_viewer::Action` 추가 + `ViewerState::handle` 분기 + `app/iv/src/input.rs` 단축키.
- 새 렌더러 = `iv_render::Renderer` 구현 + `app/iv/src/renderer.rs` 폴백 사슬.
- 새 화면 패널 = `iv-ui` 에 그리기 함수 (상태 변경 금지, `Action`/`BrowserOut` 반환). 화면 모드는 `ViewerState::mode` 하나가 진실.
- 썸네일은 `iv-thumbs`(앱이 조립), 폴더 트리는 `iv-fstree`(앱이 소유), 사진 폴더 스캔은 `iv-scan`(앱이 소유, 트리엔 `visible_rows_where` 술어로만 연결). iv-viewer 는 셋 다 모른다.
- `app/iv` 만 모든 crate 를 안다. 다른 crate 끼리 역방향 의존 금지 (core ← decode/pipeline/library, viewer → pipeline/library/viewport).

## ★ 비직관적 사실 (다시 겪지 말 것)
- **EXIF 방향은 디코더가 회전하지 않는다** — `Frame.orientation` 만 채우고 렌더러가 UV 변환(`Orientation::uv_transform`). ImageIO 썸네일도 `kCGImageSourceCreateThumbnailWithTransform=false`.
- **egui 텍스처 delta 는 프레임을 못 그려도 반영해야 한다** (Outdated/Occluded 등). 버리면 "Tried to update a texture that has not been allocated yet" 패닉.
- **표면은 비-sRGB + sRGB view_format**: 이미지는 sRGB 뷰(선형 필터링), egui 는 비-sRGB 뷰. egui 를 sRGB 표면에 그리면 색이 틀어진다.
- egui 0.37: `ctx.run_pass(raw, |ctx| ..)`, `Area::show(ctx, ..)`, `Id::unique`, `textures_delta.set` 은 `SmallVec<ImageDelta>`. 한글은 기본 폰트에 없어 `iv_ui::install_fonts` 가 OS 폰트(AppleSDGothicNeo.ttc)를 폴백으로 넣는다.
- wgpu 30: `queue.present(tex)`, `get_current_texture()` → `CurrentSurfaceTexture` enum, `SurfaceConfiguration.color_space`, `RequestAdapterOptions.apply_limit_buckets`(→ `..Default::default()`).
- macOS 에는 wgpu 폴백 어댑터가 없다 → `--renderer=fallback` 은 CPU 로 내려간다(정상).
- 파이프라인: 현재 장 본 디코드가 대기·진행 중이면 미리 읽기 워커는 기다린다(경합으로 현재 장이 3배 느려졌던 문제).
- image 크레이트는 JPEG 축소 디코딩 불가 → 맥은 ImageIO `CreateThumbnailAtIndex(MaxPixelSize)` 가 우선(우선순위 20 > image-rs 10).

- **egui 0.37 `TexturesDelta` 는 반영 후 `.clear()` 해야 한다** — 안 비우고 drop 하면 디버그 빌드 panic(비-unwind abort). wgpu·CPU 렌더러 모든 경로에서.
- **썸네일은 방향을 픽셀에 굽는다**(`iv_imageops::apply_orientation`) — "디코더는 회전 안 함" 규칙의 의도된 예외(작은 이미지, egui 텍스처로 바로 쓰기 위해).
- **썸네일 ↔ 전체 보기 경합은 `iv_core::Gate`** 로 중재: Loader 가 현재 장 대기·진행 중이면 busy, 썸네일 워커는 디스크 캐시 미스 때 `wait_idle(≤1.5s)`. 브라우저 모드 Loader 창은 이웃 0, 전체 보기에선 썸네일 `set_paused(true)`.
- **브라우저 미리보기 칸은 egui 가 투명으로 비우고 렌더러 이미지 경로(밉맵)로 그린다**: `Scene.image_area`(물리 px) + `iv_render::clamp_area` 필수 — 한 프레임 늦은 레이아웃이 창 밖이면 wgpu scissor 오류 → 장치 오류 → CPU 강등.
- egui 0.37 패널은 `ctx.root_ui(|ui| egui::Panel::left(..).show(ui, ..))`. `Sense::click()` 은 FOCUSABLE 포함 → 화살표·Enter 를 위젯 포커스가 삼킨다: 타일은 `Sense::CLICK`, 그린 뒤 글 입력 중이 아니면 `surrender_focus`, 키 입력은 `consumed && text_edit_focused` 일 때만 egui 몫.
- 격자는 `show_rows` 금지(실제 행 간격 = row_h + item_spacing → 스크롤 오차) → `show_viewport` + 고정 pitch 직접 배치. 클릭 선택에는 스크롤하지 않는다(더블클릭이 빗나감).
- 브라우저에선 앱의 마우스 매핑(휠 확대·드래그 이동·더블클릭 1:1)을 **모드로** 막는다 (누르는 순간 egui 가 consumed 를 안 줄 수 있음).
- CPU 렌더러의 egui 래스터라이저: 감마 공간 프리멀티 `src + dst*(1-a)`, top-left 규칙(반투명 대각선 이중 칠 방지). 디버그에서도 `iv-render-cpu` 는 opt-level 3 (아니면 UI 한 프레임 700ms).

## 시험·검증
- 화면 검증은 **앱 자가 캡처(IV_SELFTEST)** 로 한다. `screencapture` 전체 화면 캡처 금지(사용자 화면이 찍힌다).
- 시험 이미지는 `tools/make-fixtures.py` → `scratch/`(git 제외). 사용자 사진 폴더를 뒤지지 않는다.
- **`WindowEvent::RedrawRequested` 를 egui-winit 에 넘기지 않는다** — repaint=true 를 돌려줘 무한 다시 그리기(대기 중 CPU 12%).
- ControlFlow 는 `about_to_wait` 한 곳에서만 정한다. 지난 시각으로 `WaitUntil` 하면 바쁜 대기가 된다.
- ImageIO 내장 미리보기는 RAW·HEIF 에서만 쓴다. JPEG 은 내장 썸네일이 없으면 원본 전체를 디코드한다(6016px 645ms).
- 디코드 목표 크기는 창이 아니라 **모니터 긴 변**(`ViewerConfig.display_side`) — 창 크기 조절·전체 화면 때 재디코드 폭주 방지.
- 브라우저 시험은 반드시 `IV_CACHE_DIR=scratch/cache IV_TREE_ROOTS=scratch` 로 (사용자 홈·사진이 트리/썸네일 캐시에 들어가지 않게). 인자 없는 실행은 홈을 연다 — 시험에 쓰지 않는다. `IV_START_DIR` 로 시작 폴더 지정.
- **자가 시험 창은 포커스를 뺏지 않고(`with_active(false)`) 실제 키보드·마우스를 무시한다** — 예전엔 사용자가 터미널에서 친 ⌫ 가 시험 창에 들어가 상위 폴더로 올라간 뒤 사용자 파일을 지웠다. 휴지통 시험은 `IV_TRASH_DIR=scratch/trash-bin` + 사본 폴더(`scratch/trash-test`)로만, 시험 중 휴지통은 `IV_TREE_ROOTS` 밖 파일을 거부한다.
- **스캔 판정은 3상태 — 모르면 숨기지 않는다**: 건너뛴 하위(링크·다른 루트·시스템 폴더·읽기 오류)가 있고 사진을 못 찾으면 Unknown. 사진 규칙은 격자(`Library::open`), 폴더 규칙은 트리(`list_subdirs`: 점 폴더·번들 제외)와 같아야 한다. 열린 폴더와 조상은 숨기지 않는다. 시험은 `wait-scan:ms`, 캡처 로그 끝에 스캔 상태.
- FSEvents 는 `iv-scan/src/watch.rs` 직접 FFI(SDK `FSEvents.h` 기준, 콜백은 채널에 넣기만). `/` 루트는 감시 안 함(시스템 변경 폭주), 썸네일 캐시·~/Library·점 폴더 이벤트는 버린다. 열린 폴더 자동 새로고침은 브라우저 모드에서 목록이 실제로 달라졌을 때만(`refresh_if_changed`) — 전체 보기 확대 상태 보호.
- 스캔 스레드는 **QoS utility** (background 는 I/O 조임으로 stat 하나 1.4ms → 5만 폴더 78초, utility 는 11초). 결과는 `<캐시>/scan-v1.tsv` 에 저장 — 다음 실행은 폴더 mtime 만 확인하고 바뀐 폴더만 다시 나열. 저장은 '완료' 공개 **전에** (시험이 완료 보고 바로 끝내면 저장이 잘린다). 스캔 중엔 트리가 바뀌므로 **트리 줄은 `push_id(path)` 로 id 고정 + 갱신은 초당 1회·포인터 안 눌렸을 때만**(아니면 클릭이 사라진다).
- 저장된 나열 재사용은 `dir_mtime_reliable`(statfs apfs/hfs) 루트에서만. 자가 시험 창 제목은 "[자가 시험 — 입력 무시]" — 사용자가 시험 창을 진짜로 알고 클릭할 수 있다(실제로 '클릭 안 됨' 신고가 시험 창 때문이었을 수 있음).
- 사용자 홈 전체 스캔은 직접 돌리지 않는다(시험은 `IV_TREE_ROOTS=scratch`).
- 파일 작업(새 폴더·이름 바꾸기·옮기기)은 `iv-fsops` — **덮어쓰기 금지**. 흐름: UI 대화 상자(`iv-ui/dialog.rs`) → `Action::NewFolder/RenameFolder/RenameFile` → 뷰어가 `FileOp` 보관 + `AppRequest::FileOp` → 앱이 실행 → `on_file_op` 로 목록 반영, 트리는 앱이(`renamed`·`invalidate`). 새/이름 바꾼 폴더는 `keep_visible` 로 숨김 예외. 시험: `act:newfolder=|renamefile=|renamefolder=<경로>|<이름>`, `act:move=<폴더>`, `type:글`, `key:f2`, `drag:x0,y0,x1,y1`(egui 드래그 판정 경유).
- 끌어다 놓기: 타일은 `Sense::CLICK | Sense::DRAG`, 페이로드 `DragPhotos`(표식만 — 대상은 뷰어 선택), 트리 줄이 `dnd_release_payload` 로 `Action::MoveMarkedTo`. 자가 시험에서 shift-click 직후 바로 dblclick 하면 egui 가 세 번 클릭으로 본다(wait 넣기).
- **추가 코덱 엔진 = FFmpeg 별도 프로그램**(`iv-codecpack` 찾기·설치·동의, `iv-ffmpeg` 정보·포스터·변환). 앱 안에 링크하지 않는다. 기본 엔진 확장자가 아니거나 열고서 `failed()` 면 → 변환본 캐시 → 변환 `Job` → 엔진 없으면 동의 창(동의 기억 시 자동 설치). 설치: brew 우선, 없으면 고정 URL + SHA-256(`PINNED`) — 판을 올릴 땐 배포처 .sha256 과 대조해 값 갱신. 시험 env: `IV_CODEC_DIR`·`IV_CODEC_NO_SYSTEM`·`IV_CODEC_NO_INSTALL`(자가 시험은 설치 금지). 받은 실행 파일을 개발 중 직접 실행하지 않는다(검증은 받기·대조·풀기까지, ignored 시험).
- **글 문서 = 앱 `text_doc`**(`sync_text`, `textdoc.rs` 인코딩 판정: BOM → UTF-8 → EUC-KR) + `iv_ui::TextView`(마크다운은 `iv-ui/markdown.rs` — pulldown-cmark 를 직접 그림, egui_commonmark 는 egui 판에 묶여 안 씀). 글을 보는 동안 마우스는 egui 몫(`browse` 판정에 포함). 시험 `act:markdown`, `key:cmd+/`.
- **PDF 전체 보기 = 앱 소유 `PdfView`**(`sync_pdf` 멱등, 쪽 그리기는 스레드 + `UserEvent::DocWake`, `pump_pdf` 가 받아 `viewer.show_page`). 1:1 = 72dpi × `DPI_SCALE`(4). 확대로 `needed_max_side` 가 그린 크기의 1.2배를 넘으면 다시 그림. 쪽 명령은 `Action::Page(PageCmd)` → `AppRequest::Page`, PDF 가 아니면 파일 넘기기로. 시험 `act:page=N`.
- **비디오 재생 = `iv-video`(AVPlayer + AVPlayerItemVideoOutput BGRA)**, 앱이 소유(`sync_video` 가 전체 보기·비디오 모드·현재 항목에 맞춰 열고 닫음 — 멱등). 프레임은 `about_to_wait` 의 `pump_video` 가 꺼내 `viewer.show_live` 로 — 렌더러 이미지 경로 그대로. 재생 중에만 8ms 뒤 깨어나고 멈추면 깨지 않는다. `live` 항목은 파이프라인 포스터가 덮지 않는다. AVPlayer 는 메인 스레드(MainThreadMarker). 자가 시험은 음소거. 측정: `cargo run --release -p iv-video --example spike -- <영상>`.
- **종류(사진·비디오) = 묶음의 `MediaKind`** (`media_groups`: 비디오 확장자는 사진 "기타" 로 새지 않게 먼저 뺀다). 뷰어 `exts()` = 종류 ∩ 필터, 트리 비트 = 종류 ∩ 필터, 상태 합계 `text_for(종류 비트)`. 비디오 포스터는 `iv-decode-video`(레지스트리 디코더라 썸네일·미리보기가 그대로 동작). 시험 영상 `tools/make-video-fixtures.sh`, 시험 `act:kind=video`, `key:cmd+2`.
- **확장자 필터 = 형식 묶음 비트** (`iv_core::formats::image_groups`, 순서 = 스캔 비트). 스캔은 폴더마다 `Found{mask, unknown}`(하위까지)·묶음별 직접 수·`kids` 를 공개하고 `verdict_for/counts_for(p, mask)` 로 조회 — 필터 변경 시 재스캔 없음, 앱은 `tree_dirty` 만. 표식은 매번 직접|하위로 **새로 합친다**(OR 누적 금지). 감시 안 하는 곳은 늘 `unknown`(필터에서 숨김이 굳지 않게). 하위 수도 같은 `verdict_for` 로 세야 펼침 삼각형이 숨김과 맞는다. 뷰어의 모든 `Library::open` 은 `exts()`(필터∩아는 확장자). 시험: `act:filter=png,jpg` / `act:filter=`.
- 트리 루트는 `iv_fstree::Root{kind: RootKind}` — 구역(즐겨찾기·위치)·아이콘을 정한다. iCloud Drive 는 ~/Library 안이라 감시 무시 목록의 예외(`watch.rs` allow). 왼쪽 막대 시험은 **가짜 홈**: `HOME=scratch/fakehome IV_TREE_NO_VOLUMES=1` (실제 디스크·홈을 스캔하지 않게).
- 트리·스캔 모두 **Finder 숨김 플래그(UF_HIDDEN)** 폴더 제외(`iv_fstree::finder_hidden`) — ~/Library 가 스캔 건너뜀(Unknown)이라 숨김 안 돼 하위가 줄줄이 보였다. 폴더 규칙을 바꾸면 `persist.rs` MAGIC 버전을 올린다.
- 클립보드는 `iv-clipboard`(NSPasteboard: 항목마다 파일 URL + 절대 경로 글). **자가 시험은 `IV_CLIPBOARD_FILE` 없이는 클립보드를 안 쓴다**(사용자 클립보드 보호). 잘라내기 = 앱이 `cut_paths`+변경 번호를 기억, 붙여넣을 때 클립보드가 그대로면 `move_into`, 아니면 `copy_into`(덮어쓰기 금지, "이름 사본.jpg"). 시험: `key:cmd+c|cmd+x|cmd+v`, `act:copy|cut|paste|paste=<폴더>|copypath|rotatemarked=1`, `rclick:x,y`. ⌥ 등 키 기호는 `install_fonts` 가 Apple Symbols 를 대체 글꼴로 넣어 해결. 아이콘은 `iv-ui/icons.rs` 선 그리기, 색·크기는 `iv-ui/theme.rs` 상수만 쓴다. 미리보기 칸이 있는 패널(정보 패널)은 **바탕을 직접 칠하되 미리보기 칸은 비운다**(패널 fill 이 렌더러 이미지를 덮는다).
- 휴지통은 `iv-trash`(NSFileManager `trashItemAtURL`). 실패 시 `remove_file` 로 대체 금지.
- **폴더 휴지통은 "모든 파일" 보기(`viewer.all_files()`)에서만**: UI 확인 창(`Dialog::trash_folder` — 하위 세기는 백그라운드, 닫히면 취소) → `Action::TrashFolder{path, ident}` → 앱 `trash_folder` 가 `iv_fsops::folder_trash_block`(UI 비활성 판정과 같은 함수)·`selftest_allows`·`IV_TRASH_DIR`·dev+ino(`dialog::folder_ident`) 재확인 → `iv_trash::trash`. 성공 시 `tree.removed`·keep_visible/cut_paths 접두사 정리. 시험은 `scratch/trash-test` 사본 + `act:trashfolder=<경로>`.
- **`/Volumes` 자체에 UF_HIDDEN 이 붙어 있다** — "조상 중 Finder 숨김이면 금지" 를 그대로 하면 외장 디스크 폴더가 전부 막힌다. 숨김 검사는 디스크·홈(구성 요소 3개) 아래·트리 루트 아래만. 폴더 휴지통의 dev+ino 재확인은 마지막 검사~`trashItemAtURL` 사이 틈을 완전히 닫지 못한다(경로 기반 API의 한계 — 휴지통이라 되살릴 수 있음).
- **"모든 파일"(`all_files`)은 종류와 직교하는 토글** — `SetKind` 가 건드리지 않는다(⌘4 = `ToggleAllFiles`). 사용자가 '모드가 아니라 체크로' 를 원함.
- **"모든 파일" 판정은 둘**: `decodable`(등록 확장자 — 썸네일·브라우저 미리보기·파이프라인) vs `viewable`(지금 종류 — 전체 보기·`step`). 종류가 다른 파일로 전체 보기에 들어가면 재생기·쪽 넘김(모두 `viewer.kind()` 에 묶임)이 죽는다. 썸네일 요청 키(`last_requested`)에서도 decodable 아닌 것을 빼야 `wait-thumbs` 가 끝난다.
- **우선 스캔**: 앱이 트리 재생성 때 `counts_for==None`(하위를 다 못 훑음) 줄을 깊은 것부터(방금 펼친·연 폴더의 자식 맨 앞) `scanner.prioritize` — 스캐너는 본 순회의 하위 루프 사이에서 그 폴더를 중첩 walk(`accept`: tried·cur·스택 접두사·범위·로컬·canonical). 트리 재생성은 1초 간격이라 반영은 1~2초. 시험 트리 `scratch/deeptree`(폴더 4만, 전체 8.8초).
- **회전은 뷰어 상태(`turns`)** — 프레임에 굽지 않는다(파이프라인이 미리보기→본 디코드로 프레임을 갈아 끼우면 사라짐). `show()` 한 곳에서 `Orientation::rotated_quarters` 로 입히고, 다른 장·목록으로 가면 버린다. 렌더러(wgpu `upload`, CPU 축소 사본)는 **같은 픽셀이어도 방향은 매번 따라가야** 한다.
- 돌려 저장은 `iv-rotate`: ImageIO `CopyImageSource` + `kCGImageDestinationOrientation`(JPEG·HEIC·PNG·TIFF 모두 크기 거의 그대로 = 재압축 없음). 점 임시 파일 → 다시 열어 방향·크기 확인 → xattr·ACL·권한 비트 복사(**COPYFILE_STAT 금지** — 시각·잠금 플래그 uchg 까지 옮겨 임시 파일이 잠긴 채 남았다) → **mtime 을 지금으로**(JPEG 은 크기가 같아 시각까지 같으면 EntryId·썸네일 캐시가 못 알아챔) → rename. 잠긴 원본은 미리 거부. 저장 방향 = 파일에 적힌 방향 + 돌린 양(`FileOp::Rotate{quarters}`). 시험 키: `key:r`, `key:l`, `key:cmd+s`.
- 자가 시험 실입력: `click=x,y` `dblclick=x,y` `cmdclick:x,y` `shiftclick:x,y`(egui ModifiersChanged 주입) `scroll=x,y,dy`(점 좌표, egui 맞음 판정 경유), `key=right|enter|escape…`(단축키 표 경유), `wait-thumbs:ms`, `act:folder=|select=|activate=|toggle=|range=|selectall|trash|sort=size,desc|view|browse`.
- 화면 검증: 앱 자가 캡처(IV_SELFTEST) 또는 `tools/winid.swift` 로 kaiiv 창 ID 를 얻어 `screencapture -l <id>` (창 단위만).

## 대표 여정 (시나리오 검수)
- 전체 보기: 폴더 열기 → 사람 속도로 수십 장 넘기기(캐시 즉시) → 확대·이동 → 정보 창 → 장치 손실 모의(lost) → CPU 로 계속 넘기기·확대.
- 브라우저: 3000장 폴더(`scratch/many`) 열기 → 썸네일 채워짐 → 화살표 이동 → Enter 전체 보기 → 넘김 → Esc 복귀(마지막 장 보이게) → End/스크롤 → 더블클릭·클릭(실입력) → 정렬 → 트리로 다른 폴더 → 재실행 시 디스크 캐시 즉시 → `--renderer=cpu` 로 같은 여정.
- **창 초점(`BrowserState.focus: Pane`)** — 누른 창(`pane_focus`)·Tab 으로 바뀐다. 앱 `pane_key` 가 `key_action` **앞에서** 트리 초점의 방향키·Home/End·Enter·⌫ 를 가로챈다(자가 시험 `key:` 경로도 같은 함수). 펼침 판정은 줄 목록(한 프레임 늦음)이 아니라 `tree.is_expanded` 로 — 아니면 연타한 → 가 펼쳤다 접는다. 시험 `key:tab`.
- **인앱 업데이트 = `iv-update`(앱 UI 모름) + 앱 `update.rs`**. 피드 `releases/latest/download/update.json` 은 Ed25519 서명(판·url·크기·sha256·min_os·알림 글 해시 모두) — 공개키는 `iv_update::PUBLIC_KEY`, 비밀키는 배포자 키체인(서비스 `KaiIV-update-ed25519`)에만, **화면·저장소·인자에 절대 내보내지 않는다**(잃으면 기존 사용자에게 업데이트를 못 보냄). ad-hoc 서명은 신원이 아니라 Ed25519 가 유일한 인증 → 피드·키·대상 env(`IV_UPDATE_FEED/PUBKEY/TARGET/STATE/NO_OPEN`)는 **디버그 빌드만**(릴리스 바이너리 strings 로 확인). 배포는 `make-app.sh --universal --dmg --update` 세 파일을 **한 번의** `gh release create` 로(latest 가 update.json 없는 릴리스를 가리키지 않게).
- 교체: 대상 옆 난수 0700 `.KaiIV-update-*` 에 풀고 검사 → 앱이 `kaiiv --apply-update` 도우미를 띄우고 종료(stdin 쓰기 끝을 `mem::forget` — 앱이 끝나야 EOF, pid 재사용 걱정 없음) → `renamex_np(RENAME_SWAP)` 한 번으로 맞바꿈(대상 경로가 비는 틈 없음), open 요청 실패면 다시 맞바꿔 되돌림. 풀 때 `lipo -archs` 로 이 맥 아키텍처 검사(`--update` 는 `--universal` 만). 지금까지 본 가장 높은 판(`highest`)보다 낮은 서명 판은 철회된 판 재생으로 보고 무시. 청소는 10분 넘은 단계 폴더만. 자가 시험은 확인·적용 안 함(`IV_UPDATE_FEED` 를 준 디버그 시험만 확인, 적용은 늘 생략). 시험: `scratch/update-test.noindex` 앱 사본 + 시험 키(`IV_UPDATE_KEY_FILE`) 서명 zip, `act:update=check|now|later|skip|restart`, 도우미는 `IV_UPDATE_NO_OPEN=1 target/debug/kaiiv --apply-update …` 로 직접. **실제 /Applications 앱에 시험 적용 금지.**
- 서명 키 옮기기: `kaiiv-update-sign export <파일>`(키체인 → 0600 파일, git 작업 폴더 안 거부, 덮어쓰기 거부) / `import <파일>`(앱 PUBLIC_KEY 와 맞을 때만 키체인에, 다른 키가 있으면 거부). 사용자 사본은 `~/.kaiiv/kaiiv-update-signing.key`(저장소 밖) — USB 로 다른 맥에 옮겨 import. 비밀값을 화면·로그에 꺼내지 않는다.
- **빌드한 .app 은 `.noindex` 폴더에만**(`dist/app.noindex/KaiIV.app`, 시험 사본도 `scratch/*.noindex/`) — 아니면 Spotlight 에 KaiIV.app 이 여러 개 떠 사용자가 헷갈린다(실제로 4개가 떴다). 시험 끝나면 시험용 .app 사본은 지운다.

