# Agent notes

## Releases

A release pull request must:

- be titled `[Release] X.Y.Z` (e.g. `[Release] 0.2.17`). `[Release]` turns off the AI code review for it; CI still
  builds everything.
- carry the label `autosquash` (`gh pr create --label autosquash` or `gh pr edit <n> --add-label autosquash`). The
  Autosquash workflow then squash-merges it as `Release X.Y.Z (#N)` once every check passes. Do not merge it by hand.

Steps: branch `release-X.Y.Z`; `scripts/bump-version.sh` (next patch; or `minor`, `major`, an exact `X.Y.Z`) sets
the version in `core/Cargo.toml`, `core/Cargo.lock`, `apps/ios/project.yml` and `apps/android/app/build.gradle.kts`;
open the pull request as above; after it is merged, push the tag `vX.Y.Z` on the merge commit
(`.github/workflows/release.yml` builds and publishes from the tag).
