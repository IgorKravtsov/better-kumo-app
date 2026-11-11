#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CORE="$ROOT/core"
APP="$ROOT/apps/android/app"
PROFILE="${PROFILE:-release}"
ABIS="${ABIS:-arm64-v8a x86_64}"
CARGO_FLAGS=()
[ "$PROFILE" = "release" ] && CARGO_FLAGS+=(--release)

export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-$(ls -d "${ANDROID_HOME:-$HOME/Library/Android/sdk}/ndk/"* | sort -V | tail -1)}}"
command -v cargo-ndk > /dev/null || { echo "install cargo-ndk: cargo install cargo-ndk" >&2; exit 1; }

cd "$CORE"
NDK_ARGS=()
for abi in $ABIS; do NDK_ARGS+=(-t "$abi"); done
cargo ndk "${NDK_ARGS[@]}" --platform 26 -o "$APP/src/main/jniLibs" build -p kumo-ffi --lib ${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"}

cargo build -p kumo-ffi --lib ${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"}
rm -rf "$APP/src/main/java/uniffi"
cargo run -q -p kumo-ffi --features bindgen --bin uniffi-bindgen -- \
  generate --library "$CORE/target/$PROFILE/libkumo_ffi.$([ "$(uname)" = Darwin ] && echo dylib || echo so)" --language kotlin --out-dir "$APP/src/main/java"
echo "Android core -> $APP/src/main/jniLibs + $APP/src/main/java/uniffi"
