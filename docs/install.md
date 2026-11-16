# Installing Better Kumo

The apps are called **Better Kumo** on phones and on the Mac. Bundle IDs, package names and the Rust crates keep the
internal name *kumowatch*. This guide covers every piece end to end: building, installing the apps on real phones, and setting up the Mac that
runs Kumospace desktop. The sections are written so you can follow them top to bottom on a clean machine.

## 1. What runs where

```
 Phone (Android / iPhone)                Mac with Kumospace desktop ("office Mac")
 ┌──────────────────────────┐            ┌─────────────────────────────────────────────┐
 │ Better Kumo app          │            │ Kumospace.app  --remote-debugging-port=9222 │
 │  · chat, office, calls   │──Kumospace─┤        ▲ CDP (localhost only)               │
 │    (talks to Kumospace,  │  cloud     │        │                                    │
 │     Sendbird, Daily      │            │ kumo-relay  (launchd, port 8787)            │
 │     directly)            │            │  · own chat connection → notifications      │
 │  · Desktop screen  ──────┼──HTTP──────┤  · desktop bridge: map, move, status,       │
 │  · relay notifications   │ (Tailscale │    mic/camera, nudges, knocks               │
 └──────────────────────────┘  or LAN)   └─────────────────────────────────────────────┘
```

- **The app alone** gives you chat, the office map, people, and calls. It needs no Mac.
- **kumo-relay on the office Mac** adds two things. The first is the *Desktop* screen, which controls the avatar the
  desktop app shows. The second is notifications mirrored from Kumospace: DMs, mentions, nudges and door knocks.
  - On Android, notifications are shown natively by the app (home-screen widget toggle).
  - On iPhone, notifications come through ntfy (free) or APNs (needs a paid Apple Developer account).
- The phone reaches the relay over the same Wi-Fi or, better, over **Tailscale**, which works from anywhere.

| Piece | Where | Needed for |
|---|---|---|
| Android app | Android phone, 8.0+, arm64 | everything on Android |
| iOS app | iPhone, iOS 17+ | everything on iPhone |
| Kumospace desktop with debug port | office Mac | Desktop screen, nudges and knocks |
| `kumo-relay` | office Mac | Desktop screen, iPhone notifications |
| Tailscale | office Mac + phones | reaching the relay away from the office Wi-Fi |
| ntfy app | iPhone | iPhone notifications without a paid Apple account |

The build machine and the office Mac can be the same Mac or two different ones.

## 2. Build machine (one-time)

You need a Mac with Apple Silicon for iOS. Android-only builds also work on Linux.

```sh
# Xcode from the App Store, then:
xcode-select --install
brew install xcodegen openjdk@17 android-commandlinetools   # or install Android Studio instead
brew install --cask android-platform-tools                   # adb

# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk

```

Install the Android SDK with Android Studio → Settings → Languages & Frameworks → Android SDK:
- SDK Platform **API 37**;
- Build-Tools **36.1+**;
- **NDK (Side by side)** 27.x. The build scripts pick the newest NDK they find.

- Gradle finds the SDK through `ANDROID_HOME`, which defaults to `~/Library/Android/sdk`.
- Or put `sdk.dir=/path/to/sdk` into `apps/android/local.properties`.
- Use JDK 17: `export JAVA_HOME=$(/usr/libexec/java_home -v 17)`.

Check the core:

```sh
cd core && cargo test
```

## 3. Android phone

### 3.1 Build the APK

```sh
scripts/build-android-release.sh
```

- **First run.** Creates a signing key in `~/.config/kumowatch/`: `android-release.jks` and
  `android-signing.properties`.
  - **Back up both files** (for example to a password manager).
  - Every update must be signed with the same key. Without it you have to uninstall the app, which loses the sign-in
    and settings, before a new build installs.
- **Output.** `dist/kumowatch-<version>-<build>.apk`, plus a copy at `dist/kumowatch-latest.apk`.
- **Architecture.** arm64 only, which covers every modern phone. Use `ABIS="arm64-v8a armeabi-v7a"` for an old
  32-bit phone.
- **Rebuilding.** `--skip-core` rebuilds the APK without recompiling Rust when only Kotlin changed.

The release app is `com.kravtsov.kumowatch` and is called "Better Kumo". Development builds
(`./gradlew :app:installDebug`) are `com.kravtsov.kumowatch.debug` and are called "Better Kumo Dev". Both can sit on the
same phone.

### 3.2 Install it

**Option A — USB cable** (fastest; also for updates):

1. On the phone, open Settings → About phone and tap *Build number* 7 times to turn on Developer options.
2. Settings → System → Developer options → *USB debugging*: on.
3. Connect the cable and accept the "Allow USB debugging?" prompt on the phone.
4. Run:

   ```sh
   adb devices                                  # the phone must show as "device"
   scripts/build-android-release.sh --install   # builds and installs on every connected device
   # or, for an existing APK:
   adb install -r dist/kumowatch-latest.apk
   ```

**Option B — Wireless adb** (Android 11+, phone and Mac on the same Wi-Fi):

1. Developer options → *Wireless debugging* → *Pair device with pairing code*.
2. Pair, using the address shown in the pairing dialog:

   ```sh
   adb pair <ip>:<pair-port>            # enter the 6-digit code
   ```

3. Connect, using the address on the Wireless debugging screen:

   ```sh
   adb connect <ip>:<port>
   adb install -r dist/kumowatch-latest.apk
   ```

**Option C — Send the file** (no developer options at all):

1. Send `dist/kumowatch-latest.apk` to the phone: Telegram "Saved messages", Google Drive, AirDrop-like apps,
   or a USB file copy.
2. Tap the file on the phone.
3. Android asks to allow installs from that app (Files, Telegram, …): allow it.
4. Google Play Protect may warn "unknown developer": choose *More details → Install anyway*.
5. To update later, install a newer APK the same way. Data is kept.

### 3.3 First launch on Android

1. Allow **notifications** when asked. They are needed for chat alerts and the relay.
2. Sign in (see §5).
3. For calls, allow **microphone** and **camera** on first use.
4. Turn off battery optimization so mirroring survives in the background: Settings → Apps → Better Kumo → Battery →
   *Unrestricted*. On Samsung, Xiaomi and similar phones, also see dontkillmyapp.com.
5. Add the **widget**: long-press the home screen → Widgets → Better Kumo.
   - Tapping it turns notification mirroring on or off.
   - When it is on, a persistent "Kumospace alerts" notification shows that it runs.
6. Link the relay for the Desktop screen (§6.6).

## 4. iPhone

There is no "APK" for iOS. Apple only runs apps that are signed for your device. The app is built on a Mac and
installed with Xcode tools.

### 4.1 Pick an Apple account

| | Free Apple ID ("Personal Team") | Paid Apple Developer Program ($99/year) |
|---|---|---|
| Install on your own iPhone | yes | yes |
| App keeps working | **7 days**, then rebuild and reinstall | 1 year; TestFlight builds 90 days |
| Push notifications (APNs) | no; use ntfy (§6.7) | yes |
| Home-screen widget sharing state (App Group) | no | yes |
| Share with other people | no (must be their Apple ID) | TestFlight, up to 10,000 testers |
| Install without a cable, updates over the air | no | TestFlight (§4.4) |

Everything else works the same on both: chat, office, calls, the Desktop screen and ntfy notifications.

### 4.2 One-time setup

1. Xcode → Settings → Accounts → **+** → Apple ID → sign in.
2. Note the **Team ID**. Xcode → Settings → Accounts → select the team → the ID is shown in brackets. It can also be
   found with:

   ```sh
   defaults read com.apple.dt.Xcode IDEProvisioningTeamByIdentifier | grep -E 'teamID|teamName|isFree'
   ```

3. Connect the iPhone by cable, unlock it and tap *Trust This Computer*.
4. On the iPhone, turn on Settings → Privacy & Security → **Developer Mode**. The phone restarts; confirm.
   The switch appears only after the phone has been connected to Xcode once. If you don't see it, open
   Xcode → Window → Devices and Simulators with the phone connected.
5. After the first pairing, the phone can be reached over Wi-Fi as well: Devices and Simulators →
   *Connect via network*.

### 4.3 Build and install

```sh
scripts/build-ios-device.sh --team <TEAM_ID>                 # free or paid team, base features
scripts/build-ios-device.sh --team <TEAM_ID> --full          # paid team: + push notifications + widget App Group
```

The script:

1. builds the Rust core for iOS;
2. generates the Xcode project;
3. signs with automatic provisioning, which registers the device and the app IDs;
4. installs on the connected iPhone and starts the app.

Options:

- `--device <name|udid>` — choose the phone when several are connected (`xcrun devicectl list devices`).
- `--bundle-id <id>` — use a different bundle ID. This is needed if Apple says `com.kravtsov.kumowatch` is taken by
  another team. The widget and the App Group follow it automatically.
- `--skip-core` — skip the Rust build when only Swift changed.
- `KUMO_TEAM=<TEAM_ID>` in the environment works instead of `--team`.

The first time a **free** team installs an app, iOS refuses to open it as an "Untrusted Developer". Fix it on the
iPhone: Settings → General → VPN & Device Management → your Apple ID → *Trust*.

**Free team, every 7 days:** re-run `scripts/build-ios-device.sh --team <TEAM_ID> --skip-core` with the phone
connected. Sign-in and data are kept.

**From Xcode instead of the script:**

1. Run:

   ```sh
   scripts/build-core-ios.sh
   cd apps/ios && xcodegen generate && open Kumowatch.xcodeproj
   ```

2. Select the *Kumowatch* target → Signing & Capabilities → choose your team. Do the same for the *KumowatchWidget*
   target.
3. Pick your iPhone as the run destination and press ⌘R.

`xcodegen generate` resets the team, so the script is the more convenient path.

### 4.4 TestFlight (paid team)

TestFlight is the easiest way to keep the app on your iPhones. No cable and no Developer Mode are needed. A build
works for 90 days, and new builds arrive as updates in the TestFlight app.

The team must be **your own** Apple Developer Program membership. Enroll at developer.apple.com/programs
($99/year, approval takes from a few hours to two days).

One-time setup:

1. **Register the App ID.** developer.apple.com → Certificates, IDs & Profiles → Identifiers → **+** → App IDs → App.
   - Bundle ID `com.kravtsov.kumowatch`.
   - Enable *Push Notifications* and *App Groups*.
   - Repeat for `com.kravtsov.kumowatch.widget` with *App Groups*.
   - Create the group `group.com.kravtsov.kumowatch` and assign it to both IDs.
   - The script can register these automatically, but doing it by hand avoids surprises.
2. **Create the app record.** App Store Connect (appstoreconnect.apple.com) → Apps → **+** → New App.
   - Platform iOS; bundle ID as above; SKU `kumowatch`.
   - Name: the store name must be unique across the App Store and must not look like someone else's trademark.
     If "Better Kumo" is taken, use another record name. The name under the icon stays "Better Kumo": it comes
     from the build, not from the record.
3. **Create an API key for uploads.** App Store Connect → Users and Access → Integrations → App Store Connect API
   → Team Keys → **+**, role *App Manager*.
   - Download `AuthKey_<KEYID>.p8`; it can be downloaded only once.
   - Note the *Issuer ID* shown above the key list.
4. **Write the local config.** Save the key as `~/.config/kumowatch/AuthKey_<KEYID>.p8` and create
   `~/.config/kumowatch/testflight.env`:

   ```sh
   KUMO_TEAM=<YOUR TEAM ID>
   ASC_KEY_ID=<KEYID>
   ASC_ISSUER_ID=<issuer uuid>
   ```

   Run `chmod 600` on both files.
5. **Add testers.** App Store Connect → the app → TestFlight → *Internal Testing* → **+** group → add yourself.
   - Internal testers must be users of your App Store Connect team; up to 100 of them.
   - Their builds need no Apple review.
   - External testers, by email or public link, need a short Beta App Review for the first build.

Every release:

```sh
scripts/build-ios-testflight.sh            # builds, signs for App Store, uploads
```

The script uses push notifications and the App Group, and a timestamp as the build number. After 5–30 minutes of
processing, the build shows up in TestFlight. On the iPhone: install **TestFlight** from the App Store, accept the
invite, tap *Install*. Later builds update the same app, and sign-in is kept.

TestFlight builds use the **production** APNs environment, so set `sandbox = false` in the relay's `[apns]` section
(§6.7). Builds installed with `build-ios-device.sh` use `sandbox = true`.

### 4.5 First launch on iPhone

1. Allow notifications.
2. Sign in (§5).
3. Allow microphone and camera when joining a call for the first time.
4. Link the relay (§6.6). Install ntfy for notifications (§6.7), unless APNs is set up.

## 5. Signing in on a phone

Two ways; use whichever is easier:

1. **Sign in with browser.** Opens the Kumospace web sign-in (Google works) and returns to the app.
2. **Sign-in link from a computer that is already signed in.** No passwords at all:

   ```sh
   cargo install --path core/crates/kumo-cli --locked     # once; installs kumo-cli to ~/.cargo/bin
   kumo-cli login --from-desktop                           # takes the session of Kumospace desktop (debug port 9222)
   kumo-cli transfer-token --link                          # prints kumospace://login?token=…
   ```

   Then send the link to the phone: paste it into the app's "Sign in with link" field, or open it on the phone.
   - The token is single-use and expires in a few minutes.
   - Treat it like a password.

## 6. Office Mac: Kumospace desktop + kumo-relay

Do this on the Mac where Kumospace desktop runs and where you are signed in.

### 6.0 The easy way: the Better Kumo desktop app

`Better Kumo.app` is a menu bar app that replaces every manual step in §6.1–6.4. One app does all of this:

- **Kumospace with phone control.** Starts Kumospace desktop with the debug port. If Kumospace was opened from the
  Dock without the port, it restarts it — automatically when it was just opened, otherwise from the menu.
- **Signs itself in.** Takes the sign-in from Kumospace desktop. No `kumo-cli` or `kumo-relay login` steps.
- **Runs the relay inside itself.** Desktop control, phone calls and notifications work without a separate launchd
  service.
- **Pairs a phone by QR code.** Menu → *Pair a phone…* opens a local page with one QR code per address (Tailscale and
  same Wi-Fi). Point the phone camera at it.
- **Menu bar status.** The menu shows Kumospace and phone status, and has these items:
  - Mirror notifications on/off;
  - Start at login;
  - Open log (`~/Library/Logs/kumowatch/better-kumo.log`);
  - Quit. If the phone holds the call, Quit gives the call back to the desktop first.

Build and install:

```sh
scripts/build-desktop-macos.sh          # → dist/Better Kumo.app and dist/Better-Kumo-macOS-<version>.zip
```

1. Copy `Better Kumo.app` to `/Applications` on the office Mac. Unzip the archive there or use AirDrop.
2. The app is signed only locally (no Apple Developer ID). On the first start, right-click it → **Open** → **Open**.
   Or run `xattr -dr com.apple.quarantine "/Applications/Better Kumo.app"`.
3. Start it. A person icon appears in the menu bar. Turn on *Start at login* in its menu.
4. *Pair a phone…* and scan the code with the phone.
5. If an older setup exists, remove it so two relays do not fight over port 8787:
   `launchctl bootout gui/$(id -u)/com.kumowatch.relay`. The app reuses `~/.config/kumowatch/relay.toml` and the
   relay data, so phones that were already paired keep working.

Settings stay where the CLI keeps them (`~/.config/kumowatch/relay.toml`). The ntfy topic and APNs keys from §6.7
still go there; restart the app after editing.

Windows and Linux: the same app builds from `core/crates/kumo-desktop` (`cargo build --release -p kumo-desktop`).
- **Kumospace location.** The app finds Kumospace in its standard install folder on Windows and as an AppImage on
  Linux. Set `KUMOSPACE_PATH` if Kumospace is elsewhere.
- **Status: not yet tested** on Windows or Linux. Linux needs the GTK and AppIndicator libraries for the tray icon.

### 6.1 Install the tools

On a dedicated office Mac without a build environment you only need Rust (`rustup`, §2). The relay does not need
Xcode or Android tools.

```sh
git clone <this repo> ~/kumowatch && cd ~/kumowatch
cargo install --path core/crates/kumo-cli --locked
```

### 6.2 Run Kumospace desktop with the debug port

The relay talks to the desktop app through the Chrome DevTools port 9222. It listens on localhost only.

- **Once, by hand:** quit Kumospace (⌘Q), then run:

  ```sh
  open -a Kumospace --args --remote-debugging-port=9222
  ```

- **Automatically at login** (recommended). This uses the Node launcher, so install `node` 20+ first:

  ```sh
  cd apps/watcher && npm ci
  KUMOWATCH_NODE=$(which node) scripts/install-launchd.sh --launcher-only
  node launcher.js launch          # now: (re)start Kumospace with the port
  ```

  From the next login on, Kumospace starts with the port by itself. If you start Kumospace from the Dock instead, it
  runs without the port, and the Desktop screen shows "Kumospace desktop is not connected"; run
  `node launcher.js launch` again.

Check it: `curl -s http://127.0.0.1:9222/json/version` prints JSON.

Security note: any program on this Mac can use the debug port to act as your Kumospace session. It is not reachable
from the network.

The old Node watcher service (`com.kumowatch.service`, ntfy rules) is superseded by kumo-relay. Running both sends
duplicate notifications, so install the launcher only, as shown above.

### 6.3 Configure and sign in the relay

```sh
kumo-cli spaces                                     # find your space name (e.g. "shiftforex")
kumo-relay init --space <space> --ntfy-topic kumo-$(openssl rand -hex 8)   # ntfy optional, see §6.7
kumo-cli login --from-desktop
kumo-cli transfer-token | kumo-relay login --token-stdin
```

- **Config:** `init` writes `~/.config/kumowatch/relay.toml` with mode 600. It holds the listen address
  (`0.0.0.0:8787`), a random `secret` that the phone uses, the space, ntfy and APNs settings, and `desktop_cdp`.
- **Session:** `login` stores the relay's own Kumospace session in `~/.local/share/kumowatch-relay/`.
- **First time only:** `kumo-relay` exists after §6.4. The first time, run these commands as
  `cargo run --release -p kumo-relay -- …` from `core/`, or run §6.4 first and then come back.

### 6.4 Install as a background service

```sh
scripts/install-relay.sh
```

This builds the release binary into `~/.local/bin/kumo-relay` and installs the launchd agent `com.kumowatch.relay`.
The agent starts at login and restarts if it crashes. Logs go to `~/Library/Logs/kumowatch/relay.log`.

```sh
kumo-relay status                                   # signed in? devices? mirroring on?
tail -f ~/Library/Logs/kumowatch/relay.log
launchctl kickstart -k gui/$(id -u)/com.kumowatch.relay    # restart
launchctl bootout gui/$(id -u)/com.kumowatch.relay         # stop and disable
```

Add `~/.local/bin` to `PATH` if `kumo-relay` is not found.

Keep the Mac awake while you rely on it: System Settings → Energy → *Prevent automatic sleeping when the display is
off*. Or run `caffeinate -s` in a terminal. A sleeping Mac means no relay and no desktop presence.

### 6.5 Make the relay reachable from the phone

**Tailscale** (recommended; works on cellular and any Wi-Fi):

1. Install Tailscale on the Mac (`brew install --cask tailscale` or the App Store) and on each phone (App Store /
   Google Play). Sign in to the same account everywhere.
2. Get the Mac's address with `tailscale ip -4` (`100.x.y.z`) or use its MagicDNS name (`<mac-name>.<tailnet>.ts.net`).
3. The relay URL is `http://<that address>:8787`.

**Same Wi-Fi only:** use the Mac's LAN address, `ipconfig getifaddr en0`, as in `http://192.168.1.20:8787`.
- Give the Mac a fixed address in the router, or the link breaks when the address changes.
- The macOS firewall may ask to allow incoming connections for `kumo-relay`: allow it.

Test from the phone's browser: `http://<address>:8787/v1/status` must answer `401`, meaning reachable but no secret
yet.

### 6.6 Link the phone to the relay

On the office Mac:

```sh
kumo-relay pair --url http://<address>:8787
```

It prints `kumowatch://relay?url=…&secret=…`.
- **Open the link on the phone.** Send it to yourself in a private chat and tap it. Or paste it into Profile →
  Notifications relay.
- **Keep it secret:** it contains the relay secret.

After linking:

- **Office tab → Desktop** shows the live floor map of the desktop app.
  - Tap the map to move.
  - Choose a room or floor; follow people; set Available / Away / Focusing and a status message.
  - Switch the desktop's mic and camera; nudge people; tap the house button to go home.
  - Join the desktop's call to hear and see your room.
- **Android:** the widget turns mirrored notifications on and off.
- **iPhone:** Profile → Notifications relay → toggle, plus §6.7.

### 6.7 iPhone notifications

**ntfy** (free, no Apple account needed):

1. Install **ntfy** from the App Store.
2. In ntfy, tap **+** and subscribe to the topic from `relay.toml` (`[ntfy] topic`) on server `https://ntfy.sh`.
3. Allow notifications for ntfy. Tapping a notification opens the chat in Better Kumo.
4. Check it: `curl -X POST -H "Authorization: Bearer <secret>" http://127.0.0.1:8787/v1/test` sends a test push.

Anyone who knows the topic name can read it, so keep it random (as `init` above does). Messages carry the sender
and a short preview.

**APNs** (paid team; native notifications, no ntfy app):

1. Go to developer.apple.com → Certificates, IDs & Profiles → **Keys** → **+** → enable *Apple Push Notifications
   service* → download `AuthKey_<KEYID>.p8`. It can be downloaded only once.
2. Put it on the office Mac, e.g. `~/.config/kumowatch/AuthKey_<KEYID>.p8`, and set `chmod 600` on it.
3. Add to `~/.config/kumowatch/relay.toml`:

   ```toml
   [apns]
   team_id = "<TEAM ID>"
   key_id = "<KEY ID>"
   key_path = "/Users/<you>/.config/kumowatch/AuthKey_<KEY ID>.p8"
   topic = "com.kravtsov.kumowatch"       # the bundle id used in §4.3
   sandbox = true                          # true for Xcode/script builds, false for TestFlight/App Store
   ```

4. Restart the relay (`launchctl kickstart -k …`). Install the app with `--full` (§4.3) or from TestFlight (§4.4), open it, and link the relay.
   The app registers its push token with the relay automatically.

## 7. Updating

| What changed | Run |
|---|---|
| Rust core or Android code | `git pull && scripts/build-android-release.sh --install` (or send the new APK) |
| Rust core or iOS code | `git pull && scripts/build-ios-device.sh --team <TEAM_ID> [--full]` |
| Relay code | `git pull && scripts/install-relay.sh` (on the office Mac) |
| iOS via TestFlight | `git pull && scripts/build-ios-testflight.sh`, then update in the TestFlight app |
| Free iOS team, 7 days passed | `scripts/build-ios-device.sh --team <TEAM_ID> --skip-core` |

Updates keep sign-in, chat cache and settings on all platforms.

## 8. Uninstalling

- **Android:** long-press the app → Uninstall, or run `adb uninstall com.kravtsov.kumowatch`.
- **iPhone:** long-press the app → Remove App.
- **Office Mac:**

  ```sh
  launchctl bootout gui/$(id -u)/com.kumowatch.relay; rm ~/Library/LaunchAgents/com.kumowatch.relay.plist
  rm ~/Library/LaunchAgents/com.kumowatch.kumospace.plist
  rm -rf ~/.local/bin/kumo-relay ~/.local/share/kumowatch-relay ~/.config/kumowatch/relay.toml
  ```

  Keep `~/.config/kumowatch/android-release.jks` if you will build Android updates again.

## 9. Troubleshooting

| Symptom | Cause / fix |
|---|---|
| Android: "App not installed" / "package conflicts" | The installed copy is signed with another key (e.g. an old debug build with the same ID). Uninstall it once, then install. |
| Android: no notifications in background | Battery optimization (§3.3 step 4); notification permission; widget off. |
| iPhone: "Untrusted Developer" | Settings → General → VPN & Device Management → Trust. |
| iPhone: app stops opening after a week | Free team expiry, re-run the iOS script. |
| `build-ios-device.sh`: "No profiles for … were found" / "bundle identifier is not available" | Sign in to Xcode → Settings → Accounts. Use `--bundle-id com.<yourname>.kumowatch`. |
| `build-ios-device.sh`: "No iPhone found" | Connect and unlock the phone; trust the Mac; check `xcrun devicectl list devices`. |
| Desktop screen: "Kumospace desktop is not connected" | Kumospace is not running with `--remote-debugging-port=9222` (§6.2), or `desktop_cdp` in `relay.toml` is wrong. |
| Desktop screen: connection timeout / "failed to connect" | Wrong URL or the Mac is asleep, off Tailscale, or the firewall blocks it. Open `http://<address>:8787/v1/status` on the phone. |
| Desktop screen: "Wrong relay secret" | The secret changed (new `init`). Run `kumo-relay pair` again and open the new link. |
| Call from phone: "exp-token" / drops after a while | Fixed automatically: the app asks the desktop for a fresh Daily token. If it repeats, restart Kumospace desktop. |
| Relay log: "not signed in" / 401 from Kumospace | `kumo-cli login --from-desktop && kumo-cli transfer-token \| kumo-relay login --token-stdin`, then restart the relay. |
| Sign-in link says expired | Tokens live a few minutes and work once. Create a new one. |
| Duplicate notifications | The old Node watcher service still runs: `launchctl bootout gui/$(id -u)/com.kumowatch.service`. Also check that only one relay or one phone mirroring is on. |

## 10. Files and secrets

| File | Contents | Where |
|---|---|---|
| `~/.config/kumowatch/android-release.jks`, `android-signing.properties` | Android signing key and its password | build machine; **back up** |
| `~/.config/kumowatch/relay.toml` | relay secret, ntfy topic, APNs config | office Mac |
| `~/.config/kumowatch/AuthKey_*.p8` | APNs key (office Mac); App Store Connect upload key (build machine) | office Mac / build machine |
| `~/.config/kumowatch/testflight.env` | team ID and App Store Connect key IDs | build machine |
| `~/.local/share/kumowatch-relay/` | relay's Kumospace session, registered devices | office Mac |
| `~/.config/kumowatch/session.json` | kumo-cli session | wherever kumo-cli runs |
| Phone | session in Keystore (Android) / Keychain (iOS); relay URL and secret | phone |

None of these are in git. Never commit them or paste them into chats.
