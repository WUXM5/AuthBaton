#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$ROOT/dist/quota-ui-tests"
mkdir -p "$OUT"
SDK="$(xcrun --sdk macosx26.5 --show-sdk-path)"
swiftc -sdk "$SDK" -swift-version 6 -parse-as-library \
  "$ROOT/macos/Sources/AuthManager/Models.swift" \
  "$ROOT/macos/Sources/AuthManager/ServiceController.swift" \
  "$ROOT/scripts/quota-ui-tests/QuotaControllerTests.swift" \
  -o "$OUT/quota-controller-tests"
"$OUT/quota-controller-tests" "$ROOT/scripts/accounts-preview/fixture.json" | tee "$OUT/results.json"
