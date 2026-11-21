#!/usr/bin/env bash
# KaiIV.app 만들기 → dist/app.noindex/KaiIV.app (선택: dist/KaiIV-<버전>.dmg)
# .noindex 폴더 = Spotlight 이 색인하지 않음 (설치한 /Applications/KaiIV.app 과 둘로 보이지 않게)
#
#   tools/make-app.sh              애플 실리콘용 .app
#   tools/make-app.sh --universal  애플 실리콘 + 인텔 겸용 (.app 하나)
#   tools/make-app.sh --dmg        .app 을 담은 .dmg 도 만든다 (배포용)
#   tools/make-app.sh --update     인앱 업데이트용 KaiIV-<판>.zip + 서명된 update.json (키체인 서명 키 필요)
#
# 배포 한 번: tools/make-app.sh --universal --dmg --update → 세 파일을 한 번의 gh release create 로 올린다
#   (latest 가 update.json 없는 릴리스를 가리키는 틈이 없게)
#
# 서명: 기본은 ad-hoc(-). 개발자 ID 가 있으면 SIGN_ID="Developer ID Application: ..." 로 넘긴다.
# ad-hoc 서명 앱은 다른 맥에서 처음 열 때 Gatekeeper 가 막는다 (README 의 '처음 열기' 참고).
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

UNIVERSAL=0; DMG=0; UPDATE=0
for a in "$@"; do
  case "$a" in
    --universal) UNIVERSAL=1 ;;
    --dmg) DMG=1 ;;
    --update) UPDATE=1 ;;
    *) echo "알 수 없는 인자: $a" >&2; exit 2 ;;
  esac
done

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
APP=dist/app.noindex/KaiIV.app
SIGN_ID=${SIGN_ID:--}
echo "KaiIV $VERSION → $APP"

#region 빌드
if [ "$UNIVERSAL" = 1 ]; then
  rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
  cargo build --release -p iv --target aarch64-apple-darwin
  cargo build --release -p iv --target x86_64-apple-darwin
  BIN=target/universal/kaiiv
  mkdir -p target/universal
  lipo -create -output "$BIN" target/aarch64-apple-darwin/release/kaiiv target/x86_64-apple-darwin/release/kaiiv
else
  cargo build --release -p iv
  BIN=target/release/kaiiv
fi
#endregion

#region 묶음 조립
rm -rf "$APP"; mkdir -p "$(dirname "$APP")"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/kaiiv"

# 아이콘: assets/icon-1024.png → .icns (없으면 tools/make-app-icon.py 로 그린다)
[ -f assets/icon-1024.png ] || python3 tools/make-app-icon.py
ICONSET=$(mktemp -d)/KaiIV.iconset
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s assets/icon-1024.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s*2)) $((s*2)) assets/icon-1024.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/KaiIV.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>KaiIV</string>
  <key>CFBundleDisplayName</key><string>KaiIV</string>
  <key>CFBundleIdentifier</key><string>com.kaifacun.kaiiv</string>
  <key>CFBundleExecutable</key><string>kaiiv</string>
  <key>CFBundleIconFile</key><string>KaiIV</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleDevelopmentRegion</key><string>ko</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.photography</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSHumanReadableCopyright</key><string>KaiIV</string>
</dict>
</plist>
PLIST
#endregion

#region 서명 (ad-hoc 이면 이 맥에서 바로 열림, 배포는 개발자 ID + 공증 필요)
codesign --force --options runtime --timestamp=none --sign "$SIGN_ID" "$APP"
codesign --verify --strict "$APP"
echo "서명: $SIGN_ID"
#endregion

if [ "$DMG" = 1 ]; then
  DMGFILE="dist/KaiIV-$VERSION.dmg"
  STAGE=$(mktemp -d)
  cp -R "$APP" "$STAGE/"
  ln -s /Applications "$STAGE/Applications"
  rm -f "$DMGFILE"
  hdiutil create -volname "KaiIV $VERSION" -srcfolder "$STAGE" -ov -format UDZO "$DMGFILE" >/dev/null
  [ "$SIGN_ID" = "-" ] || codesign --sign "$SIGN_ID" "$DMGFILE"
  echo "DMG: $DMGFILE"
fi
if [ "$UPDATE" = 1 ]; then
  # 업데이트는 애플 실리콘·인텔 모두에게 가므로 겸용만 (앱도 설치 전에 아키텍처를 검사한다)
  [ "$UNIVERSAL" = 1 ] || { echo "--update 는 --universal 과 함께만" >&2; exit 2; }
  ZIP="dist/KaiIV-$VERSION.zip"
  rm -f "$ZIP" dist/update.json
  # ditto --keepParent: 서명·심볼릭 링크·확장 속성을 그대로 (zip 명령은 서명을 깬다)
  ditto -c -k --keepParent "$APP" "$ZIP"
  # 알림 글 = CHANGELOG 의 이 판 절 (머리 줄 다음부터 다음 ## 전까지)
  NOTES=$(mktemp)
  awk -v v="## $VERSION " 'index($0, v) == 1 {on=1; next} on && /^## / {exit} on {print}' CHANGELOG.md | sed '/^[[:space:]]*$/d' > "$NOTES"
  [ -s "$NOTES" ] || { echo "CHANGELOG 에 $VERSION 절이 없습니다" >&2; exit 1; }
  URL="https://github.com/kaifacun/image-viewer/releases/download/v$VERSION/KaiIV-$VERSION.zip"
  cargo run -q --release -p iv-update --bin kaiiv-update-sign -- sign "$ZIP" "$VERSION" "$URL" "$NOTES" dist/update.json
  rm -f "$NOTES"
  echo "업데이트: $ZIP + dist/update.json"
fi
echo "완료: $APP ($(du -sh "$APP" | cut -f1))"
