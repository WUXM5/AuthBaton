#!/bin/bash
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_ROOT"
cargo build --locked --offline --release
# CLT's default 27.0 SDK declares SwiftUI macros without shipping their plugin.
# Build against the installed 26.5 SDK until that toolchain pair is complete.
SWIFT_SDK="$(xcrun --sdk macosx26.5 --show-sdk-path)"
swift build --package-path macos --configuration release --sdk "$SWIFT_SDK"
SWIFT_BIN="$(swift build --package-path macos --configuration release --sdk "$SWIFT_SDK" --show-bin-path)"
APP="${AUTHMANAGER_BUILD_APP:-$PROJECT_ROOT/dist/AuthManager.app}"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$PROJECT_ROOT/target/release/auth-service" "$APP/Contents/MacOS/auth-service"
cp "$SWIFT_BIN/credential-store" "$APP/Contents/MacOS/credential-store"
cp "$SWIFT_BIN/desktop-bridge" "$APP/Contents/MacOS/desktop-bridge"
cp -R "$SWIFT_BIN/AuthManager_DesktopBridge.bundle" "$APP/Contents/Resources/"
cp -R "$SWIFT_BIN/AuthManager_AuthManager.bundle" "$APP/Contents/Resources/"
cp "$PROJECT_ROOT/assets/brand/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
cp "$SWIFT_BIN/AuthManager" "$APP/Contents/MacOS/AuthManager"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>io.github.wuxm5.authbaton</string>
<key>CFBundleName</key><string>AuthManager</string>
<key>CFBundleDisplayName</key><string>AuthManager</string>
<key>CFBundleExecutable</key><string>AuthManager</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleIconFile</key><string>AppIcon</string>
<key>CFBundleShortVersionString</key><string>0.17.6</string>
<key>CFBundleVersion</key><string>60</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force --sign - "$APP/Contents/MacOS/auth-service"
codesign --force --sign - "$APP/Contents/MacOS/credential-store"
codesign --force --sign - "$APP/Contents/MacOS/desktop-bridge"
codesign --force --sign - "$APP"
printf 'Built: %s\n' "$APP"
