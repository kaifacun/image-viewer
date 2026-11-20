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
- **확장자 필터 = 형식 묶음 비트** (`iv_core::formats::image_groups`, 순서 = 스캔 비트). 스캔은 폴더마다 `Found{mask, unknown}`(하위까지)·묶음별 직접 수·`kids` 를 공개하고 `verdict_for/counts_for(p, mask)` 로 조회 — 필터 변경 시 재스캔 없음, 앱은 `tree_dirty` 만. 표식은 매번 직접|하위로 **새로 합친다**(OR 누적 금지). 감시 안 하는 곳은 늘 `unknown`(필터에서 숨김이 굳지 않게). 하위 수도 같은 `verdict_for` 로 세야 펼침 삼각형이 숨김과 맞는다. 뷰어의 모든 `Library::open` 은 `exts()`(필터∩아는 확장자). 시험: `act:filter=png,jpg` / `act:filter=`.
- 트리·스캔 모두 **Finder 숨김 플래그(UF_HIDDEN)** 폴더 제외(`iv_fstree::finder_hidden`) — ~/Library 가 스캔 건너뜀(Unknown)이라 숨김 안 돼 하위가 줄줄이 보였다. 폴더 규칙을 바꾸면 `persist.rs` MAGIC 버전을 올린다.
- 클립보드는 `iv-clipboard`(NSPasteboard: 항목마다 파일 URL + 절대 경로 글). **자가 시험은 `IV_CLIPBOARD_FILE` 없이는 클립보드를 안 쓴다**(사용자 클립보드 보호). 잘라내기 = 앱이 `cut_paths`+변경 번호를 기억, 붙여넣을 때 클립보드가 그대로면 `move_into`, 아니면 `copy_into`(덮어쓰기 금지, "이름 사본.jpg"). 시험: `key:cmd+c|cmd+x|cmd+v`, `act:copy|cut|paste|paste=<폴더>|copypath|rotatemarked=1`, `rclick:x,y`. 메뉴 글에 ⌥ 기호 금지(글꼴에 없어 네모) — 아이콘은 `iv-ui/icons.rs` 선 그리기.
- 휴지통은 `iv-trash`(NSFileManager `trashItemAtURL`). 실패 시 `remove_file` 로 대체 금지.
- **회전은 뷰어 상태(`turns`)** — 프레임에 굽지 않는다(파이프라인이 미리보기→본 디코드로 프레임을 갈아 끼우면 사라짐). `show()` 한 곳에서 `Orientation::rotated_quarters` 로 입히고, 다른 장·목록으로 가면 버린다. 렌더러(wgpu `upload`, CPU 축소 사본)는 **같은 픽셀이어도 방향은 매번 따라가야** 한다.
- 돌려 저장은 `iv-rotate`: ImageIO `CopyImageSource` + `kCGImageDestinationOrientation`(JPEG·HEIC·PNG·TIFF 모두 크기 거의 그대로 = 재압축 없음). 점 임시 파일 → 다시 열어 방향·크기 확인 → xattr·ACL·권한 비트 복사(**COPYFILE_STAT 금지** — 시각·잠금 플래그 uchg 까지 옮겨 임시 파일이 잠긴 채 남았다) → **mtime 을 지금으로**(JPEG 은 크기가 같아 시각까지 같으면 EntryId·썸네일 캐시가 못 알아챔) → rename. 잠긴 원본은 미리 거부. 저장 방향 = 파일에 적힌 방향 + 돌린 양(`FileOp::Rotate{quarters}`). 시험 키: `key:r`, `key:l`, `key:cmd+s`.
- 자가 시험 실입력: `click=x,y` `dblclick=x,y` `cmdclick:x,y` `shiftclick:x,y`(egui ModifiersChanged 주입) `scroll=x,y,dy`(점 좌표, egui 맞음 판정 경유), `key=right|enter|escape…`(단축키 표 경유), `wait-thumbs:ms`, `act:folder=|select=|activate=|toggle=|range=|selectall|trash|sort=size,desc|view|browse`.
- 화면 검증: 앱 자가 캡처(IV_SELFTEST) 또는 `tools/winid.swift` 로 kaiiv 창 ID 를 얻어 `screencapture -l <id>` (창 단위만).

## 대표 여정 (시나리오 검수)
- 전체 보기: 폴더 열기 → 사람 속도로 수십 장 넘기기(캐시 즉시) → 확대·이동 → 정보 창 → 장치 손실 모의(lost) → CPU 로 계속 넘기기·확대.
- 브라우저: 3000장 폴더(`scratch/many`) 열기 → 썸네일 채워짐 → 화살표 이동 → Enter 전체 보기 → 넘김 → Esc 복귀(마지막 장 보이게) → End/스크롤 → 더블클릭·클릭(실입력) → 정렬 → 트리로 다른 폴더 → 재실행 시 디스크 캐시 즉시 → `--renderer=cpu` 로 같은 여정.
