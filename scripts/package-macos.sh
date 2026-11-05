#!/bin/sh
# Builds dist/ClaudeBuddy-macos.zip from the two release builds: one app for Apple silicon and Intel.
set -eu

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
APP="dist/Claude Buddy.app"

rm -rf dist
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/claude-buddy-rs" \
    target/aarch64-apple-darwin/release/claude-buddy-rs \
    target/x86_64-apple-darwin/release/claude-buddy-rs

# The icon is made from the same file Windows uses. An app without one still works.
ICONSET="dist/app.iconset"
mkdir -p "$ICONSET"
if sips -s format png assets/app.ico --out dist/icon.png >/dev/null 2>&1; then
    for s in 16 32 128 256; do
        sips -z "$s" "$s" dist/icon.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
        d=$((s * 2))
        sips -z "$d" "$d" dist/icon.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
    done
    iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/app.icns" || true
fi
rm -rf "$ICONSET" dist/icon.png

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Claude Buddy</string>
    <key>CFBundleDisplayName</key><string>Claude Buddy</string>
    <key>CFBundleIdentifier</key><string>com.taivt0109.claude-buddy</string>
    <key>CFBundleExecutable</key><string>claude-buddy-rs</string>
    <key>CFBundleIconFile</key><string>app</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>LSUIElement</key><true/>
    <key>NSHighResolutionCapable</key><true/>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key><string>Claude Buddy character</string>
            <key>CFBundleTypeRole</key><string>Viewer</string>
            <key>LSHandlerRank</key><string>Owner</string>
            <key>CFBundleTypeExtensions</key><array><string>buddy</string></array>
        </dict>
    </array>
</dict>
</plist>
EOF

# Not an identity anyone can trust, only what Apple silicon requires of every program to run at all.
codesign --force --deep --sign - "$APP"
codesign --verify --deep "$APP"

(cd dist && ditto -c -k --keepParent "Claude Buddy.app" ClaudeBuddy-macos.zip)
(cd dist && shasum -a 256 ClaudeBuddy-macos.zip > ClaudeBuddy-macos.zip.sha256)
ls -la dist
