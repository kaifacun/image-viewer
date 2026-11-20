# 트리 "확인 중" 표시·우선 스캔 + "모든 파일" 모드 + 폴더 휴지통 (v0.4.22 예정)

사용자 요청 (2026-11-20):
1. 사진 없는 폴더가 첫 스캔 동안 계속 보이는 문제 → 아직 확인 안 된 폴더는 흐리게 + "확인 중" 안내, 펼친 폴더 하위부터 먼저 훑기.
2. (클립보드 이미지 — 사용자가 하지 말라 함. 범위 밖)
3. 폴더 삭제 버튼. 단, "모든 파일 보기"를 켠 상태(폴더 안 다른 파일도 보이는 모드)에서만 가능.

## 현재 구조 (추적 결과)
- 트리 숨김: `app/iv/src/window_app.rs:1055~1072` — `hide = !show_all && 열린 폴더·조상 아님 && keep_visible 아님 && verdict_for(p, mask)==Empty`. 숫자 = `scanner.counts_for(p, mask)` → `BrowserModel.tree_counts: Vec<Option<(u32,u32)>>`. **`None` = 그 폴더 하위 전체를 아직 다 훑지 않음** (walk 는 후위 순회라 하위가 끝나야 `set_info`).
- 트리 그리기: `crates/iv-ui/src/browser.rs:643 folder_tree` — `counted = m.tree_counts.get(k)`.
- 스캔: `crates/iv-scan/src/lib.rs` 스레드 1개가 루트별 DFS `Walker::walk` (재귀, 후위). 우선순위 없음. `list()` 는 `cur`(이번 실행)·`prev`(저장) 를 mtime 일치 시 재사용 → 두 번 들러도 디스크 재나열 없음.
- 상태줄 오른쪽에 이미 `scan_status` ("사진 찾는 중… 폴더 n개").
- 목록: `iv_library::Library::open(path, &exts)` — 확장자 집합으로만 거름. 뷰어 `exts()` = 종류 ∩ 필터 (`crates/iv-viewer/src/lib.rs:328`), 호출처 `open`·`refresh`·`refresh_if_changed`.
- 종류 전환: `Action::SetKind`, UI `segmented_kind` (browser.rs:512), 단축키 ⌘1~3 (`input.rs:83`).
- 파일 작업 흐름: UI → `Action` → 뷰어 `request(FileOp)` → `AppRequest::FileOp` → 앱 `file_op()` (window_app.rs:676, `selftest_allows` 검사) → `viewer.on_file_op`. 휴지통은 `iv_trash::trash(path)` (실패 시 remove 대체 금지).
- 트리 행 오른쪽 클릭 메뉴: browser.rs:748 (새 폴더·이름 바꾸기(depth>0)·붙여넣기·경로 복사).

## A. 트리 "확인 중" + 우선 스캔
- [ ] `crates/iv-ui/src/browser.rs` `BrowserModel` 에 `scan_done: bool` 추가 → 앱이 `self.scanner.status().done` 로 채움 (window_app.rs `BrowserModel {..}` 생성부, 1090 근처).
- [ ] `folder_tree`: `counted.is_none() && !m.scan_done` 인 줄은 이름·아이콘을 `theme::TEXT_FAINT` 계열로 흐리게, 오른쪽 숫자 자리에 "…", 툴팁 "사진이 있는지 확인 중". 열린 폴더(current)는 흐리게 하지 않음.
- [ ] `folder_tree` 맨 위(첫 구역 제목 위)에 `!m.scan_done` 일 때 한 줄 안내: "폴더 확인 중 — 흐린 폴더는 아직 확인 전" (작은 글씨). 끝나면 사라짐.
- [ ] `iv-scan`: `Shared.priority: Mutex<Vec<PathBuf>>` + `Scanner::prioritize(paths: Vec<PathBuf>)` (트리 경로 → `key()` 정규화, 통째 교체).
- [ ] `Walker`: 현재 훑는 경로 스택 `stack: Vec<PathBuf>`. `walk()` 의 하위 루프 각 단계 전(= yield_now 뒤)에 `take_priority()`: 큐에서 하나 꺼내 — 이미 `cur` 에 있음 / 스택 위에 있음(지금 훑는 중인 조상·자신) / 루트 밖 / is_system → 버리고 다음; 아니면 `self.walk(p, false)` 를 그 자리에서 실행 (중첩). 나중에 본 순회가 그 폴더에 오면 `cur` 재사용으로 디스크 재나열 없음.
- [ ] 첫 훑기 끝난 뒤(`follow`)엔 우선 큐 무시 (전부 앎).
- [ ] 앱: 트리 행을 다시 만들 때(window_app.rs 1066 블록) `!scan_done` 이면 `counts_for == None` 인 보이는 행 경로를 **깊은 것부터**(depth 내림차순) 최대 64개 모아 `scanner.prioritize(..)`. (펼친 폴더의 자식 = 가장 깊은 줄 → 먼저 훑힘. 홈 같은 큰 루트는 마지막.) 스캔 진행 → `tree_scan_changed` → 행 재생성 → 우선 목록 갱신 (자기 순환).
- 검증: `IV_TREE_ROOTS=scratch` 에 깊은 가짜 트리(사진 없는 폴더 수천 개 + 끝쪽에 사진 없는 폴더 몇 개)를 만들어, 시작 직후 끝쪽 폴더를 펼쳤을 때 그 자식이 본 순회보다 먼저 숨겨지는지 (`wait`, 캡처, 로그).

## B. "모든 파일" 모드
- 의미: 격자에 그 폴더의 **모든 파일**(숨김 제외), 트리에 **모든 폴더**(사진 없는 폴더도 — 지울 폴더를 고르려면 보여야 함). 이 모드에서만 폴더 휴지통·아무 파일 휴지통 가능.
- [ ] `iv_library::Library::open` 에 "전부" 선택지: `pub enum ExtFilter<'a> { Only(&'a HashSet<&'static str>), All }` 대신 최소 변경으로 `pub fn open_all(path)` 추가 (내부 공용 함수에 `Option<&HashSet>`). 기존 `open` 시그니처 유지.
- [ ] `iv-viewer`: `all_files: bool` 필드, `pub fn all_files()`, `Action::SetAllFiles(bool)` (켜면 필터 유지·종류 유지, 목록 다시 나열 = refresh 와 같은 선택 유지 로직). `Action::SetKind` 는 `all_files=false` 로.
  - 목록을 여는 3곳(`open`·`refresh`·`refresh_if_changed`) → `fn open_lib(&self, p)` 하나로.
  - `viewable(entry)` = 확장자가 `self.extensions`(등록된 모든 형식)에 있음.
  - `window()`: 현재 항목이 볼 수 없는 파일이면 `current: None` (파이프라인이 디코드 시도하지 않게).
  - `EnterView`/`Activate`: 볼 수 없는 파일이면 전체 보기로 가지 않고 `notice = "이 형식은 볼 수 없습니다"` ⚠️확인필요(기본 앱으로 열기는 범위 밖 — advisor 판단).
  - 전체 보기 `step()`: 볼 수 없는 파일은 건너뜀 (순환 유지, 최대 len 번).
- [ ] 썸네일: 앱 `after_browser` 의 썸네일 요청 키 목록에서 볼 수 없는 항목 제외. 타일은 썸네일 대신 문서 아이콘 + 확장자 글자(대문자), 해상도 칸엔 크기. "열 수 없음" 으로 보이면 안 됨.
- [ ] 트리: 앱 hide 술어에 `|| viewer.all_files()` 를 show_all 처럼 (숨기지 않음). 숫자는 그대로(사진 수).
- [ ] UI: `segmented_kind` 에 4번째 칸 "모든 파일"(Icon::Folder) — 켜지면 그 칸만 강조, 종류 칸 누르면 해제. 단축키 ⌘4 (`input.rs`). 보기 메뉴에도 체크 항목.
- [ ] 상태줄: 모든 파일 모드면 "파일 n개 (용량)".

## C. 폴더 휴지통
- [ ] `iv-viewer`: `FileOp::TrashFolder { path }`, `Action::TrashFolder(PathBuf)` — `all_files` 가 아니면 무시(notice "모든 파일 보기에서만 폴더를 지울 수 있습니다").
- [ ] UI 확인 대화 상자 `iv-ui/dialog.rs` `DialogKind::TrashFolder { path, summary }` — 열 때 하위를 세어(파일 수·폴더 수·용량, 최대 2만 항목/0.5초에서 멈추고 "이상") 보여 주고 "휴지통으로 이동"/"취소". Enter=확인 아님(실수 방지), Esc=취소.
- [ ] 진입점: (1) 트리 행 오른쪽 클릭 "휴지통으로 이동…" (모든 파일 모드가 아니면 비활성 + 툴팁 이유), (2) 파일 메뉴 "이 폴더를 휴지통으로…", (3) 도구 막대에 모든 파일 모드일 때만 보이는 "폴더 삭제" 버튼(대상 = 열린 폴더).
- [ ] 금지 대상 (UI 비활성 + 앱에서 다시 검사): 트리 루트(depth 0: 홈·데스크탑·문서·다운로드·사진·동영상·iCloud·디스크), `/`, 홈 자신과 그 조상, 볼륨 루트(`/Volumes/X`), 경로 정규화 후 판정.
- [ ] 앱 `file_op`: `FileOp::TrashFolder` → `selftest_allows` (+ 자가 시험은 `IV_TRASH_DIR` 필수, 기존 trash 와 같은 규칙) → 금지 대상 재검사 → `iv_trash::trash(path)`. 성공 시 `tree.invalidate(parent)`, `keep_visible` 에서 제거, `tree_dirty`. 스캔은 FSEvents 가 반영.
- [ ] 뷰어 `on_file_op(TrashFolder)`: 열린 폴더가 그 폴더이거나 하위면 → 부모 폴더 열기 (선택은 지운 폴더 자리 근처 없음 — 첫 항목). 아니면 열린 폴더가 부모면 refresh(하위 폴더 수). notice "'이름' 폴더를 휴지통으로 옮김".
- [ ] 실패: notice/error 로 이유 표시, 삭제 대체 없음.

## DoD (관측)
1. `scratch` 큰 가짜 트리로 첫 실행: 확인 전 폴더는 흐리게 + 트리 위 안내 → 스캔 끝나면 안내 사라지고 사진 없는 폴더 숨김. 펼친 폴더 자식이 먼저 정리됨(로그·캡처).
2. "모든 파일"(⌘4) → 격자에 .md·.pdf·.txt·확장자 없는 파일까지, 트리에 빈 폴더까지. 볼 수 없는 파일 더블클릭 → 안내, 전체 보기에서 넘기면 건너뜀. 사진 칸(⌘1) → 원래대로.
3. 모든 파일 모드에서 `scratch/trash-test/x` 폴더 오른쪽 클릭 → 휴지통으로 → 확인 창(개수) → 이동 → `IV_TRASH_DIR` 에 들어감, 트리·격자에서 사라짐, 열려 있었으면 부모로.
4. 사진 모드에선 메뉴 비활성 + 이유. 루트 행은 비활성.
5. 단위 시험 + 기존 시험 통과, 릴리스 빌드, CPU 렌더러로도 확인.

## advisor 자문 반영 — 확정 (2026-11-20)
- A 중첩 walk 승인 + 조건: 본 루프에서 `cur.contains_key(sub)` 면 `known(sub)`; 스택은 접두사 판정(`stack.any(s.starts_with(p))`); `in_priority` 중이면 큐 안 꺼냄; `tried` 집합; 받을 경로 = normalize 의 검사(숨김·번들·finder_hidden·skip) + 루트 아님 + 로컬 볼륨 + `canonicalize()==p`(링크 줄 제외); 스택 push 는 list 성공 뒤, pop 은 set_info 직전. 앱은 루트·링크 줄을 큐에 넣지 않고, 방금 펼친/연 폴더 자식을 맨 앞에.
- B: `all_files: bool`(MediaKind 확장 안 함). 판정 둘: `decodable` = 등록 확장자(썸네일·Browse 미리보기·파이프라인), `viewable` = `kind_of_path == Some(kind)`(전체 보기·step). 종류 다른 등록 형식 → "비디오 보기(⌘2)에서 열 수 있습니다", 미등록 → "이 형식은 볼 수 없습니다". SetAllFiles → Browse 강제·필터 None·filter_menu 비활성. SetKind 조기 반환 조건 `k==kind && !all_files`, UI on = `kind==k && !all_files`. window(): Browse=decodable, View=viewable, 이웃도 거름. First/Last/on_removed/refresh 뒤 View 현재 항목 보정. RotateMarked·SaveRotation 대상 사진만. `last_requested` = decodable 만(wait-thumbs). has_arrow: show_all_folders 면 `row.has_children`. 상태줄 항목은 이미 있음(삭제).
- C: `iv_fsops::folder_trash_block(path, roots, protected) -> Option<&'static str>` 하나를 UI·앱이 공유 — 링크, 루트의 조상·자신, 시스템·~/Library, 숨김·번들 구성요소, 캐시·IV_TRASH_DIR·~/.Trash 의 조상·자신, 구성요소 3개 미만. 하위 세기는 백그라운드 스레드(취소 플래그), 확인 버튼은 바로 사용 가능, 기본 포커스 취소. TrashFolder 는 Browse 에서만. 성공 시 `FolderTree::removed`, keep_visible·cut_paths 접두사 정리. 시험 중 IV_TRASH_DIR 필수. 원래 경로로 휴지통 이동. selftest `kind=all`·`act:trashfolder=`. iv_trash move_to_dir 이름 겹치면 번호(🟡).
- 🟢 차후: 기본 앱으로 열기, Scanner::forget_path, 격자 폴더 타일, open_all stat 비용.
