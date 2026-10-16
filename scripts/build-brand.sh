#!/bin/bash
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_ROOT"
BRAND_OUTPUT="$PROJECT_ROOT/target/brand"
mkdir -p "$BRAND_OUTPUT" macos/Sources/AuthManager/Resources
BRAND_SDK="$(xcrun --sdk macosx26.5 --show-sdk-path)"
xcrun swiftc -sdk "$BRAND_SDK" scripts/brand-assets.swift -o "$BRAND_OUTPUT/render"
"$BRAND_OUTPUT/render" pack assets/brand/logo-master.png "$BRAND_OUTPUT/generated"
iconutil -c icns "$BRAND_OUTPUT/generated/AppIcon.iconset" -o assets/brand/AppIcon.icns
cp "$BRAND_OUTPUT/generated/BrandMark.png" macos/Sources/AuthManager/Resources/BrandMark.png
cp "$BRAND_OUTPUT/generated/MenuMark.png" macos/Sources/AuthManager/Resources/MenuMark.png
"$BRAND_OUTPUT/render" preview assets/brand/logo-master.png assets/brand/preview.png
printf 'Brand resources exported.\n'
