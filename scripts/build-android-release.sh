#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CONF="${KUMOWATCH_CONFIG:-$HOME/.config/kumowatch}"
PROPS="$CONF/android-signing.properties"
STORE="$CONF/android-release.jks"
DIST="$ROOT/dist"
INSTALL=0
SKIP_CORE=0
for arg in "$@"; do
  case "$arg" in
    --install) INSTALL=1 ;;
    --skip-core) SKIP_CORE=1 ;;
    *) echo "usage: $0 [--install] [--skip-core]" >&2; exit 2 ;;
  esac
done

if [ ! -f "$PROPS" ]; then
  mkdir -p "$CONF" && chmod 700 "$CONF"
  PASS="$(openssl rand -hex 24)"
  keytool -genkeypair -v -keystore "$STORE" -alias kumowatch -keyalg RSA -keysize 4096 -validity 36500 \
    -storepass "$PASS" -keypass "$PASS" -dname "CN=Kumowatch" > /dev/null
  printf 'storeFile=%s\nstorePassword=%s\nkeyAlias=kumowatch\nkeyPassword=%s\n' "$STORE" "$PASS" "$PASS" > "$PROPS"
  chmod 600 "$PROPS" "$STORE"
  echo "Created signing key $STORE. Back it up: updates must be signed with the same key."
fi

ABIS="${ABIS:-arm64-v8a}"
[ "$SKIP_CORE" = 1 ] || ABIS="$ABIS" PROFILE=release "$ROOT/scripts/build-core-android.sh"

cd "$ROOT/apps/android"
./gradlew -q :app:assembleRelease -Pkumowatch.abis="$(echo "$ABIS" | tr ' ' ',')"
VERSION="$(sed -n 's/.*versionName = "\(.*\)".*/\1/p' app/build.gradle.kts | head -1)"
BUILD="$(git -C "$ROOT" rev-list --count HEAD)"
mkdir -p "$DIST"
APK="$DIST/kumowatch-$VERSION-$BUILD.apk"
cp app/build/outputs/apk/release/app-release.apk "$APK"
cp "$APK" "$DIST/kumowatch-latest.apk"
echo "APK: $APK"

if [ "$INSTALL" = 1 ]; then
  for serial in $(adb devices | awk 'NR>1 && $2=="device" {print $1}'); do
    echo "Installing on $serial"
    adb -s "$serial" install -r "$APK"
  done
fi
