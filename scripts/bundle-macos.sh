#!/usr/bin/env bash
# Builds "Meeting Transcriber.app" in target/release/bundle.
# macOS asks for microphone and screen/system-audio permission per app, so a
# real .app bundle (with a stable identifier) is needed for the prompts and
# for the permissions to be remembered. Dictation also needs Input Monitoring
# (to see the hotkey) and Accessibility (to press ⌘V); those two have no
# usage string, and are switched on in System Settings.
# The /usr/lib/swift rpath the app needs is added by build.rs.
#
#   ./scripts/bundle-macos.sh            build the app
#   ./scripts/bundle-macos.sh --install  also copy it to /Applications
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ "$(uname)" != "Darwin" ]]; then
  echo "This script must run on macOS." >&2
  exit 1
fi

NAME="Meeting Transcriber"
BUNDLE_ID="com.patricksun.meeting-transcriber"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
APP="target/release/bundle/$NAME.app"

cargo build --release

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/meeting-transcriber "$APP/Contents/MacOS/meeting-transcriber"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$NAME</string>
  <key>CFBundleDisplayName</key><string>$NAME</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleExecutable</key><string>meeting-transcriber</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>Your microphone is transcribed as "Me" in meetings, and while you hold the dictation hotkey.</string>
  <key>NSAudioCaptureUsageDescription</key>
  <string>Meeting audio from other apps is transcribed as "Others".</string>
</dict>
</plist>
PLIST

# Ad-hoc signature: enough for macOS to attach permissions to this app.
codesign --force --deep --sign - "$APP"

echo "Built: $APP"

if [[ "${1:-}" == "--install" ]]; then
  rm -rf "/Applications/$NAME.app"
  cp -R "$APP" /Applications/
  echo "Installed: /Applications/$NAME.app"
fi
