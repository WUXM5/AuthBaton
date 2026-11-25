#!/bin/bash
# Usage: render.sh [output-dir] | render.sh interact | render.sh dump [scenario] [page] [selection]
#        | render.sh live <directory.json> <snapshot.json> <out-dir> | render.sh imagerender <out.png> | render.sh classes <page> [scenario]
#        | render.sh perf [directory.json|-] [rounds] | render.sh same <poll.json> <poll.json>...
# PREVIEW_ONLY=1,4 limits interact to those sections; DRIVER_DEBUG=1 prints where each synthetic click lands.
# perf builds optimized, like the release app, and prints the main-thread time of page switches and polls.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="$ROOT/dist/accounts-preview/accounts-preview"
mkdir -p "$(dirname "$BIN")"
SDK="$(xcrun --sdk macosx26.5 --show-sdk-path)"
SRC="$ROOT/macos/Sources/AuthManager"
OPT=()
if [ "${1:-}" = perf ]; then OPT=(-O); BIN="$BIN-perf"; fi
swiftc -sdk "$SDK" -parse-as-library ${OPT[@]+"${OPT[@]}"} \
  "$SRC/Models.swift" "$SRC/Theme.swift" "$SRC/LoopKit.swift" "$SRC/Presentation.swift" "$SRC/DashboardView.swift" \
  "$SRC/AccountsView.swift" "$SRC/AccountParts.swift" "$SRC/CodexPage.swift" "$SRC/Onboarding.swift" "$SRC/ClaudeHome.swift" "$SRC/ClaudeInUse.swift" "$SRC/ClaudePage.swift" "$SRC/ConfirmSheets.swift" \
  "$SRC/CodexStrategyView.swift" "$SRC/DesktopConnectionView.swift" "$SRC/ClaudeDesktopModels.swift" "$SRC/ClaudeDesktopPage.swift" "$SRC/ChromeSyncView.swift" "$SRC/NetworkModels.swift" "$SRC/NetworkPage.swift" \
  "$SRC/MenuBarPanel.swift" "$SRC/LoginView.swift" "$SRC/CloudView.swift" "$SRC/CloudParts.swift" "$SRC/ResetHistoryView.swift" \
  "$ROOT/scripts/accounts-preview/Preview.swift" -o "$BIN"
case "${1:-}" in
  interact|dump|live|imagerender|classes|perf|same) "$BIN" "$@" ;;
  *) "$BIN" "${1:-$ROOT/dist/accounts-preview}" ;;
esac
