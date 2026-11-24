#!/bin/bash
# Run in Git Bash on Windows with Rust and Inno Setup 6.5+ (`winget install JRSoftware.InnoSetup`).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST="$ROOT/dist"
VERSION="${KUMO_VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/core/Cargo.toml" | head -1)}"
BIN="$ROOT/core/target/release"

ISCC="$(command -v iscc || true)"
for candidate in "/c/Program Files (x86)/Inno Setup 6/ISCC.exe" "${LOCALAPPDATA:-}\\Programs\\Inno Setup 6\\ISCC.exe"; do
  if [ -z "$ISCC" ] && [ -f "$candidate" ]; then ISCC="$candidate"; fi
done
[ -n "$ISCC" ] || { echo "Inno Setup 6 not found: winget install JRSoftware.InnoSetup" >&2; exit 1; }
command -v 7z > /dev/null || { echo "7z not found: install 7-Zip and add it to PATH" >&2; exit 1; }

# Static CRT: a clean Windows has no VCRUNTIME140.dll.
RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static" cargo build --manifest-path "$ROOT/core/Cargo.toml" -p kumo-desktop -p kumo-mcp --release
mkdir -p "$DIST"

# Git Bash would rewrite ISCC's /switches as paths.
MSYS_NO_PATHCONV=1 "$ISCC" /Q "/DAppVersion=$VERSION" "/DBinDir=$(cygpath -w "$BIN")" "/DOutputDir=$(cygpath -w "$DIST")" \
  "$(cygpath -w "$ROOT/packaging/windows/better-kumo.iss")"
echo "$DIST/Better-Kumo-Windows-x64-$VERSION-Setup.exe"

PORTABLE="$(mktemp -d)/Better Kumo"
mkdir -p "$PORTABLE"
cp "$BIN/better-kumo.exe" "$PORTABLE/Better Kumo.exe"
cp "$BIN/kumo-mcp.exe" "$PORTABLE/kumo-mcp.exe"
rm -f "$DIST/Better-Kumo-Windows-x64-$VERSION.zip"
(cd "$(dirname "$PORTABLE")" && 7z a -tzip "$(cygpath -w "$DIST/Better-Kumo-Windows-x64-$VERSION.zip")" "Better Kumo" > /dev/null)
echo "$DIST/Better-Kumo-Windows-x64-$VERSION.zip"
