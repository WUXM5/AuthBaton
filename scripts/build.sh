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
cp "$SWIFT_BIN/hotspot-helper" "$APP/Contents/MacOS/hotspot-helper"
cp -R "$SWIFT_BIN/AuthManager_DesktopBridge.bundle" "$APP/Contents/Resources/"
cp -R "$SWIFT_BIN/AuthManager_AuthManager.bundle" "$APP/Contents/Resources/"
cp "$PROJECT_ROOT/assets/brand/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
# Licenses travel with the binaries: AuthBaton's own, and cc-same's for services/claude-desktop.
cp "$PROJECT_ROOT/LICENSE.md" "$APP/Contents/Resources/LICENSE.md"
cp "$PROJECT_ROOT/services/claude-desktop/LICENSE-cc-same.txt" "$APP/Contents/Resources/LICENSE-cc-same.txt"
cp "$SWIFT_BIN/AuthManager" "$APP/Contents/MacOS/AuthManager"
# Sparkle (updates) is embedded as shipped, with its own signature, and loaded from Contents/Frameworks.
rm -rf "${APP:?}/Contents/Frameworks/Sparkle.framework"
mkdir -p "$APP/Contents/Frameworks"
ditto "$SWIFT_BIN/Sparkle.framework" "$APP/Contents/Frameworks/Sparkle.framework"
install_name_tool -add_rpath @executable_path/../Frameworks "$APP/Contents/MacOS/AuthManager"
install -m 0644 "$PROJECT_ROOT/macos/.build/checkouts/Sparkle/LICENSE" "$APP/Contents/Resources/LICENSE-Sparkle.txt"
# AUTHMANAGER_UPDATE_FEED (an appcast URL) turns updates on: only release builds set it.
UPDATE_PLIST=""
if [ -n "${AUTHMANAGER_UPDATE_FEED:-}" ]; then
  UPDATE_PLIST="<key>SUFeedURL</key><string>$AUTHMANAGER_UPDATE_FEED</string>
<key>SUEnableAutomaticChecks</key><true/>
<key>SUAutomaticallyUpdate</key><true/>"
fi
# AUTHMANAGER_BUNDLE_ID keeps the bundle ID of an existing install.
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>${AUTHMANAGER_BUNDLE_ID:-io.github.wuxm5.authbaton}</string>
<key>CFBundleName</key><string>AuthBaton</string>
<key>CFBundleDisplayName</key><string>AuthBaton</string>
<key>CFBundleExecutable</key><string>AuthManager</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleIconFile</key><string>AppIcon</string>
<key>CFBundleShortVersionString</key><string>0.31.0</string>
<key>CFBundleVersion</key><string>93</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>CFBundleLocalizations</key><array><string>zh-Hans</string></array>
<key>SUPublicEDKey</key><string>INDQKwhbMdHpgQi88WgZRYbvV74e/CB7+BH0r+Po7Dc=</string>
<key>SUVerifyUpdateBeforeExtraction</key><true/>
$UPDATE_PLIST
</dict></plist>
PLIST
codesign --force --sign - "$APP/Contents/MacOS/auth-service"
codesign --force --sign - "$APP/Contents/MacOS/credential-store"
codesign --force --sign - "$APP/Contents/MacOS/desktop-bridge"
codesign --force --sign - "$APP/Contents/MacOS/hotspot-helper"
codesign --force --sign - "$APP"
printf 'Built: %s\n' "$APP"
