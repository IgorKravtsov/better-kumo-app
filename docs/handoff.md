# Handoff: kumowatch → native Kumospace client

Date: 2026-11-11. Talk to the user in Russian. Repo docs and code are in English.

## Goal

The official Kumospace mobile app works badly, chat especially. Build our own iOS + Android apps that use the same
Kumospace backends, reach feature parity with the official app, and make chat fast: local-first, background sync,
no scrolling to trigger loading. Shared logic in Rust (the user wants practice with Rust). The existing desktop watcher
script must keep working. More features later; parity first.

## Where things are

- Repo: `/Users/igorkravtsov/user/Projects/kumowatch`, branch `monorepo-restructure`. Everything is **staged but not
  committed** (the user has not asked to commit yet).
- Read first: `README.md`, `docs/roadmap.md` (feature breakdown, milestones M0–M8), `docs/architecture.md`,
  `docs/kumospace-features.md` (screens of the official app), `docs/kumospace-api.md` (REST, WS, Sendbird, Firebase).
- Layout: `apps/watcher` (Node CDP watcher), `apps/ios` (SwiftUI, XcodeGen), `apps/android` (Compose, AGP 9.5,
  compileSdk 37), `core/` (Rust: `kumo-core` + `kumo-ffi` with UniFFI 0.32), `scripts/build-core-{ios,android}.sh`.
- Reverse-engineering artifacts (outside the repo, not committed):
  `~/.cache/kumowatch-re/` with the XAPK, `an/decompiled.js` (70 MB hermes-dec output; functions have
  `// Original name:` comments; use `/usr/bin/grep -a`, the default grep is ugrep), `an/hstr.txt` (string table),
  `an/names.txt`, and the original `api-spec.md` / `features.md`. `hbc_strings.py` and `sniff.js` are helper scripts
  (`sniff.js` = read-only CDP network sniffer for the desktop app on mac-m4).
- The user's own coding rules (`~/.claude/CLAUDE.md`): no code comments unless asked; JS/TS functions with more than
  two arguments take an object; TS types not declared inside functions.

## Done

1. Analysed the official Android app 202610.24.411 (Expo/React Native, Hermes v99): routes, screens, API, Sendbird
   usage, causes of slow chat. Cross-checked REST calls against the live desktop app on `mac-m4` via CDP.
2. Restructured into a monorepo. Watcher moved to `apps/watcher` (logic unchanged, still loads; `npm ci` done
   locally); launchd plists are now templates + `apps/watcher/scripts/install-launchd.sh`; the Raycast script finds the
   launcher relative to itself.
3. Rust core: endpoints/URL building, pagination and errors, WS connect URL + client/server ops, Sendbird channel URL
   scheme. 10 tests pass, clippy clean.
4. iOS and Android stub apps (tabs Chat / Space / You) build and call Rust. Verified on the iOS 26.1 simulator and
   the Android 36 emulator.

## Not done / open

- **mac-m4 (ssh host `mac-m4`, user `shatori`, repo `~/usr/projects/kumowatch`) was not touched.** After the branch
  is merged and pulled there: `cd apps/watcher && npm ci && scripts/install-launchd.sh`, then point Raycast's script
  directory at `apps/watcher/raycast`. Until then the running service uses the old root paths. Do not restart
  Kumospace there unexpectedly.
- Running the **official** app in an emulator: the APKPure XAPK only has the `armeabi-v7a` split. The arm64
  Android 36 image is 64-bit only; the Android 30 image hangs at boot. Solution in progress: AVD **`kumo_play36`**
  (Android 36, Google Play, arm64), already created. The user signs into Google Play themselves, installs Kumospace
  from the store, and signs into Kumospace. Never ask for or handle their passwords.
- `docs/kumospace-api.md` and `KumoConfig` contain the public Firebase web API key and Sendbird app id from the
  official app. Repo visibility on GitHub is unknown (`gh` could not resolve `IgorKravtsov/kumowatch`).

## Session 2 (2026-11-11): what changed

- Sign-in solved without passwords or native Google SDKs: Kumospace's desktop hand-off. `POST /v1/account/desktop/token`
  mints a Firebase custom token; `kumospace://login?token=…` carries it; `signInWithCustomToken` gives an independent
  session. Used for browser sign-in (`https://www.kumospace.com/auth/redirect/desktop`) and device-to-device transfer.
  Details: `docs/kumospace-api.md` §7.1.
- `kumo-cli` (core/crates/kumo-cli): `login --from-desktop` (reads the desktop app's Firebase user over CDP port 9222
  on this Mac; start it with `node apps/watcher/launcher.js launch`), `transfer-token [--link]`, `spaces`,
  `chat-config`, `members`, `channels`, `messages`, `changelog`, `tail-chat`, `tail-space`, `chat-sync --db`,
  `probe-file`. Session file: `~/.config/kumowatch/session.json` (0600). The user's session is already imported there.
- Sendbird client in Rust works against the live backend (`docs/sendbird-protocol.md` §0 lists verified facts).
- `ChatEngine` + SQLite store, FFI `KumoApp`, and real chat UIs on Android (Compose) and iOS (SwiftUI). See
  `docs/roadmap.md` "Progress".
- CI workflow added (`.github/workflows/ci.yml`), not run yet (nothing pushed).
- Testing on devices: `kumo-cli transfer-token --link` piped into the device tool's `open <bundle> "$link"` signs the
  app in without showing the token. iOS simulator builds must be signed (do not pass `CODE_SIGNING_ALLOWED=NO`), or the
  Keychain write fails silently. iOS core build needs `IPHONEOS_DEPLOYMENT_TARGET=17.0` (set in the script).
- Still nothing committed. The local Kumospace desktop app was closed at the user's request.

## Session 2, later (2026-11-11)

- Office tab and calls on both platforms (Daily SDK 0.41). Key facts: Daily room per floor, participant `userId` =
  census key, subscribe only to zone mates, identify self by entity id. See `kumospace-api.md` §5.
- Android "Mirror notifications" foreground service + home-screen widget; nudges come from the space socket (`playSound`).
- Chat actions: reactions, edit, delete (soft delete for thread parents), threads, image upload, mentions, DM creation,
  member sheet, unread divider, image viewer. Offline deletions via message changelogs.
- `kumo-relay` (Rust) for iOS notifications: ntfy works now; APNs and the iOS widget need an Apple Developer team.
- Testing rule the user set: in calls join only the user's own room; write operations only in the self-DM.

## Next

1. Ask the user to commit / open a PR; run CI. Apple Developer team + APNs key needed for iOS pushes and widget.
2. Deploy `kumo-relay` on mac-m4 (`scripts/install-relay.sh`), pair the iPhone.
2. M3 remainder: thread screen, reactions, mention autocomplete, image upload, edit/delete, DM creation, first-unread marker.
3. M4 push: decide between background sync + local notifications and a relay (see roadmap M4 constraint).
4. M5 Space tab over `space_socket` (census, rooms, teleport).

## Immediate task for the new thread

1. Open the `kumo_play36` Android emulator in the T3 Device panel so the user can use it
   (`device_list` → `device_open`). Agent device access was just enabled in T3 settings.
2. Tell the user, in Russian, to sign into Google Play in the emulator, install Kumospace, and sign in.
3. While the user explores, collect what is slow or annoying and add it to `docs/roadmap.md` as requirements.
   Screenshots via `device_screenshot` are fine; do not tap through the user's account without being asked.

## Plan after that

1. Commit the branch (ask first), open a PR, migrate mac-m4 as described above.
2. M0: `kumo-cli` crate (login, spaces, channels, tail-space) to test the core against the real backend from the Mac;
   CI (cargo test, iOS sim build, Android assembleDebug).
3. M0 spike, the key decision: implement the Sendbird client protocol in Rust (WS `CMD{json}` frames: LOGI, MESG,
   READ, ... + REST `/v3` with changelogs). The official Sendbird JS SDK inside `decompiled.js` is the reference.
   If it proves impractical, fall back to native Sendbird SDKs per platform.
4. M1 auth (Firebase REST: password, `signInWithIdp` for Google/Microsoft, `securetoken` refresh), then M2 spaces,
   then M3 chat with a SQLite cache and background sync. Then push, office, session chat, calls (Daily native SDKs).
