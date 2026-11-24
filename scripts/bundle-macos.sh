#!/usr/bin/env bash
# Build dist/Alfred.app: the release binary, an Info.plist that names why Alfred needs the microphone and
# Automation (Notes, Reminders), no Dock icon, and the orb icon. Signed ad hoc unless ALFRED_SIGN names an
# identity ("Developer ID Application: …"), which lets macOS remember permissions across rebuilds.
#
#   scripts/bundle-macos.sh            # build dist/Alfred.app
#   scripts/bundle-macos.sh --install  # also copy it to ~/Applications and open it
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
  <key>NSMicrophoneUsageDescription</key><string>Alfred listens when you click the orb, and transcribes on this Mac.</string>
  <key>NSAppleEventsUsageDescription</key><string>Alfred saves notes in Notes, adds reminders and quits apps when you ask it to.</string>
</dict>
</plist>
PLIST

codesign --force --sign "${ALFRED_SIGN:--}" --timestamp=none "$app" >/dev/null
echo "Built $app ($version)"

if [[ "${1:-}" == "--install" ]]; then
  mkdir -p "$HOME/Applications"
  rm -rf "$HOME/Applications/Alfred.app"
  cp -R "$app" "$HOME/Applications/Alfred.app"
  open "$HOME/Applications/Alfred.app"
  echo "Installed ~/Applications/Alfred.app"
fi
