# kumowatch

A better client for [Kumospace](https://www.kumospace.com): a desktop event watcher plus native iOS and Android apps
that share their logic through a Rust core.

| Path | What |
|---|---|
| `apps/watcher` | Node service that listens to the Kumospace desktop app over CDP and pushes events to ntfy. See its README section below. |
| `apps/ios` | SwiftUI app (XcodeGen). |
| `apps/android` | Jetpack Compose app. |
| `core` | Rust workspace: `kumo-core` (protocol, sync, state) and `kumo-ffi` (UniFFI bindings). |
| `docs` | `architecture.md`, `roadmap.md` (feature breakdown), `kumospace-api.md`, `kumospace-features.md`. |

## Mobile apps

Prerequisites: Rust with targets `aarch64-apple-ios aarch64-apple-ios-sim aarch64-linux-android x86_64-linux-android`,
Xcode + `xcodegen`, Android SDK + NDK, `cargo install cargo-ndk`, JDK 17.

    cd core && cargo test
    cargo run -p kumo-cli -- --help      # CLI over the core: login, channels, messages, tail-chat, ...

    scripts/build-core-ios.sh
    cd apps/ios && xcodegen generate && open Kumowatch.xcodeproj

    scripts/build-core-android.sh
    cd apps/android && ./gradlew :app:installDebug

Re-run the build script after changing Rust code. `PROFILE=debug` builds faster.

## Notification relay (`core/crates/kumo-relay`)

A small service for the Mac that keeps its own Kumospace chat connection (no presence in the office) and mirrors DMs,
group messages and mentions to phones: through ntfy (works without an Apple Developer account; tapping opens
`kumowatch://chat?channel=…` in our app) and through APNs (needs a key from an Apple Developer account).

    cargo run -p kumo-relay -- init --space <space> [--ntfy-topic <topic>]     # writes ~/.config/kumowatch/relay.toml
    cargo run -p kumo-cli -- transfer-token | cargo run -p kumo-relay -- login --token-stdin
    scripts/install-relay.sh                                                  # launchd agent com.kumowatch.relay
    kumo-relay pair --url http://<mac-address>:8787                           # link to open on the phone

For APNs add to `relay.toml`:

    [apns]
    team_id = "<TEAM ID>"
    key_id = "<KEY ID>"
    key_path = "/path/to/AuthKey_<KEY ID>.p8"
    topic = "com.kravtsov.kumowatch"
    sandbox = true

HTTP API (Bearer secret from `relay.toml`): `GET /v1/status`, `PUT /v1/enabled {enabled}`,
`PUT|DELETE /v1/devices/{apnsToken}`, `POST /v1/test`. The phone must reach the Mac (same network or Tailscale).

## Watcher (`apps/watcher`)

Listens to the Kumospace desktop app (Electron) over the Chrome DevTools Protocol and reacts to its WebSocket
traffic: ws.kumospace.com (presence, statuses, positions) and Sendbird (chat).

- `kumowatch.js` — the service: CDP attach, frame parsing, events, reconnect, launch watchdog.
- `rules.js` — what to do on events (ntfy pushes). Edit this to add scenarios.
- `launcher.js` — `node launcher.js launch` starts/focuses Kumospace with `--remote-debugging-port=9222`.
- `raycast/kumospace.sh` — Raycast Script Command wrapping the launcher.
- `replay-test.js` — replays a frame dump (`KUMO_DEBUG_LOG=frames.jsonl`) through rules with ntfy stubbed.

### launchd agents (~/Library/LaunchAgents)

    cd apps/watcher && npm ci && scripts/install-launchd.sh

installs `com.kumowatch.service` (the service, at login, auto-restarted; logs in `apps/watcher/logs/kumowatch.log`)
and updates `com.kumowatch.kumospace` (starts Kumospace with the debug port at login; not restarted, so a running
session is kept). Node defaults to the mise install; override with `KUMOWATCH_NODE`.

    launchctl kickstart -k gui/$(id -u)/com.kumowatch.service   # restart after editing rules.js
    launchctl bootout gui/$(id -u)/com.kumowatch.service        # stop / disable

If Kumospace is opened without the port (Dock, ⌥6 bound to the app) the watchdog relaunches it within a few
seconds — only if it is younger than 2 minutes, so an active session is never killed.

### Settings (env vars in the plist)

`NTFY_TOPIC`, `NTFY_SERVER`, `FORWARD_WHEN` (away,focusing), `KUMO_NEAR_RADIUS` (300),
`KUMO_DEBUG_LOG` (dump raw frames), `KUMO_WATCHDOG=0` (disable relaunch).

Note: the debug port gives any local process full access to your Kumospace session.
