#!/usr/bin/env bash
# Build a release binary and wrap it in BioCartoon.app (with icon) under app/dist/.
#   ./bundle_mac.sh            # then: open dist/BioCartoon.app, or drag it to /Applications
#   ./bundle_mac.sh --install  # also quit the running app, replace /Applications/BioCartoon.app, relaunch
set -euo pipefail
cd "$(dirname "$0")"
source "$HOME/.cargo/env" 2>/dev/null || true
cargo build --release --bin biocartoon-app --bin biocartoon

APP=dist/BioCartoon.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
# The executable keeps its own name: a "BioCartoon" next to the command line's "biocartoon" would
# be the same file on a case-insensitive disk.
cp target/release/biocartoon-app "$APP/Contents/MacOS/biocartoon-app"
# The command line / MCP server ships inside the bundle: $APP/Contents/MacOS/biocartoon
cp target/release/biocartoon "$APP/Contents/MacOS/biocartoon"

# icon.icns is generated from icon.svg by `cargo run --bin make_icon`.
cp assets/icon.icns "$APP/Contents/Resources/BioCartoon.icns"
# Shown in the standard About panel (see `resources` in Cargo.toml).
cp assets/Credits.html "$APP/Contents/Resources/Credits.html"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>BioCartoon</string>
  <key>CFBundleDisplayName</key><string>BioCartoon</string>
  <key>CFBundleIdentifier</key><string>com.biocartoon.app</string>
  <key>CFBundleVersion</key><string>0.3.6</string>
  <key>CFBundleShortVersionString</key><string>0.3.6</string>
  <key>NSHumanReadableCopyright</key><string>© 2026 Joseph Rich. Licensed under the GNU AGPL v3.0.</string>
  <key>CFBundleExecutable</key><string>biocartoon-app</string>
  <key>CFBundleIconFile</key><string>BioCartoon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleDocumentTypes</key>
  <array><dict>
    <key>CFBundleTypeName</key><string>BioCartoon document</string>
    <key>CFBundleTypeExtensions</key><array><string>biocartoon</string></array>
    <key>CFBundleTypeRole</key><string>Editor</string>
  </dict></array>
</dict>
</plist>
PLIST
echo "built $APP"

if [[ "${1:-}" == "--install" ]]; then
  # `quit app` returns before the process exits; opening while it is still
  # shutting down fails with LSOpenURLsWithCompletionHandler error -600.
  osascript -e 'quit app "BioCartoon"' 2>/dev/null || true
  for _ in $(seq 50); do
    pgrep -xq biocartoon-app || break
    sleep 0.1
  done
  pkill -x biocartoon-app 2>/dev/null || true
  rm -rf /Applications/BioCartoon.app
  cp -R "$APP" /Applications/
  open /Applications/BioCartoon.app
  echo "installed /Applications/BioCartoon.app"
fi
