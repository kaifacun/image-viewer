# KaiIV 인앱 업데이트 설계 (v0.4.24)

## 배경
- macOS 네이티브 Rust 앱(egui/winit), ad-hoc codesign(개발자 ID·공증 없음), GitHub Releases 공개 배포(.dmg).
- 사용자 요구: 새 판이 올라가면 앱 안에서 알리고, 받아서 바꿔 끼우고 다시 실행 — "현업처럼".

## 신뢰 모델 (Sparkle 방식)
- Ed25519 키쌍. 공개키는 바이너리에 상수로(릴리스 빌드). 비밀키는 배포자 맥 키체인(`security` generic password, 서비스 `KaiIV-update-ed25519`)에만. 저장소·터미널에 절대 출력 안 함.
- ad-hoc codesign 은 무결성만 — 신원 인증은 Ed25519 하나뿐. 릴리스 빌드에서는 피드 URL·공개키를 env 로 못 바꾼다 (디버그/시험 빌드만).

## 피드
- 릴리스마다 자산 3개를 **한 번의** `gh release create` 로: `KaiIV-X.dmg`, `KaiIV-X.zip`(ditto -c -k --keepParent), `update.json`.
- 앱은 `https://github.com/kaifacun/image-viewer/releases/latest/download/update.json` 을 읽는다 (API 한도 없음, 리다이렉트).
- update.json: `{ "version", "url", "size", "sha256", "signature"(base64 ed25519), "notes", "min_os" }`
- 서명 대상(정규 문자열): `kaiiv-update-v1\n{version}\n{size}\n{sha256}\n` → 오래된 서명 zip 을 새 판인 척 재생 불가.
- 404·파싱 실패·서명 불일치 = 백그라운드 확인에선 조용히 "업데이트 없음", 수동 확인에선 오류 표시.

## 클라이언트 흐름
1. 확인: 시작 후 몇 초 뒤 백그라운드 스레드, 하루 1회(마지막 확인 시각 저장), 자동 확인 끄기 가능. 상태줄 판 번호 클릭 = 수동 확인("최신입니다" / 오류도 보여 줌).
2. 판 비교: 숫자 의미 비교(0.4.10 > 0.4.9). 서명이 맞고 signed version > 현재 판일 때만 제안. "이 판 건너뛰기" 기억.
3. 창: 새 판 X, 알림 글(평문, 길이 제한) — [지금 업데이트] [나중에] [이 판 건너뛰기].
4. 받기: curl https(`--proto =https`), 크기 상한, 받은 뒤 크기·SHA-256·Ed25519 확인 **후에만** `ditto -x -k` 로 풀기.
5. 푼 묶음 검사: 단일 `KaiIV.app`, CFBundleIdentifier == com.kaifacun.kaiiv, CFBundleShortVersionString == 서명된 판, `codesign --verify --deep --strict` 통과, 실행 파일 존재.
6. 단계(staging): 대상 앱과 **같은 볼륨**의 형제 경로 `.KaiIV-update-<ver>.app` 로 옮김(ditto). 대상 폴더 쓰기 불가·App Translocation(`/AppTranslocation/`)·dmg 안에서 실행·개발 빌드(.app 밖) → 자동 설치 대신 "다운로드 페이지 열기".
7. 적용: 앱이 하던 파일 작업·회전 저장·코덱 설치가 없을 때만. 스캔 색인 저장 후 종료. 종료 직전 분리된 도우미 `kaiiv --apply-update <staged> <target> <pid>` (setsid, 현재 실행 파일 — 이름 바뀌어도 inode 유지) 실행.
8. 도우미: 부모 pid 종료 대기(최대 30초) → rename target→`.KaiIV-old-<ver>.app` → rename staged→target (실패 시 되돌림) → quarantine 제거 → `/usr/bin/open target` → 옛 묶음 삭제. 로그 `<캐시>/update.log`.

## 저장 상태
- `~/Library/Application Support/KaiIV/update-state` (codecpack 의 consent 파일과 같은 폴더): last_check, skip_version, auto_check.

## 시험 격리
- 자가 시험(IV_SELFTEST)·`IV_UPDATE_NO_CHECK` 면 확인·받기·적용 안 함.
- 끝까지 시험은 `scratch/update-test/` 의 앱 사본(판 낮춤) + 로컬 피드(file://, 시험 키) — 디버그 빌드에서만 허용. 실제 /Applications 앱에는 절대 적용하지 않음.

## 알려야 할 것
- 0.4.22·0.4.23 은 업데이터가 없어 0.4.24 는 손으로 한 번 설치.
- ad-hoc 서명이라 업데이트 후 폴더 접근 권한(TCC) 재질문 가능.
- 비밀키 분실 = 이후 자동 업데이트 불가(다운로드 페이지 폴백만) → 백업 필요.
