#!/bin/bash
# Sets the version of every app for a release.
#   scripts/bump-version.sh            # next patch: 0.2.16 -> 0.2.17
#   scripts/bump-version.sh minor      # 0.2.16 -> 0.3.0 (also: major)
#   scripts/bump-version.sh 1.0.0      # exactly this version
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

current="$(sed -n 's/^version = "\(.*\)"/\1/p' core/Cargo.toml | head -1)"
IFS=. read -r major minor patch <<< "$current"
case "${1:-patch}" in
  patch) next="$major.$minor.$((patch + 1))" ;;
  minor) next="$major.$((minor + 1)).0" ;;
  major) next="$((major + 1)).0.0" ;;
  *) next="$1" ;;
esac
[[ "$next" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Not a version: $next (expected X.Y.Z, patch, minor or major)" >&2; exit 1; }

# sed -i differs between macOS and GNU, so write through a temp file.
replace() {
  local file="$1" expr="$2"
  sed -E "$expr" "$file" > "$file.tmp" && mv "$file.tmp" "$file"
  grep -qF "$next" "$file" || { echo "$file: the version was not changed" >&2; exit 1; }
}
replace core/Cargo.toml "1,/^version = /s/^version = \".*\"/version = \"$next\"/"
replace apps/ios/project.yml "s/(MARKETING_VERSION: )\"[^\"]*\"/\1\"$next\"/"
replace apps/android/app/build.gradle.kts "s/(kumowatch\.versionName\"\)\.orElse\()\"[^\"]*\"/\1\"$next\"/"
# The workspace crates' versions in Cargo.lock; no dependency changes, no network.
cargo update --workspace --offline --quiet --manifest-path core/Cargo.toml

echo "$current -> $next"
git status --short -- core/Cargo.toml core/Cargo.lock apps/ios/project.yml apps/android/app/build.gradle.kts
