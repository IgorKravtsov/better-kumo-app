#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IOS="$ROOT/apps/ios"
TEAM="${KUMO_TEAM:-}"
BUNDLE_ID="${KUMO_BUNDLE_ID:-com.kravtsov.kumowatch}"
CAPS=""
DEVICE=""
SKIP_CORE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --team) TEAM="$2"; shift ;;
    --bundle-id) BUNDLE_ID="$2"; shift ;;
    --full) CAPS="-full" ;;
    --device) DEVICE="$2"; shift ;;
    --skip-core) SKIP_CORE=1 ;;
    *) echo "usage: $0 --team <TEAM_ID> [--bundle-id <id>] [--full] [--device <name|udid>] [--skip-core]" >&2; exit 2 ;;
  esac
  shift
done
[ -n "$TEAM" ] || { echo "Pass --team <TEAM_ID> (Xcode > Settings > Accounts) or set KUMO_TEAM" >&2; exit 2; }

[ "$SKIP_CORE" = 1 ] || PROFILE=release "$ROOT/scripts/build-core-ios.sh"
cd "$IOS"
xcodegen generate --quiet

if [ -z "$DEVICE" ]; then
  DEVICE="$(xcrun devicectl list devices 2>/dev/null | awk '/iPhone|iPad/ && /available|connected/ {print $3; exit}')"
fi
[ -n "$DEVICE" ] || { echo "No iPhone found. Connect it by cable (or same Wi-Fi after pairing) and unlock it." >&2; exit 1; }
INFO="$(mktemp)"
xcrun devicectl device info details --device "$DEVICE" --json-output "$INFO" > /dev/null
UDID="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["result"]["hardwareProperties"]["udid"])' "$INFO")"
rm -f "$INFO"

BUILD="$IOS/build/device"
xcodebuild -project Kumowatch.xcodeproj -scheme Kumowatch -configuration Release \
  -destination "id=$UDID" -derivedDataPath "$BUILD" -allowProvisioningUpdates \
  DEVELOPMENT_TEAM="$TEAM" CODE_SIGN_STYLE=Automatic KUMO_BUNDLE_ID="$BUNDLE_ID" KUMO_CAPS="$CAPS" \
  CURRENT_PROJECT_VERSION="$(git -C "$ROOT" rev-list --count HEAD)" build -quiet
APP="$BUILD/Build/Products/Release-iphoneos/Kumowatch.app"
echo "App: $APP"

xcrun devicectl device install app --device "$DEVICE" "$APP"
xcrun devicectl device process launch --device "$DEVICE" "$BUNDLE_ID" || true
