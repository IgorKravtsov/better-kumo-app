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
# Google Play requires 16 KiB pages on 64-bit; NDK r28+ links that way by default, r27 needs the flag.
PAGES="-C link-arg=-Wl,-z,max-page-size=16384"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="${CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS:+$CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS }$PAGES"
export CARGO_TARGET_X86_64_LINUX_ANDROID_RUSTFLAGS="${CARGO_TARGET_X86_64_LINUX_ANDROID_RUSTFLAGS:+$CARGO_TARGET_X86_64_LINUX_ANDROID_RUSTFLAGS }$PAGES"
# Build into a staging directory and swap it in only on success: a failed build keeps the last working set,
# and an ABI left from an earlier build can't ship a stale core.
STAGE="$APP/src/main/jniLibs.new"
rm -rf "$STAGE"
trap 'rm -rf "$STAGE"' EXIT
cargo ndk "${NDK_ARGS[@]}" --platform 26 -o "$STAGE" build -p kumo-ffi --lib ${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"}
for abi in $ABIS; do [ -s "$STAGE/$abi/libkumo_ffi.so" ] || { echo "cargo ndk built no core for $abi" >&2; exit 1; }; done
rm -rf "$APP/src/main/jniLibs"
mv "$STAGE" "$APP/src/main/jniLibs"

cargo build -p kumo-ffi --lib
rm -rf "$APP/src/main/java/uniffi"
cargo run -q -p kumo-ffi --features bindgen --bin uniffi-bindgen -- \
  generate --library "$CORE/target/debug/libkumo_ffi.$([ "$(uname)" = Darwin ] && echo dylib || echo so)" --language kotlin --out-dir "$APP/src/main/java"
echo "Android core -> $APP/src/main/jniLibs + $APP/src/main/java/uniffi"
