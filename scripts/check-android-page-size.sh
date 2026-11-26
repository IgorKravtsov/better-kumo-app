#!/bin/bash
# Fails unless every 64-bit native library in the given APKs and bundles loads on 16 KiB page devices,
# which Google Play requires: ELF LOAD segments aligned to 16 KiB, and in APKs, zip entries aligned too.
set -euo pipefail

[ $# -gt 0 ] || { echo "usage: $0 <file.apk|file.aab>..." >&2; exit 2; }
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}}"
NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-$(ls -d "$SDK/ndk/"* | sort -V | tail -1)}}"
READELF="$(ls "$NDK"/toolchains/llvm/prebuilt/*/bin/llvm-readelf | head -1)"
ZIPALIGN="$(ls "$SDK"/build-tools/*/zipalign | sort -V | tail -1)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

bad=0
for file in "$@"; do
  lib=lib
  case "$file" in
    *.apk) "$ZIPALIGN" -c -P 16 4 "$file" || { echo "$file: native libraries are not zip-aligned to 16 KiB" >&2; bad=1; } ;;
    *.aab) lib=base/lib ;;
  esac
  rm -rf "$WORK/x" && mkdir "$WORK/x"
  unzip -q -o "$file" "$lib/arm64-v8a/*.so" "$lib/x86_64/*.so" -d "$WORK/x" 2> /dev/null || true
  libs="$(find "$WORK/x" -name '*.so')"
  [ -n "$libs" ] || { echo "$file: no 64-bit native libraries" >&2; bad=1; continue; }
  while IFS= read -r so; do
    for align in $("$READELF" -lW "$so" | awk '$1 == "LOAD" {print $NF}'); do
      if [ $((align)) -lt 16384 ]; then
        echo "$file: ${so#"$WORK/x/"} has a LOAD segment aligned to $align, not 16 KiB" >&2
        bad=1
        break
      fi
    done
  done <<< "$libs"
done
[ "$bad" = 0 ] && echo "16 KiB pages: OK ($*)"
exit "$bad"
