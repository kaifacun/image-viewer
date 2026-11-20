# 빠른 이미지 뷰어 — 아키텍처

## 목표
- 맥 우선, 이후 Windows. Rust 단일 코드베이스 + GPU(wgpu) 렌더링.
- "웬만한 이미지는 다" 열기: JPEG·PNG·GIF·WebP·HEIC·AVIF·카메라 RAW·SVG·JXL·TIFF·PSD …
- **모듈·기능 독립**: 새 포맷·새 기능을 추가할 때 기존 구조를 바꾸지 않고 crate/모듈 하나를 더하는 것으로 끝나야 한다.

## 버전 (2026-11-18 해석 기준)
egui / egui-wgpu / egui-winit 0.37 → wgpu 30.0.x, winit 0.30.x (egui가 끌고 오는 버전에 정확히 맞춘다).

## Workspace 구성

```
crates/
  iv-core        포맷·GPU·UI 무관한 공용 타입과 trait (의존성 최소)
  iv-decode-image    image 크레이트 기반 디코더 (JPEG/PNG/GIF/WebP/BMP/TIFF/ICO …)  ※ 순수 Rust, 전 플랫폼
  iv-decode-imageio  macOS ImageIO 디코더 (HEIC/AVIF/RAW/…; OS 하드웨어 디코딩)  ※ cfg(target_os="macos")
  (이후) iv-decode-wic(Windows) · iv-decode-svg(resvg) · iv-decode-jxl · iv-decode-raw(libraw) · iv-decode-turbojpeg
  iv-imageops    축소 리샘플(fast_image_resize)·방향 굽기(썸네일용)
  iv-pipeline    디코드 작업 큐(워커 스레드) + 세대(generation) id + 미리 읽기 창 + 메모리 상한 LRU
  iv-thumbs      썸네일 서비스: 보이는 목록 우선 워커 + 메모리 LRU + 디스크 캐시 (Gate 로 전체 보기에 양보)
  iv-library     폴더 스캔·정렬(이름·날짜·크기·형식)·현재 위치·지원 확장자 필터
  iv-fstree      폴더 트리 모델 (루트·지연 나열·펼침 상태, 숨김 조건은 술어로 받음) — std 만
  iv-scan        백그라운드 사진 폴더 스캔 (낮은 QoS 스레드, Gate 양보) → 폴더별 Verdict(HasPhotos|Empty|Unknown)
  iv-fsops       새 폴더·이름 바꾸기·폴더로 옮기기 (덮어쓰지 않음) — std 만
  iv-trash       OS 휴지통 이동 (macOS NSFileManager) — 영구 삭제로 대체하지 않음
  iv-rotate      돌린 방향을 원본에 저장 (ImageIO CopyImageSource — 방향 태그만, 픽셀 재압축 없음)
  iv-viewport    확대·이동·화면 맞춤 수학 (순수 함수, 단위 시험)
  iv-render      Renderer trait (백엔드 무관)
  iv-render-wgpu GPU 렌더러 (텍스처·밉맵·변환 셰이더·egui 오버레이)
  iv-render-cpu  CPU 폴백 렌더러 (softbuffer + CPU 리샘플링 + egui 소프트웨어 래스터라이저)
  iv-viewer      winit 무관 뷰어 상태(Mode: Browse|View) + Action 처리기 (헤드리스 시험)
  iv-ui          egui 화면: 전체 보기 오버레이 + 브라우저(메뉴·도구모음·트리·격자·미리보기·상태줄)
app/
  iv             바이너리: winit 이벤트 루프 + 배선 + 단축키 → Action 디스패치, --render-to 헤드리스 출력
```

### 의존 방향 (단방향, 순환 금지)
```
iv-core  ←  iv-decode-*      (디코더는 core만 안다)
iv-core  ←  iv-pipeline      (파이프라인은 Decoder trait만 안다 — 구체 디코더 모름)
iv-core  ←  iv-library
iv-core, iv-imageops ← iv-thumbs   (썸네일은 디코더 trait 만 안다)
iv-fstree (독립, std 만)
iv-core ← iv-scan          (트리·뷰어를 모른다 — 앱이 판정을 트리 숨김 술어로 잇는다)
iv-trash (독립)
iv-core ← iv-rotate
iv-viewport (독립)
iv-core, iv-viewport ← iv-render
iv-viewer → iv-pipeline, iv-library, iv-viewport   (썸네일·트리는 모른다)
iv-ui → egui, iv-viewer, iv-library, iv-fstree, iv-core   (그리기만, Action 반환)
app/iv  →  전부 (유일하게 모두를 아는 조립 지점)
```
→ 새 디코더 = 새 crate + `app/iv`의 등록 한 줄. 파이프라인·렌더러는 손대지 않는다.

## 핵심 인터페이스 (iv-core) — advisor 자문 반영(2026-11-18)

```rust
pub trait Decoder: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> Caps;                       // PREVIEW | SCALED | REGION | ANIM | HDR
    fn probe(&self, src: &ImageSource, head: &[u8]) -> Option<Priority>;
    fn info(&self, src: &ImageSource) -> Result<ImageInfo>;   // 디코드 전 크기·방향·프레임 수·색공간
    fn preview(&self, src, hint, cancel) -> Result<Option<Frame>> { Ok(None) }        // 선택
    fn decode(&self, src, hint: SizeHint, cancel: &CancelToken) -> Result<Frame>;
    fn decode_region(&self, src, rect, scale, cancel) -> Result<Option<Frame>> { Ok(None) } // 선택(타일링)
    fn open_animation(&self, src) -> Result<Option<Box<dyn FrameStream>>> { Ok(None) }   // 선택(GIF/APNG/WebP)
}
Frame { width, height, stride, format: PixelFormat{Rgba8Srgb | Rgba16 | Rgba16F},
        color: ColorInfo{Srgb | DisplayP3 | Icc(Arc<[u8]>)}, orientation: Orientation,
        full_size, is_preview, pixels: Arc<[u8]> }
ImageSource = Path(PathBuf)        // 이후 Archive(cbz)/Bytes 변형 추가
```
- **확장 기능은 기본 구현이 있는 선택 메서드 + `capabilities()`** — 새 능력을 추가해도 기존 디코더는 수정 불필요.
- **방향(EXIF orientation)은 디코더가 회전하지 않고 `Orientation`만 돌려준다** → 렌더러가 UV 변환으로 처리(대형 이미지 CPU 회전 비용 0, 타일 좌표계 일관).
- 처음엔 `Rgba8Srgb`/`Srgb`만 구현. 포맷·색 필드는 자리만 미리 둔다.
- 오래 걸리는 디코드(RAW·대형 TIFF)는 `CancelToken`으로 중도 취소.
- JPEG 축소 디코딩은 image 크레이트로 불가 → **zune-jpeg/turbojpeg + fast_image_resize 를 초기 단계로 당김**.
- ImageIO 워커는 작업마다 `autoreleasepool`.

## 파이프라인 (iv-pipeline)
- API: `set_window(entries, center_idx, generation, screen_hint)` — 앱은 "지금 어디를 보고 있는지"만 알리고, 우선순위 계산은 파이프라인 내부.
- 창(window) 밖 작업만 취소, 창 안 작업은 우선순위만 재계산 (연속 넘김 시 진행 중 다음 장 디코드를 버리지 않음).
- 키별 in-flight 맵으로 중복 디코드 방지.
- **현재 장 전용 워커 레인** 1개 + 미리 읽기 워커 min(4, 코어-1).
- 캐시: 바이트 상한 LRU, 키 = (path, mtime, 해상도 등급). 조회 = 요청 등급 이상 중 가장 작은 것, 없으면 가장 큰 것(점진 교체).
- 썸네일은 별도 crate(iv-thumbs)·별도 캐시. 두 작업 풀은 `iv_core::Gate` 로 중재 (현재 장 디코드 우선).
- 결과 전달: 내부 큐에 쌓고 콜백 `wake()` 한 번 → 앱이 메인 스레드에서 `drain()` (이벤트 폭주 방지). 파이프라인은 winit을 모른다.

## 뷰어 상태 (iv-viewer, 신설)
- `ViewerState`(현재 목록·위치·Viewport·표시 중 프레임 메타) + `Action` 처리기 + (이후) 애니메이션 시계. winit 의존 없음 → 헤드리스 시험 가능.

## 렌더링 (iv-render)
- Frame → wgpu 텍스처(포맷은 Frame에서) + **밉맵 blit 패스 직접 구현**(wgpu는 자동 생성 없음). 표면은 sRGB.
- 방향 변환·확대·이동은 uniform — 재업로드 없음.
- 기기 `max_texture_dimension_2d` 초과 시 축소 hint (타일링은 이후 iv-render 내부만 변경).
- GPU 텍스처 캐시는 VRAM 상한을 iv-render가 따로 관리.
- egui 오버레이는 같은 프레임 위에. `ControlFlow::Wait`.

## 렌더러 폴백 (요구사항 추가 2026-11-18, 전하 승인)
- `iv-render`는 **`Renderer` trait**(백엔드 무관): `resize`, `set_frame(key, &Frame)`, `render(&RenderParams{viewport, orientation, overlay})`, `info() -> "Metal"/"WARP"/"CPU"`.
  - `iv-render-wgpu`: 기본 GPU 경로. 어댑터 실패 시 `force_fallback_adapter: true` 재시도(WARP/llvmpipe).
  - `iv-render-cpu`: 최종 비상구 — `softbuffer` 표시 + CPU 리샘플링(`fast_image_resize`; 확대·이동 시 보이는 영역만 리샘플).
- 선택 순서: `--renderer=auto|gpu|fallback|cpu` (기본 auto: GPU → fallback adapter → CPU). 앱이 죽거나 빈 화면 금지.
- 렌더러 이름은 로그 + 상태 표시줄에 표시.
- 오버레이(UI)는 `iv-ui`가 백엔드 무관 "오버레이 모델"(텍스트·사각형 목록)을 만들고, wgpu 쪽은 egui로, CPU 쪽은 간이 래스터라이저로 그린다. → 실제 구현: CPU 렌더러가 egui 메시를 직접 래스터화 (iv-render-cpu/src/egui_raster.rs).
- 디코딩·파이프라인은 렌더러와 무관 → 폴백에서도 열기·넘기기 속도 동일.

## 교차 검증 반영 (GPT 계열, 2026-11-18)
- **오류 분류**: `DecodeError{Unsupported, InvalidData, Io, Cancelled, ResourceLimit}`. 다음 디코더 폴백은 `Unsupported`·`InvalidData`일 때만(예: image가 못 여는 CMYK JPEG를 ImageIO가 열 수 있음 — GPT는 Unsupported만 주장, 최종판단: InvalidData도 허용), `Io`·`Cancelled`는 즉시 실패.
- **표시 일치 규칙**: 결과마다 `(EntryId, generation, quality)`. 화면엔 현재 항목과 일치하고 현재 표시보다 품질이 높을 때만 올린다.
- **자원 예산**: `info()`의 크기로 예상 바이트 계산 → 전역 in-flight 바이트 예산 안에서만 미리 읽기 시작. 현재 장은 예산 무시(항상 시작).
- **lost wakeup 방지**: `pending: AtomicBool`. 워커: push → `swap(true)`가 false였으면 wake. 메인: `store(false)` 후 drain (순서 고정).
- **Renderer 계약**: `render() -> RenderOutcome{Ok, Reconfigure, DeviceLost}`; DeviceLost 시 다음 단계 폴백. `ui_capability() -> Full | StatusOnly`(CPU 도 2026-11-19 부터 egui 래스터라이저로 Full).
- **ControlFlow**: 기본 `Wait`, egui repaint deadline·애니메이션·관성은 `WaitUntil`.
- **iv-library 확장자**는 후보 축소 힌트, 최종 판정은 registry probe.
- **썸네일 캐시는 별도 타입·키**(CacheTier에 섞지 않음).
- `--render-to`: GPU offscreen 실패 시 CPU 렌더러로 PNG.

## 브라우저 모드 (2026-11-19, 설계: `.plan/browser-mode.md`)
- 화면: 메뉴·도구모음·경로 줄 / 왼쪽 트리 + 미리보기 칸 / 썸네일 격자(파일명·해상도) / 상태줄(폴더·파일 수·용량·선택 정보).
- 더블클릭·Enter → 전체 보기, Esc → 브라우저(마지막 본 장이 보이게). 파일 인자 실행은 전체 보기로 시작.
- 미리보기 칸은 egui 가 투명으로 두고 렌더러가 `Scene.image_area` 로 그 칸에 밉맵 이미지를 그린다 → Enter 시 재업로드 0.
- 썸네일 요청: 매 프레임 "보이는 ±2줄 키 목록"이 직전과 다를 때만. 결과는 방향이 구워진 RGBA → egui 텍스처(96MB 상한, 프레임당 48장 삽입).
- CPU 렌더러도 egui 래스터라이저로 같은 화면 (UiCapability::Full). 래스터라이저가 없는 렌더러면 브라우저 대신 전체 보기만.

## 이후 경계 (차후)
- HDR: 표면 Rgba16Float + EDR, iv-render 내부. ICC: `iv-color`(lcms2/qcms) 디코드 후처리 단계.

## 기능 확장 방식
- **Action enum + Keymap**: 모든 사용자 동작은 `Action`(Next, Prev, ZoomIn, Fit, …)으로 정의, 단축키·메뉴·UI 버튼이 같은 Action을 낸다. 새 기능 = Action 추가 + 처리기.
- 디코더 = 새 crate, 등록 한 줄. Windows WIC도 같은 방식.
- 썸네일 = iv-thumbs (디스크 캐시 `~/Library/Caches/iv/thumbs-v1-320`, `IV_CACHE_DIR` 로 바꿈).

## 검증
- `--render-to out.png <파일>`: 오프스크린 렌더 후 PNG 저장 (방향·색 확인용).
- 디코드·업로드·첫 프레임 ms 로그 (`IV_TIMING=1`).
- dev 프로필: 의존 crate는 opt-level=3.

## 개발 순서
1) 창+이미지 1장 → 2) 폴더·넘기기·미리 읽기 → 3) 확대·이동·맞춤 (← 첫 보고 단위)
4) 썸네일 브라우저·디스크 캐시 ✅ → 5) SVG/JXL/RAW 전용 디코더·WIC → 6) 서명·공증·MSI
