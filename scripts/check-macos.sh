#!/bin/sh
# Runs the packaged app on the build machine with its own data folder, and keeps what it looked like and
# what it said. Published with the release, because nobody sits at this machine to look. Fails the build
# only if the app doesn't stay running.
set -u

EXE="dist/Claude Buddy.app/Contents/MacOS/claude-buddy-rs"
HOME_DIR="$(mktemp -d)"
export CLAUDE_BUDDY_RS_HOME="$HOME_DIR/app"
export CLAUDE_CONFIG_DIR="$HOME_DIR/claude"
mkdir -p "$CLAUDE_BUDDY_RS_HOME" "$CLAUDE_CONFIG_DIR"
echo '{"port":47398,"muted":true,"toasts":false,"askedHooks":true,"language":"vi"}' > "$CLAUDE_BUDDY_RS_HOME/config.json"
LOG=dist/mac-check-log.txt
: > "$LOG"

say() { echo "$1" | tee -a "$LOG"; }

"$EXE" --install-hooks; say "install hooks exit: $?"
grep -q claude-buddy-rs "$CLAUDE_CONFIG_DIR/settings.json" && say "hooks written: yes" || say "hooks written: NO"

# The buddy alone.
"$EXE" >>"$LOG" 2>&1 &
PID=$!
sleep 12
if kill -0 "$PID" 2>/dev/null; then say "buddy running after 12s: yes"; ALIVE=1; else say "buddy running after 12s: NO"; ALIVE=0; fi
screencapture -x dist/mac-check-buddy.png; say "screenshot buddy exit: $?"

# A session starting and needing attention, the way a hook reports it.
send() { printf '%s' "$1" | "$EXE" hook 47398; say "hook exit: $?"; }
send '{"session_id":"check","hook_event_name":"SessionStart","cwd":"/tmp/shop-api"}'
send '{"session_id":"check","hook_event_name":"UserPromptSubmit","cwd":"/tmp/shop-api","prompt":"hello"}'
sleep 3
screencapture -x dist/mac-check-working.png
send '{"session_id":"check","hook_event_name":"Notification","cwd":"/tmp/shop-api","message":"Cần quyền chạy lệnh","notification_type":"permission_prompt"}'
sleep 3
screencapture -x dist/mac-check-waiting.png
kill "$PID" 2>/dev/null
sleep 1

# The main window, each page.
for PAGE in 0 1 2; do
    CLAUDE_BUDDY_RS_OPEN_PAGE=$PAGE "$EXE" >>"$LOG" 2>&1 &
    PID=$!
    sleep 10
    if kill -0 "$PID" 2>/dev/null; then say "page $PAGE running: yes"; else say "page $PAGE running: NO"; ALIVE=0; fi
    screencapture -x "dist/mac-check-page$PAGE.png"
    kill "$PID" 2>/dev/null
    sleep 1
done

"$EXE" --remove-hooks; say "remove hooks exit: $?"
grep -q claude-buddy-rs "$CLAUDE_CONFIG_DIR/settings.json" && say "hooks removed: NO" || say "hooks removed: yes"

# Crash reports, if any: what the app itself could not say.
for f in "$HOME"/Library/Logs/DiagnosticReports/claude-buddy-rs*; do
    [ -e "$f" ] || continue
    say "--- crash report: $f"
    head -80 "$f" >> "$LOG"
done

rm -rf "$HOME_DIR"
echo "--- log"; cat "$LOG"
[ "$ALIVE" = 1 ]
