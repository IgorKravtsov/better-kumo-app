#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CORE="$ROOT/core"
OUT="$ROOT/apps/ios/KumoCore"
PROFILE="${PROFILE:-release}"
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"
CARGO_FLAGS=()
[ "$PROFILE" = "release" ] && CARGO_FLAGS+=(--release)

cd "$CORE"
for target in aarch64-apple-ios aarch64-apple-ios-sim; do
  cargo build -p kumo-ffi --lib --target "$target" ${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"}
done
cargo build -p kumo-ffi --lib ${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"}

GEN="$CORE/target/uniffi-swift"
rm -rf "$GEN" && mkdir -p "$GEN/headers"
cargo run -q -p kumo-ffi --features bindgen --bin uniffi-bindgen -- \
  generate --library "$CORE/target/$PROFILE/libkumo_ffi.dylib" --language swift --out-dir "$GEN"
mv "$GEN"/*.h "$GEN/headers/"
cat "$GEN"/*.modulemap > "$GEN/headers/module.modulemap"

rm -rf "$OUT" && mkdir -p "$OUT/Sources"
xcodebuild -create-xcframework \
  -library "$CORE/target/aarch64-apple-ios/$PROFILE/libkumo_ffi.a" -headers "$GEN/headers" \
  -library "$CORE/target/aarch64-apple-ios-sim/$PROFILE/libkumo_ffi.a" -headers "$GEN/headers" \
  -output "$OUT/KumoFFI.xcframework" > /dev/null
cp "$GEN"/*.swift "$OUT/Sources/"
echo "iOS core -> $OUT"
