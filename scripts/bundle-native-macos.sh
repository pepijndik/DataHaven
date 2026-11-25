#!/bin/sh
# Builds the native window and wraps it in a macOS app bundle:
#   target/release/bundle/DataHaven.app
# Options (environment variables):
#   SKIP_BUILD=1  use the binary already at target/release/backup-station (CI builds a universal one)
#   DMG=1         also make target/release/bundle/DataHaven.dmg
set -eu
cd "$(dirname "$0")/.."

if [ "${SKIP_BUILD:-0}" != "1" ]; then
  cargo build --release -p backup-station
fi

bundle_dir="target/release/bundle"
app="$bundle_dir/DataHaven.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/backup-station "$app/Contents/MacOS/backup-station"
cp native/assets/icon.icns "$app/Contents/Resources/icon.icns"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' native/Cargo.toml | head -1)

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>DataHaven</string>
  <key>CFBundleDisplayName</key><string>DataHaven</string>
  <key>CFBundleIdentifier</key><string>app.datahaven.desktop</string>
  <key>CFBundleExecutable</key><string>backup-station</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$app" 2>/dev/null || true
echo "Built $app"

if [ "${DMG:-0}" = "1" ]; then
  staging="$bundle_dir/dmg"
  rm -rf "$staging" "$bundle_dir/DataHaven.dmg"
  mkdir -p "$staging"
  cp -R "$app" "$staging/"
  ln -s /Applications "$staging/Applications"
  hdiutil create -volname DataHaven -srcfolder "$staging" -ov -format UDZO "$bundle_dir/DataHaven.dmg"
  rm -rf "$staging"
  echo "Built $bundle_dir/DataHaven.dmg"
fi
