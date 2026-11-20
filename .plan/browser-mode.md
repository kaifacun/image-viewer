# 브라우저 모드 (FastStone 형) — 설계·지침서 v1

## 목표 (끝난 상태)
`iv <폴더>` 또는 인자 없이 실행 → **브라우저 화면**:
- 위: 메뉴(파일·보기·정렬) + 도구모음(폴더 열기·위로·새로고침·썸네일 크기·정렬) + 경로 줄
- 왼쪽 위: 폴더 트리 (지연 펼침) / 왼쪽 아래: 선택 이미지 미리보기 + 간단 정보
- 가운데: 썸네일 격자 (파일명·해상도), 가상화 스크롤
- 아래: 상태줄 (폴더 N · 파일 M · 총 용량 · 선택 파일 정보)
- 썸네일 더블클릭/Enter → 지금의 **전체 보기**, Esc → 브라우저 (선택=마지막으로 본 장, 격자가 그 장으로 스크롤)
- 파일 인자로 실행(`iv a.jpg`)하면 전체 보기로 바로, Esc 하면 그 폴더 브라우저
- CPU 렌더러에서도 같은 화면 (egui 소프트웨어 래스터라이저)

## 새 crate / 변경 (의존 방향 유지)

| crate | 역할 | 의존 |
|---|---|---|
| **iv-thumbs** (신설) | 썸네일 서비스: 보이는 목록 우선 생성 워커풀 + 메모리 LRU + 디스크 캐시. 결과는 **방향 적용된** RGBA (egui 텍스처로 바로) + 원본 크기 | iv-core, iv-imageops |
| **iv-fstree** (신설) | 폴더 트리 모델: 루트(홈·데스크탑·사진·/Volumes/*), 하위 폴더 지연 나열·캐시, 경로까지 펼치기 | std 만 |
| iv-library | `sort(SortKey, asc)`, `subdir_count`, `total_bytes`, Entry 에 `len`·`mtime` 보관 | (변경 없음) |
| iv-viewer | `Mode{Browse,View}`, 브라우저 Action, 미리보기 viewport | + iv-thumbs? ✗ (모름) |
| iv-ui | `browser::draw(ctx, &BrowserModel, &mut ThumbTextures) -> BrowserOut` (Action + 보이는 범위 + 미리보기 사각형) | egui, iv-viewer |
| iv-render | `Scene.image_area: Option<[f32;4]>` (물리 px 사각형; None=전체) | |
| iv-render-wgpu | 이미지 패스를 image_area 로 viewport+scissor | |
| iv-render-cpu | image_area 반영 + **egui 메시 래스터라이저** → `UiCapability::Full` | + egui(epaint) |
| app/iv | ThumbService 조립·drain, 모드별 입력 매핑, 보이는 범위→thumbs.request, 시작 모드 | 전부 |

iv-viewer 는 iv-thumbs 를 모른다: 썸네일은 앱이 서비스에서 꺼내 iv-ui 의 `ThumbTextures` 에 넣는다.

## iv-thumbs API
```rust
pub struct ThumbConfig { side: u32 /*320*/, mem_bytes: usize /*256MB*/, disk_dir: Option<PathBuf>, disk_bytes: u64 /*1GB*/, workers: usize /*cores/2, 1..=3*/ }
pub struct Thumb { pub key: u64, pub image: Arc<Frame> /*방향 적용, Rgba8*/, pub full_size: [u32;2] /*표시 방향 기준*/ }
pub enum ThumbEvent { Ready(Thumb), Failed{key, error} }
impl ThumbService {
  pub fn new(registry: Arc<DecoderRegistry>, cfg, wake: impl Fn()+Send+Sync) -> Self;
  /// 보이는 순서대로(우선) + 여유분. 목록에 없는 대기 작업은 버림(진행 중은 끝까지 → 캐시)
  pub fn request(&self, items: &[(u64, ImageSource, DiskKey)]);
  pub fn get(&self, key) -> Option<Thumb>;     // 메모리 캐시
  pub fn drain(&self) -> Vec<ThumbEvent>;
  pub fn set_paused(&self, bool);               // 전체 보기 중엔 멈춤 → 넘기기 속도 보호
}
```
- 생성: `registry.decode(src, SizeHint::max_side(side))` (ImageIO 축소 디코드 ~10–30ms) → `iv_imageops::apply_orientation`(신설, 작은 이미지만) .
- 디스크 키: **안정 해시**(FNV-1a 64: 절대경로 바이트 + mtime ns + len + side + 캐시 버전). `DefaultHasher` 는 버전 간 안정 보장이 없어 쓰지 않음.
- 디스크 형식: `thumbs-v1/ab/abcdef….ivt` = 헤더(magic, w,h, full_w, full_h) + JPEG q85(불투명) / PNG(알파 있음). 임시파일 → rename. 읽기 실패·손상은 무시하고 재생성.
- 위치: mac `~/Library/Caches/iv/`, win `%LOCALAPPDATA%\iv\cache\`, 그 외 `$XDG_CACHE_HOME`/`~/.cache/iv`. 시작 시 백그라운드로 용량 초과분 오래된 순 삭제.
- wake 프로토콜은 Loader 와 동일 (`pending: AtomicBool`).

## iv-viewer 변경
- `pub mode: Mode` (`Browse` | `View`), `pub preview: Viewport`(미리보기 칸 크기, FitAlways), `pub grid_cols: usize`(UI 가 매 프레임 알려줌), `pub browse_dir_changed` 없음 — Effects 에 `library_changed: bool` 추가 (앱이 썸네일 요청·텍스처 정리).
- Action 추가: `EnterView`(현재 선택), `ExitView`, `Select(usize)`, `Activate(usize)`(=Select+EnterView), `GridMove{dx:isize, dy:isize}`, `OpenFolder(PathBuf)`(브라우즈 유지), `ParentFolder`, `Refresh`, `Sort(SortKey, bool)`.
- `Open(path)`: 파일이면 View, 폴더면 Browse.
- 브라우즈 모드에서도 Loader 창 = 선택 장(+이웃) @ 모니터 크기 → 미리보기 칸은 GPU 축소 표시, Enter 시 **즉시** 전체 보기.
- status_line 은 모드별.

## 렌더 배선
- Browse: Scene{viewport: &viewer.preview, image: 선택 장, image_area: Some(미리보기 칸 물리 rect), ui: 브라우저 egui}. egui 는 미리보기 칸을 비워 둠(투명) → 아래 이미지가 보임. 순서: 이미지(클리어 포함) → egui. 패널 배경이 이미지를 덮지 않도록 미리보기 칸은 egui 가 그리지 않는다.
- View: 지금과 동일 (image_area None).
- 미리보기 칸 rect 는 **직전 egui 프레임 레이아웃 결과**(BrowserOut.preview_rect, points×ppp)를 viewer.preview.set_screen 에 반영 → 한 프레임 지연(크기 바뀔 때만, 허용).

## iv-ui browser
- `ThumbTextures`: key → (TextureHandle, full_size). `insert(ctx, Thumb)`, `retain_visible(range)` 상한 800장 넘으면 보이는 범위 밖부터 해제. 라이브러리 바뀌면 clear.
- 격자: `ScrollArea::vertical().show_rows(ui, row_h, rows, |ui, range| …)` → 보이는 행 범위를 BrowserOut 에 담음. 타일: 정사각 칸에 비율 유지 이미지(없으면 회색 자리), 아래 파일명(말줄임)·해상도(`4032×3024`, 모르면 빈칸). 선택 타일 강조. 클릭=Select, 더블클릭=Activate. 선택이 바뀌면(키보드) scroll_to 로 보이게.
- 트리: `CollapsingState` 로 지연 펼침, 클릭=OpenFolder. 현재 폴더 경로 강조·자동 펼침.
- 메뉴/도구모음/경로 줄/상태줄: Action 만 낸다.

## CPU egui 래스터라이저 (iv-render-cpu/src/egui_raster.rs)
- 텍스처 맵 `HashMap<TextureId, Tex{w,h,Vec<[u8;4]>}>` — `textures_delta.set` 의 부분 갱신(pos) 반영, `free` 제거. 그리지 못한 프레임에도 delta 반영.
- 메시: 삼각형별 bbox ∩ clip_rect(점→물리 px), 무게중심 좌표, uv 보간 → **쌍선형** 샘플(글꼴 가독성), 정점색 곱, **감마 공간 프리멀티플라이드 블렌드** (egui-wgpu 의 비-sRGB 표면과 같은 결과). 행 띠 단위 rayon.
- egui Callback primitive 는 무시.

## 앱 배선 (window_app.rs) — 누가 언제 호출하나
- [ ] `App::new`: `ThumbService::new(registry.clone(), cfg, wake→UserEvent::ThumbWake)`.
- [ ] `user_event(ThumbWake)`: `thumbs.drain()` → `ThumbTextures.insert` (egui ctx 가 있을 때) / 미보유 키는 버림 → redraw.
- [ ] `build_ui`: 모드 Browse 면 `iv_ui::browser::draw` → BrowserOut{actions, visible: Range, preview_rect, grid_cols}. 보이는 범위가 바뀌었거나 library_changed 면 `thumbs.request(visible ± 2행)`. 텍스처 없는 보이는 키는 `thumbs.get()` 로 먼저 채움.
- [ ] `apply(fx)`: `fx.library_changed` → 텍스처 clear + 다음 프레임에 request. 모드 전환 → `thumbs.set_paused(mode==View)`.
- [ ] 입력: `input::key_action(key, mods, fullscreen, mode)` — Browse: 화살표=GridMove, Enter=EnterView, Backspace=ParentFolder, ⌘R=Refresh. View: Esc(전체화면 아님)=ExitView, 나머지 기존.
- [ ] 마우스: Browse 모드에선 휠·드래그·더블클릭을 egui 가 처리(격자 스크롤) — 기존 Pan/Zoom 매핑은 View 모드에서만.
- [ ] 렌더러 교체(switch_renderer) 시 egui ctx 새로 → ThumbTextures 재생성(clear) 후 다시 request.
- [ ] 시작 모드: 인자가 파일 → View, 폴더 → Browse, 없음 → Browse(사진 폴더 → 홈).
- [ ] selftest: `act:browse|view|select=i|activate=i|folder=path|grid=dx,dy|sort=name|date|size`, `wait-thumbs`.

## 시험
- 단위: fstree(임시 폴더), library sort, thumbs 디스크 왕복·손상 파일·키 안정성, egui_raster(사각형·텍스처·클립 → 기대 픽셀), viewer 모드 전환·GridMove 경계.
- 실창: 300장 폴더 브라우즈 → 썸네일 채워지는 시간, 빠른 스크롤, 더블클릭→즉시 전체 보기→Esc 복귀 위치, 재실행 시 디스크 캐시로 즉시, CPU 강제에서 같은 여정.

## 열린 질문
1. 폴더 나열이 느린 네트워크 볼륨에서 UI 스레드를 막음 → v1 은 동기 + 캐시, 별도 스레드는 차후?
2. 격자에 하위 폴더 타일도 보일지 (FastStone 은 보임) → v1 트리만.
3. 썸네일 320px 고정 vs 크기 슬라이더 따라 다단 → v1 320 고정, 표시만 확대/축소.

## 1차 자문 반영 (advisor 2026-11-19)
- **사용자 사진 보호**: `IV_CACHE_DIR`(썸네일 캐시 위치), `IV_TREE_ROOTS`(트리 루트, `:` 구분) 환경변수 오버라이드. 모든 시험은 `scratch/` 폴더·`scratch/cache`·루트 `scratch/` 로만. 인자 없는 실행 시험 금지.
- **썸네일 생성 순서**: `registry.preview(src, max_side(320))` (RAW·HEIF 내장 미리보기, 320 으로 축소되어 수락) → 없거나 거부되면 `registry.decode(src, max_side(320))`.
- **경합 방지**: Browse 모드 Loader 창 = 선택 장만 (ahead/behind 0, 모니터 크기). 이웃 미리 읽기는 `EnterView` 때. View 모드에선 썸네일 서비스 `set_paused(true)`.
- **배선 추가**:
  - BrowserOut 의 `preview_rect`·`grid_cols` 가 직전과 다르면 앱이 viewer 갱신 + `request_redraw`.
  - `Effects { library_changed }` 를 `merge`·`apply` 조건에 포함.
  - 격자 스크롤은 위젯 scroll_to 가 아니라 `ScrollArea::vertical_scroll_offset(row * row_h)` (show_rows 는 화면 밖 행을 배치하지 않음) — `scroll_to: Option<usize>` 를 UI 상태로 한 번만 적용.
  - CPU `capture()` 도 UI 를 그린다 (CPU 화면 검증용).
- **selftest 실입력**: `click=x,y`, `dblclick=x,y`, `scroll=dy`, `key=name` 을 egui raw_input / winit 경로로 주입 (Action 직접 주입은 보조). 대표 여정은 실입력으로.
- **순서**: ① GPU 브라우저 전 여정 완성·캡처 → ② CPU 래스터라이저. ①과 ② 사이 CPU(StatusOnly)면 View 모드 강제.
- 래스터라이저 블렌드·감마·ImageData 형식은 registry 의 egui-wgpu 0.37 셰이더/epaint 소스로 맞춘다.
- fixture: 3000장 폴더(`scratch/many`, 하드링크) 추가.
- docs/architecture.md 낡은 부분 함께 갱신.

## 2차 자문(advisor Agent) + 교차 검증(GPT) 반영 — 확정
합의:
- 전역 우선순위 관문 `iv_core::Gate` (Loader 가 현재 장 대기·진행 시 busy, 썸네일 워커는 디스크 캐시 미스 시 wait_idle(≤1.5s)). ✅ 구현
- 디스크 캐시: 128비트 이름 + 헤더의 경로·mtime·len 대조(충돌 시 남의 썸네일 금지), 임시파일 pid+스레드 → rename, 백그라운드 정리(새 파일 제외). ✅ 구현
- 실패 집합: 재요청 안 함, 타일에 경고, 새로고침 시 forget_failures.
- 썸네일 요청 = 매 프레임 "보이는 ±2행 키 목록"을 직전과 비교해 다를 때만 request (정렬·열 수·스크롤·폴더 변경을 한 규칙으로).
- 텍스처: 바이트 상한 96MB, 보이는 범위 밖부터 해제, 프레임당 삽입 48장, 텍스처 삽입은 build_ui(run_pass) 안에서. ThumbWake 는 ctx 없어도 drain.
- 해상도 표시는 썸네일 결과 full_size 로 점진 (`info()` 일괄 스캔 안 함).
- Effects: `library_changed`, `reveal` 추가. `apply` 에서 `thumbs.set_paused(mode==View)` 매번(멱등).
- 격자: `show_rows` 대신 `ScrollArea::vertical().show_viewport` + 고정 pitch 직접 배치, 타일 `ui.interact(rect, id, Sense::CLICK)`(포커스 불가). reveal 은 화면 밖일 때만 최소 이동, 한 프레임만 vertical_scroll_offset. 클릭 선택엔 스크롤 안 함(더블클릭 보호).
- 트리 상태 단일 원천 = 앱 소유 FolderTree, UI 는 그리기만(`tree_toggle` 반환). CollapsingState 안 씀.
- 패널: 왼쪽 Panel `Frame::NONE`, 그 안 bottom 미리보기 패널 `Frame::NONE`(투명) + 트리 영역은 채움.
- image_area = 물리 px [x0,y0,x1,y1], 타깃 안으로 클램프, 비었으면 이미지 안 그림(scissor 범위 초과 → wgpu 오류 → CPU 강등 방지). 첫 Browse 프레임은 Some(빈 rect).
- `show()` 가 preview viewport 도 set_image (Fit, 작은 그림 확대 안 함). set_preview_screen 은 window_changed 안 냄.
- Browse 모드에선 앱 마우스 매핑(줌·팬·더블클릭) 전체 차단 (consumed 가 아니라 모드로).
- 포커스: 그린 뒤 text_edit 포커스가 아니면 surrender_focus; KeyboardInput 은 consumed && text_edit_focused 일 때만 무시.
- CPU: 모든 경로에서 delta set 은 그리기 전, free 는 그린 뒤. 래스터: 감마 공간 프리멀티 `src + dst*(1-a)`, 정점색×텍스처, uv*size-0.5 쌍선형 clamp, Nearest 옵션 존중, clip round(clip*ppp) 클램프, 픽셀 중심 + top-left rule, dithering 생략(시험 오차 ±2), 단색 축정렬 사각형 빠른 경로.
- selftest: Capture 도 redraw 와 같은 경로(액션 처리). 실입력 주입은 프레임 나눠서. 값 소문자화는 동작 이름만.
- 무인자 시작: 홈(또는 IV_START_DIR) Browse. 파일 인자 → View, Esc 시 그 폴더 Browse + 트리 expand_to.
- 폴더 열기 실패(Browse): 빈 라이브러리 + 상태줄 오류 (트리와 격자 어긋남 방지).
- 격자 키: 화살표, PageUp/Down(보이는 행), Home/End, Enter, Backspace(상위).
이견:
- GPT: "CPU Full UI 는 별도 마일스톤/검증된 백엔드 채택 검토". 최종판단: 외부 소형 crate(egui_software_backend 0.0.3, 파생판들)는 성숙도·공급망 위험이 커서 자체 구현(~300줄, 단위 시험)으로 하되 GPU 완성 뒤 2단계로 분리. 그 사이 CPU 면 View 강제.
- GPT: "썸네일 별도 워커풀 대신 전역 permit". 최종판단: 별도 풀 유지 + Gate 로 중재(advisor 도 분리 지지). 진행 중 썸네일 1개는 짧아(320px) 허용.
차후(🟢): 설정 저장(마지막 폴더·패널 폭·정렬), 백그라운드 폴더 나열, 하위 폴더 타일, 타이핑 이동, 우클릭 Finder.
