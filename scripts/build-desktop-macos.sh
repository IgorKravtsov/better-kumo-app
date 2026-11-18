#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST="$ROOT/dist"
APP="$DIST/Better Kumo.app"
ICON_SRC="$ROOT/apps/ios/Kumowatch/Assets.xcassets/AppIcon.appiconset/icon-1024.png"
VERSION="${KUMO_VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/core/Cargo.toml" | head -1)}"
BUILD="$(git -C "$ROOT" rev-list --count HEAD)"

rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
if [ "${KUMO_UNIVERSAL:-0}" = 1 ]; then
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --manifest-path "$ROOT/core/Cargo.toml" -p kumo-desktop -p kumo-mcp --release --target "$target"
  done
  for bin in better-kumo kumo-mcp; do
    lipo -create -output "$APP/Contents/MacOS/$bin" \
      "$ROOT/core/target/aarch64-apple-darwin/release/$bin" "$ROOT/core/target/x86_64-apple-darwin/release/$bin"
    chmod 755 "$APP/Contents/MacOS/$bin"
  done
else
  cargo build --manifest-path "$ROOT/core/Cargo.toml" -p kumo-desktop -p kumo-mcp --release
  install -m 755 "$ROOT/core/target/release/better-kumo" "$APP/Contents/MacOS/better-kumo"
  install -m 755 "$ROOT/core/target/release/kumo-mcp" "$APP/Contents/MacOS/kumo-mcp"
fi

ICONSET="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z $size $size "$ICON_SRC" --out "$ICONSET/icon_${size}x${size}.png" > /dev/null
  sips -z $((size * 2)) $((size * 2)) "$ICON_SRC" --out "$ICONSET/icon_${size}x${size}@2x.png" > /dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Better Kumo</string>
    <key>CFBundleDisplayName</key><string>Better Kumo</string>
    <key>CFBundleIdentifier</key><string>com.kravtsov.kumowatch.desktop</string>
    <key>CFBundleExecutable</key><string>better-kumo</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$BUILD</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>LSUIElement</key><true/>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$APP"
ZIP="$DIST/Better-Kumo-macOS-$VERSION-$BUILD.zip"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"
echo "App: $APP"
echo "Zip: $ZIP"
