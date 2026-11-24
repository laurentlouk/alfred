#!/usr/bin/env bash
# Build dist/Alfred.app: the release binary, an Info.plist that names why Alfred needs the microphone and
# Automation (Notes, Reminders, Mail, the browser's tab), and the orb icon.
#
# Signing: macOS ties Alfred's permissions (Accessibility, the microphone, Automation) to its signature. An
# ad hoc signature changes with every build, so macOS would take each rebuild for a new app and forget them.
# The script signs with ALFRED_SIGN when set, else with this Mac's first "Apple Development" identity, and
# ad hoc only when there is none.
#
#   scripts/bundle-macos.sh            # build dist/Alfred.app
#   scripts/bundle-macos.sh --install  # also replace ~/Applications/Alfred.app (quitting it first) and open it
set -euo pipefail
cd "$(dirname "$0")/.."

target_dir="${CARGO_TARGET_DIR:-target}"
cargo build --release -p alfred
version="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"

app="dist/Alfred.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$target_dir/release/alfred" "$app/Contents/MacOS/alfred"
if [[ -f assets/Alfred.icns ]]; then
  cp assets/Alfred.icns "$app/Contents/Resources/Alfred.icns"
fi

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Alfred</string>
  <key>CFBundleDisplayName</key><string>Alfred</string>
  <key>CFBundleIdentifier</key><string>com.laurentlouk.alfred</string>
  <key>CFBundleExecutable</key><string>alfred</string>
  <key>CFBundleIconFile</key><string>Alfred</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>Alfred listens when you click the orb or say “Alfred”, and transcribes on this Mac.</string>
  <key>NSAppleEventsUsageDescription</key><string>Alfred saves notes in Notes, adds reminders, reads and drafts mail, reads the address of your browser's tab and quits apps when you ask it to.</string>
</dict>
</plist>
PLIST

sign="${ALFRED_SIGN:-}"
if [[ -z "$sign" ]]; then
  sign="$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Apple Development: [^"]*\)".*/\1/p' | head -1)"
fi
codesign --force --sign "${sign:--}" --timestamp=none "$app" >/dev/null
echo "Built $app ($version), signed ${sign:-ad hoc}"

if [[ "${1:-}" == "--install" ]]; then
  mkdir -p "$HOME/Applications"
  # A running Alfred keeps the old build: quit it first, so the new one is what opens.
  if pgrep -f "Alfred.app/Contents/MacOS/alfred" >/dev/null; then
    osascript -e 'tell application id "com.laurentlouk.alfred" to quit' >/dev/null 2>&1 || true
    for _ in $(seq 1 40); do
      pgrep -f "Alfred.app/Contents/MacOS/alfred" >/dev/null || break
      sleep 0.25
    done
    pkill -f "Alfred.app/Contents/MacOS/alfred" || true
    sleep 0.5
  fi
  rm -rf "$HOME/Applications/Alfred.app"
  cp -R "$app" "$HOME/Applications/Alfred.app"
  open "$HOME/Applications/Alfred.app"
  echo "Installed ~/Applications/Alfred.app"
fi
