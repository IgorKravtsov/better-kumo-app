#!/bin/bash
# Sets the version of every app for a release.
#   scripts/bump-version.sh            # next patch: 0.2.16 -> 0.2.17
#   scripts/bump-version.sh minor      # 0.2.16 -> 0.3.0 (also: major)
#   scripts/bump-version.sh 1.0.0      # exactly this version
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# The workspace version, [workspace.package] in core/Cargo.toml.
workspace_version() { awk -F'"' '/^[[:space:]]*\[/ { s = ($0 ~ /^[[:space:]]*\[[[:space:]]*workspace\.package[[:space:]]*\][[:space:]]*(#.*)?$/) } s && /^[[:space:]]*version[[:space:]]*=/ { print $2; exit }' core/Cargo.toml; }
current="$(workspace_version)"
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
  sed -E "$expr" "$file" > "$file.tmp"
  # An unchanged file means the pattern found nothing, whatever else in it looks like the new version.
  if cmp -s "$file" "$file.tmp"; then rm -f "$file.tmp"; echo "$file: the version was not changed" >&2; exit 1; fi
  mv "$file.tmp" "$file"
}
replace core/Cargo.toml "/^[[:space:]]*\[[[:space:]]*workspace\.package[[:space:]]*\]/,/^[[:space:]]*\[/s/^([[:space:]]*version[[:space:]]*=[[:space:]]*)\"[^\"]*\"/\1\"$next\"/"
[ "$(workspace_version)" = "$next" ] || { echo "core/Cargo.toml: [workspace.package] version is not $next" >&2; exit 1; }
replace apps/ios/project.yml "s/(MARKETING_VERSION: )\"[^\"]*\"/\1\"$next\"/"
replace apps/android/app/build.gradle.kts "s/(kumowatch\.versionName\"\)\.orElse\()\"[^\"]*\"/\1\"$next\"/"
# The workspace crates' versions in Cargo.lock; no dependency changes, no network.
cargo update --workspace --offline --quiet --manifest-path core/Cargo.toml

echo "$current -> $next"
git status --short -- core/Cargo.toml core/Cargo.lock apps/ios/project.yml apps/android/app/build.gradle.kts
