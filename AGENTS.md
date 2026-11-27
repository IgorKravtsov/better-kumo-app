# Agent notes

## Releases

A release pull request must:

- be titled `[Release] X.Y.Z` (e.g. `[Release] 0.2.17`). `[Release]` turns off the AI code review for it; CI still
  builds everything.
- carry the label `autosquash` (`gh pr create --label autosquash` or `gh pr edit <n> --add-label autosquash`). The
  Autosquash workflow turns on GitHub's auto-merge: GitHub squash-merges it as `Release X.Y.Z (#N)` once the required
  checks pass. Do not merge it by hand.

Steps: branch `release-X.Y.Z`; `scripts/bump-version.sh` (next patch; or `minor`, `major`, an exact `X.Y.Z`) sets
the version in `core/Cargo.toml`, `core/Cargo.lock`, `apps/ios/project.yml` and `apps/android/app/build.gradle.kts`;
open the pull request as above. After the merge, Autosquash tags the merge commit `vX.Y.Z` and starts
`.github/workflows/release.yml`, which builds and publishes; do not push the tag yourself.

## Merging

A ruleset on `main` requires the CI checks `core`, `windows`, `watcher`, `android` and `ios`. Merge a pull request
only when they pass; the owner may bypass them (`gh pr merge --admin`), agents do so only when told to. The label
`autosquash` on any pull request means "merge it by itself once CI passes".
