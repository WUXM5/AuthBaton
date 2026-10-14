#!/bin/bash
# Usage: render.sh [output-dir] | render.sh interact | render.sh dump [scenario] [page] [selection]
#        | render.sh live <directory.json> <snapshot.json> <out-dir> | render.sh imagerender <out.png> | render.sh classes <page> [scenario]
# PREVIEW_ONLY=1,4 limits interact to those sections; DRIVER_DEBUG=1 prints where each synthetic click lands.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="$ROOT/dist/accounts-preview/accounts-preview"
mkdir -p "$(dirname "$BIN")"
SDK="$(xcrun --sdk macosx26.5 --show-sdk-path)"
SRC="$ROOT/macos/Sources/AuthManager"
swiftc -sdk "$SDK" -parse-as-library \
  "$SRC/Models.swift" "$SRC/Theme.swift" "$SRC/LoopKit.swift" "$SRC/Presentation.swift" "$SRC/DashboardView.swift" \
  "$SRC/AccountsView.swift" "$SRC/AccountParts.swift" "$SRC/CodexPage.swift" "$SRC/ClaudePage.swift" "$SRC/ConfirmSheets.swift" \
  "$SRC/CodexStrategyView.swift" "$SRC/DesktopConnectionView.swift" \
  "$SRC/MenuBarPanel.swift" "$SRC/LoginView.swift" "$SRC/CloudView.swift" "$SRC/ResetHistoryView.swift" \
  "$ROOT/scripts/accounts-preview/Preview.swift" -o "$BIN"
case "${1:-}" in
  interact|dump|live|imagerender|classes) "$BIN" "$@" ;;
  *) "$BIN" "${1:-$ROOT/dist/accounts-preview}" ;;
esac
