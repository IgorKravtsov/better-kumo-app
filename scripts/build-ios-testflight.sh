#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IOS="$ROOT/apps/ios"
CONF="${KUMOWATCH_CONFIG:-$HOME/.config/kumowatch}"
[ -f "$CONF/testflight.env" ] && source "$CONF/testflight.env"
TEAM="${KUMO_TEAM:-}"
BUNDLE_ID="${KUMO_BUNDLE_ID:-com.kravtsov.kumowatch}"
SKIP_CORE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --team) TEAM="$2"; shift ;;
    --bundle-id) BUNDLE_ID="$2"; shift ;;
    --skip-core) SKIP_CORE=1 ;;
    *) echo "usage: $0 --team <TEAM_ID> [--bundle-id <id>] [--skip-core]" >&2; exit 2 ;;
  esac
  shift
done
[ -n "$TEAM" ] || { echo "Set KUMO_TEAM in $CONF/testflight.env or pass --team" >&2; exit 2; }
: "${ASC_KEY_ID:?Set ASC_KEY_ID in $CONF/testflight.env}"
: "${ASC_ISSUER_ID:?Set ASC_ISSUER_ID in $CONF/testflight.env}"
ASC_KEY_PATH="${ASC_KEY_PATH:-$CONF/AuthKey_$ASC_KEY_ID.p8}"
[ -f "$ASC_KEY_PATH" ] || { echo "App Store Connect key not found: $ASC_KEY_PATH" >&2; exit 2; }

[ "$SKIP_CORE" = 1 ] || PROFILE=release "$ROOT/scripts/build-core-ios.sh"
cd "$IOS"
xcodegen generate --quiet

OUT="$IOS/build/testflight"
rm -rf "$OUT" && mkdir -p "$OUT"
AUTH=(-allowProvisioningUpdates -authenticationKeyPath "$ASC_KEY_PATH" -authenticationKeyID "$ASC_KEY_ID" -authenticationKeyIssuerID "$ASC_ISSUER_ID")
BUILD_NUMBER="$(date -u +%Y%m%d%H%M)"

xcodebuild archive -project Kumowatch.xcodeproj -scheme Kumowatch -configuration Release \
  -destination generic/platform=iOS -archivePath "$OUT/Kumowatch.xcarchive" "${AUTH[@]}" \
  DEVELOPMENT_TEAM="$TEAM" CODE_SIGN_STYLE=Automatic KUMO_BUNDLE_ID="$BUNDLE_ID" KUMO_CAPS="-full" \
  CURRENT_PROJECT_VERSION="$BUILD_NUMBER" -quiet

cat > "$OUT/ExportOptions.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>method</key><string>app-store-connect</string>
    <key>destination</key><string>upload</string>
    <key>teamID</key><string>$TEAM</string>
    <key>signingStyle</key><string>automatic</string>
    <key>manageAppVersionAndBuildNumber</key><false/>
</dict>
</plist>
PLIST

xcodebuild -exportArchive -archivePath "$OUT/Kumowatch.xcarchive" -exportOptionsPlist "$OUT/ExportOptions.plist" \
  -exportPath "$OUT/export" "${AUTH[@]}" -quiet
echo "Uploaded build $BUILD_NUMBER. It appears in App Store Connect → TestFlight after processing (5–30 min)."
