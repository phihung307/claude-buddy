#!/bin/sh
# Removes Claude Buddy from macOS, and its hooks from Claude Code. Your characters and settings stay in
# ~/Library/Application Support/claude-buddy-rs unless you delete that folder too.
set -eu

APP="$HOME/Applications/Claude Buddy.app"
EXE="$APP/Contents/MacOS/claude-buddy-rs"

pkill -x claude-buddy-rs 2>/dev/null || true
if [ -x "$EXE" ]; then
    "$EXE" --remove-hooks || true
fi
rm -f "$HOME/Library/LaunchAgents/com.taivt0109.claude-buddy.plist"
rm -rf "$APP" "$HOME/Applications/.claude-buddy-update"
echo "Claude Buddy was removed."
