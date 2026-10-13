#!/bin/bash
# Usage: render.sh [output-dir] | render.sh interact | render.sh dump [scenario] | render.sh live <directory.json> <snapshot.json> <out-dir>
# PREVIEW_ONLY=1,4 limits interact to those sections; DRIVER_DEBUG=1 prints where each synthetic click lands.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="$ROOT/dist/accounts-preview/accounts-preview"
mkdir -p "$(dirname "$BIN")"
SDK="$(xcrun --sdk macosx26.5 --show-sdk-path)"
SRC="$ROOT/macos/Sources/AuthManager"
swiftc -sdk "$SDK" -parse-as-library \
  "$SRC/Models.swift" "$SRC/Theme.swift" "$SRC/Presentation.swift" "$SRC/DashboardView.swift" \
  "$SRC/AccountsView.swift" "$SRC/AccountInspector.swift" "$SRC/ConfirmSheets.swift" \
  "$SRC/CodexStrategyView.swift" "$SRC/DesktopConnectionView.swift" \
  "$SRC/MenuBarPanel.swift" "$SRC/LoginView.swift" "$SRC/CloudView.swift" "$SRC/ResetHistoryView.swift" \
  "$ROOT/scripts/accounts-preview/Preview.swift" -o "$BIN"
case "${1:-}" in
  interact|dump|live) "$BIN" "$@" ;;
  *) "$BIN" "${1:-$ROOT/dist/accounts-preview}" ;;
esac
