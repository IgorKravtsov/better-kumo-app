<div align="center">

<img src="apps/ios/Kumowatch/Assets.xcassets/AppIcon.appiconset/icon-1024.png" width="112" alt="Better Kumo icon">

# Better Kumo

**Your [Kumospace](https://www.kumospace.com) office in your pocket.**<br>
Native iPhone and Android apps plus a small Mac menu bar app that lets the phone see and control
the avatar on your desktop, and sends you the office's notifications.

[Install](#install) · [Features](#features) · [How it works](#how-it-works) · [Full install guide](docs/install.md) · [Development](#development)

</div>

<p align="center">
  <img src="docs/images/ios-desktop.png" width="250" alt="Desktop screen: live floor map of the desktop app">
  &nbsp;
  <img src="docs/images/ios-notifications.png" width="250" alt="Notification switches per status">
  &nbsp;
  <img src="docs/images/ios-ntfy.png" width="250" alt="Delivery and ntfy topic">
</p>

> Screenshots use a demo relay with made-up people (`cargo run -p kumo-relay --example demo_relay`).

## Features

| | Phone app alone | + Better Kumo on your Mac |
|---|:---:|:---:|
| Chat: DMs, groups, channels, threads, reactions, photos, GIFs, Markdown | ✅ | ✅ |
| Office: rooms, people, statuses, search | ✅ | ✅ |
| Calls in a room (audio and video) | ✅ | ✅ |
| **Desktop screen**: live floor map; move, change rooms and floors, follow people, set status, mic/camera, nudge | | ✅ |
| Take the desktop's call on the phone and hand it back | | ✅ |
| Notifications: messages, @mentions, nudges, knocks, broadcasts, Public Chat, Nearby Chat, someone joining your floor | Android (from its own connection) | ✅ iPhone and Android, via ntfy |
| Separate notification switches for Available / Away / Focusing / Unknown | Android | ✅ |
| Home screen widget: notifications on/off in one tap | Android | ✅ |

## Install

You need three things. The [full install guide](docs/install.md) explains every step, with options and fixes.

**1. On the Mac where Kumospace runs: Better Kumo for the menu bar.**

```sh
scripts/build-desktop-macos.sh        # → dist/Better Kumo.app
```

Copy `Better Kumo.app` to `/Applications` and open it (right-click → **Open** the first time). Before you start it,
make sure Kumospace desktop is installed and signed in. Better Kumo then:
- restarts Kumospace with phone control on;
- signs itself in from it;
- starts the relay the phone talks to.

Turn on **Start at login** in its menu.

**2. On your phone: the Better Kumo app.**

| iPhone (any Apple ID, even free) | Android 8+ |
|---|---|
| `scripts/build-ios-device.sh --team <TEAM_ID>` with the iPhone connected by cable | `scripts/build-android-release.sh --install` with the phone connected, or send `dist/kumowatch-latest.apk` to the phone |

Sign in with **Sign in with browser**, or with a sign-in link from a computer that is already signed in.

<img src="docs/images/ios-sign-in.png" width="220" alt="Sign-in screen">

**3. Connect them.**

1. Install [Tailscale](https://tailscale.com) on the Mac and the phone, so the phone can reach the Mac from anywhere.
2. In the Better Kumo menu, choose **Pair a phone…**.
3. Scan the code. On iPhone, use **Profile → Scan relay QR code**. On Android, use the camera app.

<img src="docs/images/desktop-pair.png" width="560" alt="Pairing page with QR codes">

**Notifications on iPhone** arrive through [ntfy](https://ntfy.sh):
1. In **Profile → Notifications relay**, type a topic (any hard-to-guess name) and tap **Save**.
2. Subscribe to the same topic in the ntfy app.

Android shows notifications itself by default (**This phone**). Switch to ntfy in the same place if you prefer it.

## How it works

```mermaid
flowchart LR
    subgraph Phone["📱 Phone: Better Kumo app"]
        UI["SwiftUI / Compose UI"]
        Core1["Rust core (kumo-core via UniFFI)"]
        UI --> Core1
    end
    subgraph Mac["💻 Mac"]
        KS["Kumospace desktop<br/>--remote-debugging-port=9222"]
        BK["Better Kumo menu bar app<br/>(supervisor + relay :8787)"]
        BK -- "DevTools protocol<br/>(localhost)" --> KS
    end
    Cloud["☁️ Kumospace, Sendbird, Daily"]
    Core1 -- "chat, office, calls" --> Cloud
    KS --> Cloud
    Core1 -- "HTTP over Tailscale / Wi-Fi<br/>map, move, status, calls" --> BK
    BK -- "alerts" --> ntfy["ntfy.sh"] --> Phone
```

- **The phone app talks to Kumospace directly** for chat, the office and calls. All logic lives in a shared Rust core.
  Swift and Kotlin only draw it.
- **Better Kumo on the Mac** starts Kumospace desktop with a local debugging port and reads it. It sees what the
  desktop sees: floors, rooms, people, nudges, knocks, broadcasts and Public/Nearby Chat. It runs a small **relay**
  (HTTP on port 8787) that the phone pairs with by QR code.
- **Notifications** come from the relay, filtered by your current status and your switches. iPhones get them through
  ntfy. Android can show them itself.

More: [architecture](docs/architecture.md) · [design decisions](docs/adr) · [glossary](GLOSSARY.md).

## Repository

| Path | What |
|---|---|
| [`apps/ios`](apps/ios) | iPhone app and widget (SwiftUI, XcodeGen) |
| [`apps/android`](apps/android) | Android app and widget (Jetpack Compose) |
| [`core`](core) | Rust workspace: `kumo-core` (protocol, chat, office, calls), `kumo-ffi` (UniFFI bindings), `kumo-relay` (relay and desktop bridge), `kumo-desktop` (Better Kumo menu bar app), `kumo-devtools`, `kumo-cli` |
| [`scripts`](scripts) | build and install scripts |
| [`docs`](docs) | [install guide](docs/install.md), [architecture](docs/architecture.md), [Kumospace API](docs/kumospace-api.md), [Sendbird protocol](docs/sendbird-protocol.md), [roadmap](docs/roadmap.md) |
| [`apps/watcher`](apps/watcher) | the first Node prototype; replaced by the relay, kept for reference |

## Development

Prerequisites (full list in the [install guide](docs/install.md#2-build-machine-one-time)):
- Rust with the iOS and Android targets;
- Xcode and `xcodegen`;
- the Android SDK and NDK, plus `cargo-ndk`;
- JDK 17.

```sh
cd core && cargo test                                  # the whole core: protocol, chat, relay, calls
cargo run -p kumo-cli -- --help                        # the core from a terminal: login, channels, tail-chat …
cargo run -p kumo-relay --example demo_relay           # a relay with a made-up office, for UI work and screenshots

scripts/build-core-ios.sh && (cd apps/ios && xcodegen generate && open Kumowatch.xcodeproj)
scripts/build-core-android.sh && (cd apps/android && ./gradlew :app:installDebug)
```

Re-run the `build-core-*` script after changing Rust code; `PROFILE=debug` builds faster.
Checks before a PR:

```sh
cd core && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

The [`kumospace-devtools`](.claude/skills/kumospace-devtools/SKILL.md) skill shows how to watch what the Kumospace
desktop app sends and receives.

> **Note:** Better Kumo is an independent project and is not affiliated with Kumospace. The debugging port gives
> local programs on that Mac full access to your Kumospace session; it is not reachable from the network.
