#!/usr/bin/env bash
# Wraps the release binary in a macOS app bundle: target/release/bundle/macos/WispyDiff.app.
#   scripts/bundle-macos.sh [profile]   (default: release)
set -euo pipefail
cd "$(dirname "$0")/.."
PROFILE=${1:-release}
BIN=target/$PROFILE/wispydiff
APP=target/$PROFILE/bundle/macos/WispyDiff.app
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/wispydiff"
cp assets/icons/icon.icns "$APP/Contents/Resources/icon.icns"
cat > "$APP/Contents/Info.plist" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>WispyDiff</string>
  <key>CFBundleDisplayName</key><string>WispyDiff</string>
  <key>CFBundleIdentifier</key><string>dev.wispydiff.WispyDiff</string>
  <key>CFBundleExecutable</key><string>wispydiff</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
echo "$APP"
