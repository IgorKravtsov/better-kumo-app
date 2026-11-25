#!/bin/bash
# Builds an unsigned Better Kumo .ipa for SideStore / AltStore, which re-sign it with the user's own free Apple ID.
# The binaries carry an ad-hoc signature only to hold the keychain entitlement; SideStore swaps the Team ID prefix.
#
#   scripts/build-ios-ipa.sh [out-dir]     (default: dist/)
#   KUMO_VERSION=1.2.3   version for the app and the file name (default: MARKETING_VERSION in project.yml)
#   SKIP_CORE=1          reuse the Rust core already in apps/ios/KumoCore
set -euo pipefail
unset CDPATH

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IOS="$ROOT/apps/ios"
BUNDLE_ID="${KUMO_BUNDLE_ID:-com.kravtsov.kumowatch}"
mkdir -p "${1:-$ROOT/dist}"
OUT="$(cd "${1:-$ROOT/dist}" && pwd)"
SKIP_CORE="${SKIP_CORE:-0}"
VERSION="${KUMO_VERSION:-}"

# Inside $OUT, so the finished .ipa is renamed, not copied, into place.
TMP="$(mktemp -d "$OUT/.Better-Kumo-iOS.XXXXXX")"
PARTIAL="$TMP/package.ipa"
trap 'rm -rf "$TMP"' EXIT

[ "$SKIP_CORE" = 1 ] || PROFILE=release "$ROOT/scripts/build-core-ios.sh"
cd "$IOS"
xcodegen generate --quiet

VERSION_ARGS=()
[ -z "$VERSION" ] || VERSION_ARGS=(MARKETING_VERSION="$VERSION")
BUILD="$IOS/build/ipa"
rm -rf "$BUILD/Build/Products/Release-iphoneos"
xcodebuild -project Kumowatch.xcodeproj -scheme Kumowatch -configuration Release \
  -sdk iphoneos -destination generic/platform=iOS -derivedDataPath "$BUILD" \
  CODE_SIGNING_ALLOWED=NO KUMO_BUNDLE_ID="$BUNDLE_ID" KUMO_CAPS="" ${VERSION_ARGS[@]+"${VERSION_ARGS[@]}"} \
  CURRENT_PROJECT_VERSION="$(git -C "$ROOT" rev-list --count HEAD)" build -quiet
APP="$BUILD/Build/Products/Release-iphoneos/Kumowatch.app"
WIDGET="$APP/PlugIns/KumowatchWidget.appex"

ENT="$TMP/entitlements.plist"
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
if [ -d "$APP/Frameworks" ]; then
  for framework in "$APP/Frameworks"/*.framework; do
    [ -e "$framework" ] && codesign -f -s - "$framework"
  done
fi
codesign -f -s - --entitlements "$ENT" "$WIDGET"
codesign -f -s - --entitlements "$ENT" "$APP"
codesign --verify --deep --strict "$APP"

VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Info.plist")"
mkdir "$TMP/Payload"
cp -R "$APP" "$TMP/Payload/"
IPA="$OUT/Better-Kumo-iOS-$VERSION.ipa"
(cd "$TMP" && zip -qry "$PARTIAL" Payload)
mv -f "$PARTIAL" "$IPA"
echo "$IPA"
