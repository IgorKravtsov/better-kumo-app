#!/bin/bash
# Builds an unsigned Better Kumo .ipa for SideStore / AltStore, which re-sign it with the user's own free Apple ID.
# The binaries carry an ad-hoc signature only to hold the keychain entitlement; SideStore swaps the Team ID prefix.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IOS="$ROOT/apps/ios"
BUNDLE_ID="${KUMO_BUNDLE_ID:-com.kravtsov.kumowatch}"
OUT="${1:-$ROOT/dist}"
SKIP_CORE="${SKIP_CORE:-0}"

[ "$SKIP_CORE" = 1 ] || PROFILE=release "$ROOT/scripts/build-core-ios.sh"
cd "$IOS"
xcodegen generate --quiet

BUILD="$IOS/build/ipa"
rm -rf "$BUILD/Build/Products/Release-iphoneos"
xcodebuild -project Kumowatch.xcodeproj -scheme Kumowatch -configuration Release \
  -sdk iphoneos -destination generic/platform=iOS -derivedDataPath "$BUILD" \
  CODE_SIGNING_ALLOWED=NO KUMO_BUNDLE_ID="$BUNDLE_ID" KUMO_CAPS="" \
  CURRENT_PROJECT_VERSION="$(git -C "$ROOT" rev-list --count HEAD)" build -quiet
APP="$BUILD/Build/Products/Release-iphoneos/Kumowatch.app"

ENT="$(mktemp)"
cat > "$ENT" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>keychain-access-groups</key>
    <array><string>TEAMID.$BUNDLE_ID.shared</string></array>
</dict>
</plist>
EOF
find "$APP/Frameworks" -maxdepth 1 -name '*.framework' -exec codesign -f -s - {} \; 2>/dev/null || true
codesign -f -s - --entitlements "$ENT" "$APP/PlugIns/KumowatchWidget.appex"
codesign -f -s - --entitlements "$ENT" "$APP"
rm -f "$ENT"

VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Info.plist")"
STAGE="$(mktemp -d)"
mkdir "$STAGE/Payload"
cp -R "$APP" "$STAGE/Payload/"
mkdir -p "$OUT"
IPA="$OUT/Better-Kumo-iOS-$VERSION.ipa"
rm -f "$IPA"
(cd "$STAGE" && zip -qry "$IPA" Payload)
rm -rf "$STAGE"
echo "$IPA"
