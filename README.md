# iv — 빠른 이미지 뷰어

Rust + GPU(wgpu) 이미지 뷰어. 맥 우선, 이후 Windows. 설계는 [docs/architecture.md](docs/architecture.md).

## 빌드·실행
```bash
export PATH="$HOME/.cargo/bin:$PATH"     # rustup 설치 위치
cargo run --release -p iv -- <폴더>        # 브라우저 화면 (인자 없으면 홈 폴더)
cargo run --release -p iv -- <파일>        # 그 사진 전체 보기로 시작 (Esc → 그 폴더 브라우저)
cargo run --release -p iv -- <폴더> --renderer=cpu      # CPU 렌더러 강제
```

## 화면
- **브라우저**: 메뉴·도구모음·경로 줄 / 왼쪽 폴더 트리 + 미리보기 / 썸네일 격자(파일명·해상도) / 상태줄(폴더·파일 수·용량·선택 정보)
- **전체 보기**: 썸네일 더블클릭·Enter. Esc 또는 "▦ 목록" 으로 돌아오면 마지막에 본 사진이 격자에 보인다.

## 조작 — 브라우저
| 입력 | 동작 |
|---|---|
| 클릭 / 더블클릭·Enter·Space | 선택 / 전체 보기 |
| ← → ↑ ↓, PageUp/PageDown, Home/End | 격자 이동 |
| Backspace / ⌘R | 상위 폴더 / 새로고침 |
| 트리 클릭, ▶ | 폴더 열기, 펼치기 |
| 경로 줄에 입력 + Enter | 그 폴더로 |
| 도구모음 "크기" | 썸네일 크기 |
| 정렬 메뉴 | 이름·수정한 날짜·크기·형식, 오름/내림 |

## 조작 — 전체 보기
| 입력 | 동작 |
|---|---|
| → ↓ Space PageDown / ← ↑ Backspace PageUp | 다음 / 이전 |
| Home / End | 처음 / 끝 |
| + / - , 마우스 휠, 트랙패드 핀치, ⌘+트랙패드 스크롤 | 확대·축소 (커서 기준) |
| 드래그, 트랙패드 두 손가락 스크롤 | 이동 |
| 0 / 9 / 1 | 화면 맞춤 / 작은 이미지도 맞춤 / 원본 크기 |
| 더블클릭 | 맞춤 ↔ 1:1 |
| Esc | 브라우저로 (전체 화면이면 먼저 해제) |
| F, Enter | 전체 화면 |
| I | 정보 창 |
| 파일 끌어다 놓기 | 열기 |
| ⌘Q / ⌘W | 종료 |

## 썸네일 캐시
`~/Library/Caches/iv/thumbs-v1-320` (Windows `%LOCALAPPDATA%\iv\cache`). 1GB 를 넘으면 시작 때 오래된 것부터 정리. `IV_CACHE_DIR` 로 위치 변경.

## 지원 형식
- macOS ImageIO: JPEG·PNG·GIF·TIFF·HEIC/HEIF·AVIF·WebP·JPEG 2000·PSD·EXR·HDR·카메라 RAW(DNG/CR2/CR3/NEF/ARW/RAF/ORF/RW2…)
- 순수 Rust(image-rs, 전 플랫폼 폴백): JPEG·PNG·GIF·WebP·BMP·TIFF·ICO·TGA·PNM·QOI
- ⚠️ 실제로 열어 시험한 것: JPEG·PNG·HEIC. RAW·AVIF·WebP·PSD·TIFF 는 등록만 되어 있고 실파일 시험 전. SVG·JXL 미지원.

## 렌더러
`--renderer=auto|gpu|fallback|cpu` — 기본 auto: GPU → 폴백 어댑터(WARP/llvmpipe) → CPU(softbuffer). 실행 중 장치 손실도 다음 단계로 전환하도록 되어 있으나 실제 장치 손실은 모의(`lost`)로만 시험. 현재 렌더러는 상태 표시줄에 표시. CPU 렌더러도 같은 화면(브라우저 포함)을 그린다.

## 시험
```bash
cargo test --workspace
python3 tools/make-fixtures.py scratch/fixtures         # 시험 이미지 생성 (git 제외)
cargo run --release -p iv -- scratch/fixtures/orient6.jpg --render-to=out.png --size=400x300 --renderer=cpu
cargo run --release -p iv-viewer --example nav_bench -- scratch/fixtures 300     # 넘기기 체감 시간
cargo run --release -p iv-decode-imageio --example decode_bench -- scratch/fixtures 2560
IV_SELFTEST="wait:1000;cap:/abs/a.png;act:next;wait:100;cap:/abs/b.png;quit" ./target/release/iv scratch/fixtures
# 브라우저 자가 시험 (사용자 폴더·캐시를 건드리지 않게 scratch 로 고정)
IV_CACHE_DIR=$PWD/scratch/cache IV_TREE_ROOTS=$PWD/scratch \
IV_SELFTEST="wait-thumbs:8000;cap:/abs/b1.png;key:down;key:enter;wait:500;cap:/abs/b2.png;key:escape;dblclick:700,250;quit" \
  ./target/release/iv scratch/many
```
