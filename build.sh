#!/bin/bash
# Собирает build/Sugrob.app. Xcode не нужен, хватает Command Line Tools.
# UNIVERSAL=1 ./build.sh собирает под Apple silicon и Intel.
set -euo pipefail
cd "$(dirname "$0")"

VERSION=$(cat VERSION)
REPO=$(cat REPO 2>/dev/null || true)

APP="build/Sugrob.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"

if [ "${UNIVERSAL:-0}" = "1" ]; then
    swift build -c release --product Sugrob --arch arm64 --scratch-path .build-arm64
    swift build -c release --product Sugrob --arch x86_64 --scratch-path .build-x86_64
    lipo -create -output "$APP/Contents/MacOS/Sugrob" \
        .build-arm64/release/Sugrob .build-x86_64/release/Sugrob
else
    swift build -c release --product Sugrob
    cp .build/release/Sugrob "$APP/Contents/MacOS/Sugrob"
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>tech.borsschec.sugrob</string>
    <key>CFBundleName</key><string>Sugrob</string>
    <key>CFBundleExecutable</key><string>Sugrob</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>LSUIElement</key><true/>
    <key>NSHighResolutionCapable</key><true/>
    <key>SugrobRepo</key><string>$REPO</string>
</dict>
</plist>
PLIST

# С постоянной подписью разрешения переживают обновления (см. make-dev-cert.sh).
if security find-certificate -c "Sugrob Dev" >/dev/null 2>&1; then
    codesign -f -s "Sugrob Dev" "$APP"
else
    codesign -f -s - "$APP"
fi
echo "Готово: $PWD/$APP"
