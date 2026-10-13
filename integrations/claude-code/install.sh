#!/bin/bash
# Copies the companion mod to AuthManager's data folder and prints that folder. Claude Code loads
# it in every session once ~/.claude/settings.json has "env": {"CLAUDE_CODE_PLUGIN_DIRS": <folder>}.
# Not inside the app bundle: Claude Code writes type files into every mod folder it loads.
set -euo pipefail
SRC="$(cd "$(dirname "$0")/authmanager" && pwd)"
DEST="$HOME/Library/Application Support/AuthManager/claude-code-plugin/authmanager"
mkdir -p "$DEST/.claude-plugin"
ditto "$SRC/.claude-plugin/plugin.json" "$DEST/.claude-plugin/plugin.json"
ditto "$SRC/hooks" "$DEST/hooks"
printf '%s\n' "$DEST"
