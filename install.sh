#!/bin/sh
# Installs Claude Buddy on macOS: curl -fsSL https://raw.githubusercontent.com/taivt0109/claude-buddy/main/install.sh | sh
# BUDDY_VERSION=v0.3.3 picks a release other than the latest.
set -eu

REPO="taivt0109/claude-buddy"
ASSET="ClaudeBuddy-macos.zip"
APP="$HOME/Applications/Claude Buddy.app"
EXE="$APP/Contents/MacOS/claude-buddy-rs"

if [ "$(uname -s)" != "Darwin" ]; then
    echo "This installer is for macOS. On Windows use install.ps1." >&2
    exit 1
fi

if [ -n "${BUDDY_VERSION:-}" ]; then
    BASE="https://github.com/$REPO/releases/download/$BUDDY_VERSION"
else
    BASE="https://github.com/$REPO/releases/latest/download"
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "Downloading Claude Buddy..."
curl -fsSL "$BASE/$ASSET" -o "$WORK/$ASSET"
curl -fsSL "$BASE/$ASSET.sha256" -o "$WORK/$ASSET.sha256"

WANT="$(cut -d' ' -f1 "$WORK/$ASSET.sha256")"
GOT="$(shasum -a 256 "$WORK/$ASSET" | cut -d' ' -f1)"
if [ "$WANT" != "$GOT" ]; then
    echo "The download is damaged: its checksum doesn't match. Nothing was installed." >&2
    exit 1
fi

ditto -x -k "$WORK/$ASSET" "$WORK/unpacked"

pkill -x claude-buddy-rs 2>/dev/null || true
sleep 1
mkdir -p "$HOME/Applications"
rm -rf "$APP"
ditto "$WORK/unpacked/Claude Buddy.app" "$APP"

"$EXE" --install-hooks
echo "Connected to Claude Code (settings.json was backed up first). Sessions opened from now on will show up."

# Lets Finder open .buddy files with it.
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$APP" 2>/dev/null || true

open "$APP"
echo "Claude Buddy is installed in $APP"
echo "To remove it: curl -fsSL https://raw.githubusercontent.com/$REPO/main/uninstall.sh | sh"
