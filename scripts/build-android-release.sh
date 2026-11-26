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
# Many watches (Galaxy Watch 4–6, Pixel Watch 1–2) run 32-bit apps, so the Wear OS app also ships armeabi-v7a.
WEAR_ABIS="${WEAR_ABIS:-arm64-v8a armeabi-v7a}"
CORE_ABIS="$(printf '%s\n' $ABIS $WEAR_ABIS | awk '!seen[$0]++' | tr '\n' ' ')"
[ "$SKIP_CORE" = 1 ] || ABIS="$CORE_ABIS" PROFILE=release "$ROOT/scripts/build-core-android.sh"
for abi in $CORE_ABIS; do
  [ -f "$ROOT/apps/android/app/src/main/jniLibs/$abi/libkumo_ffi.so" ] || { echo "No Rust core for $abi; run without --skip-core" >&2; exit 1; }
done

cd "$ROOT/apps/android"
VERSION="${KUMO_VERSION:-$(sed -n 's/.*orElse("\([0-9.]*\)").*/\1/p' app/build.gradle.kts | head -1)}"
commas() { printf '%s\n' "$@" | paste -sd, -; }
# shellcheck disable=SC2086 # split the ABI lists the way the core build does
./gradlew -q :app:assembleRelease :wear:assembleRelease :app:bundleRelease :wear:bundleRelease -Pkumowatch.abis="$(commas $ABIS)" \
  -Pkumowatch.wearAbis="$(commas $WEAR_ABIS)" -Pkumowatch.versionName="$VERSION"
BUILD="$(git -C "$ROOT" rev-list --count HEAD)"
mkdir -p "$DIST"
APK="$DIST/kumowatch-$VERSION-$BUILD.apk"
cp app/build/outputs/apk/release/app-release.apk "$APK"
cp "$APK" "$DIST/kumowatch-latest.apk"
WEAR_APK="$DIST/kumowatch-wear-$VERSION-$BUILD.apk"
cp wear/build/outputs/apk/release/wear-release.apk "$WEAR_APK"
cp "$WEAR_APK" "$DIST/kumowatch-wear-latest.apk"
# One Play listing routes these bundles to their form factors.
cp app/build/outputs/bundle/release/app-release.aab "$DIST/kumowatch-play-phone.aab"
cp wear/build/outputs/bundle/release/wear-release.aab "$DIST/kumowatch-play-wear.aab"
echo "APK: $APK"
echo "Wear OS APK: $WEAR_APK (install on the watch with adb, see docs/install.md)"
echo "Google Play bundles: $DIST/kumowatch-play-phone.aab, $DIST/kumowatch-play-wear.aab"

if [ "$INSTALL" = 1 ]; then
  for serial in $(adb devices | awk 'NR>1 && $2=="device" {print $1}'); do
    chars="$(adb -s "$serial" shell getprop ro.build.characteristics)" || { echo "Can't ask $serial what it is; skipping it" >&2; continue; }
    case "$chars" in *watch*) target="$WEAR_APK" ;; *) target="$APK" ;; esac
    echo "Installing $(basename "$target") on $serial"
    adb -s "$serial" install -r "$target"
  done
fi
