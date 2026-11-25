# Roadmap: native Kumospace client

Goal: feature parity with the official Kumospace mobile app (Android 202610.24.411), with chat that is
local-first and instant. Every feature is listed with where its logic lives (R = Rust core, N = native UI/SDK).
References: `kumospace-features.md` (what the official app does), `kumospace-api.md` (wire protocol).

Each milestone ends with something usable on both platforms.

## Requirements from hands-on review of the official app

Collected while using the official Android app (`kumo_play36` emulator, real account). IDs are referenced from the
milestones below.

### Chat

- **CH-1. Opening a channel is instant and stable.** Tapping a chat opens it immediately with content from the local
  cache; no glitchy transition. Official app: the screen glitches and stays half-loaded for a while.
- **CH-2. The composer is not usable before the chat has loaded.** Official app: the keyboard can be opened and text
  typed while messages are still loading. Ours: the composer appears together with the messages (cache makes this
  instant), or typing during a sync must never lose text or jump the list.
- **CH-3. A channel opens at the newest message.** No manual scrolling to the bottom to see the latest messages,
  ever. If there are unread messages, open at the first unread with a "jump to latest" button.
- **CH-4. No scroll-driven batch loading of recent history.** Official app: the user scrolls to the end and waits for
  several small batches to load each time. Ours: recent history is already in the local store; loading must not
  depend on scrolling.
- **CH-5. Background sync.** Latest messages of recent channels are fetched in the background (on app start, on
  reconnect, on push), so a channel is up to date before it is opened. If that is impossible for a channel, load its
  recent history in one go (large page, or parallel pages) instead of small batches.
- **CH-6. Older history may page in while scrolling up, but prefetched ahead** of the viewport so the user never
  waits at the top.

## Progress (2026-11-11)

Working end to end on the Android emulator and the iOS simulator against the real backend:

- Sign-in by session transfer (`kumospace://login?token=…` minted by `POST /v1/account/desktop/token`) and by the
  browser hand-off flow (Android: system browser + intent filter; iOS: `ASWebAuthenticationSession`). Session kept in
  Android Keystore / iOS Keychain, restored on launch.
- Default space picked automatically after sign-in; space picker and switching.
- Chat: Rust `ChatEngine` (`kumo-core::chat`) with SQLite cache, Sendbird socket + REST, background prefetch of the 40
  most recent channels plus all unread ones, gap catch-up on open, live messages / edits / deletes / typing / read
  state, unread and mention counters, send with optimistic insert and retry, reconnect with backoff.
  Measured: cached channel list + 50 messages read in ~3–6 ms; warm sync online in ~1.6 s; cold sync with prefetch ~4.4 s.
- UI on both platforms: chat list with filters (All / Unread / Channels / Direct), channel screen that opens at the
  newest message from cache, prefetches older pages before the top is reached, day separators, markdown (bold, italic,
  strike, code, links, GIF/image embeds), mentions, reactions (read-only), reply counts, authenticated Sendbird
  file images, typing indicator, composer disabled until the list is ready, "jump to latest".

- Avatars come from the Kumospace member directory (`userRoles.avatarUrl`, real uploaded photos) with the Sendbird
  `profile_url` as fallback; Sendbird often has only Google's default letter avatar or nothing.
- Android "Mirror notifications": a foreground service keeps the chat and the space socket open and posts normal
  system notifications for DMs, group DMs, mentions, nudges and door knocks. Toggled from the profile screen or a
  home-screen widget (2×1, pinned from the app). While on, the user is also present in the space as a mobile client.
  iOS cannot keep sockets in the background and cannot receive Kumospace's Sendbird pushes, so it has no real-time
  equivalent (see M4).

- Office and calls (Android): "Office" tab with floors, rooms and people (live census), search, join a room or a
  person; call screen on the Daily Android SDK 0.41 with Speaker / Screen / Grid layouts, active-speaker highlight,
  mic and camera toggles (both start off), leave; foreground service while in a call. Same on iOS (Daily iOS SDK
  0.41, SwiftUI, background audio mode). "Speaking" highlight comes from Daily audio levels of the participants we
  subscribe to; Daily's own `active-speaker-changed` is floor-wide and is ignored. Password-protected rooms prompt for
  the password; closed or full rooms show a message.

- Chat actions on both platforms: long-press / context menu with quick reactions (recently used first), a full emoji
  picker by category with search by English, Ukrainian and Russian names, reply in thread, copy, edit and
  delete own messages (with Kumospace's soft delete for thread parents); tapping a reaction chip toggles it; thread
  screen; image upload (resized to JPEG ≤1600 px); `@mention` autocomplete from channel members. Offline deletions and
  edits are applied from message changelogs on catch-up.

- Channel member sheet (tap the title; online first; tap a person to open the DM), "new message" picker that opens or
  creates a DM (`POST /v1/chat/channel`), "New messages" divider at the first unread message, full-screen image viewer
  with zoom, animated GIFs on iOS.
- Notification relay (`kumo-relay`) with ntfy and APNs delivery, HTTP API, launchd installer and phone pairing link;
  iOS relay settings (toggle, test, device registration) and an iOS home-screen widget toggle (WidgetKit + App Group).

- Desktop bridge in `kumo-relay` (CDP to the desktop app): nudges and knocks mirrored as notifications, and a
  "Desktop" screen on both phones: floor map with rooms and people, tap to walk or enter a room, go to a person,
  change floor, status, desktop mic/camera, nudge. Reading verified against the live desktop app; commands not yet
  exercised (they move the user's real avatar).

Not done yet: ghost-DM cleanup, emoji picker beyond the quick set. Blocked on an Apple Developer team: APNs delivery
(the `aps-environment` entitlement) and the iOS widget (App Group entitlement); both are coded and build.

## M0. Foundation (done)

- [x] Monorepo: `apps/watcher`, `apps/ios`, `apps/android`, `core/`.
- [x] Rust core + UniFFI bindings for Swift and Kotlin; both apps build and call Rust.
- [x] Protocol models: REST endpoints, WS ops (`census`, `entities`, `spaceZones`, `room`, ...), channel URL scheme.
- [x] `kumo-cli` crate: a desktop CLI over the core (`login --from-desktop`, `transfer-token`, `spaces`,
      `chat-config`, `members`, `tail-space`) to test against the real backend without an emulator.
- [ ] CI: `cargo test`, iOS simulator build, Android `assembleDebug`.
- [x] Spike: Sendbird protocol from Rust. Connect with `chatConfig` credentials, list channels, load messages, receive
      `MESG` live. This decides whether chat stays in Rust (preferred) or falls back to the native Sendbird SDKs.
      Result: works, chat stays in Rust (`kumo-core::sendbird`, `docs/sendbird-protocol.md` §0).

## M1. Auth

Native Google Sign-In cannot work for a third-party app (see `kumospace-api.md` §7.1). Sign-in goes through
Kumospace's own desktop hand-off instead, which covers every provider the web supports.

| Feature | Logic |
|---|---|
| Browser sign-in: open `https://www.kumospace.com/auth/redirect/desktop` in the system browser (Custom Tab / `ASWebAuthenticationSession`), receive `kumospace://login?token=…`, `signInWithCustomToken` | N + R |
| Session transfer from another signed-in device: `POST /v1/account/desktop/token` → QR code / link → `signInWithCustomToken` | R + N |
| Email + password sign-in (Firebase `signInWithPassword`) for accounts that have a password | R |
| Reset password: `POST /v1/account/resetPassword` | R |
| Session restore + id-token refresh (`securetoken`), secure storage | R + N |
| Sign out: close sockets, wipe caches and tokens | R |

## M2. Spaces ("You" tab)

| Feature | Logic |
|---|---|
| Profile header: photo and name | N |
| My spaces: `GET /v1/spaces?members=true&excludeDemo=true` | R |
| Default space on first launch: `GET /v1/users/me/defaultSpace` | R |
| Unread badge per space (computed from local chat store, not one REST call per space) | R |
| Select space (persisted), switch space | R + N |

## M3. Chat (main value)

| Feature | Logic |
|---|---|
| Sendbird connect via `GET /v1/spaces/{space}/chatConfig` (`userId`, `sessionToken`) | R |
| SQLite cache for channels, members, messages, reactions, sync tokens | R |
| Incremental sync with channel/message changelogs, gap filling after offline | R |
| Background prefetch of latest messages for recent channels (CH-4, CH-5) | R |
| Chat list: Nearby/Public session rows, Channels (`custom`), Groups (`group_dms`), Online / Offline people (DMs) | R + N |
| Unread and mention counters, live from the socket | R |
| Ghost-DM cleanup (mark DMs from foreign spaces as read) | R |
| Open DM; create it when missing: `POST /v1/chat/channel` | R |
| Message list: instant from cache, opens at newest / first unread, prefetched paging both directions, date separators, first-unread marker (CH-1, CH-3, CH-6) | R + N |
| Composer enabled only when the list is ready; draft survives syncs (CH-2) | N |
| Send text with optimistic insert, outgoing queue, retry | R |
| Markdown rendering (bold, italic, strike, code, code blocks, links): parsed in Rust (`chat::markdown`), styled natively | R + N |
| Mentions: `@user`, `@here`, `@channel` with autocomplete | R + N |
| Images: pick, resize to JPEG, upload; full-screen zoom viewer; other files listed | R + N |
| Reactions: quick set 👍 🙌 😂 ❤️ 🔥 + emoji picker, recently used | R + N |
| Edit and delete own messages (file messages: `DELETE /v1/chat/channel/{url}/file-messages/{id}`) | R |
| Threads: reply counts, thread screen, per-thread unread | R + N |
| Typing indicators (send and show) | R + N |
| Mark as read on open and on new messages | R |
| Channel details: title, privacy, member list sorted online first | R + N |
| Frozen channel state ("Chat Closed") | R + N |
| Free-tier upsell screen | N |

## M4. Push notifications

Constraint: Sendbird delivers pushes through the FCM/APNs credentials configured in Kumospace's Sendbird app, so a
token from our own app cannot receive them. Decision (2026-11-11): option (b), our own relay. Android mirrors through
its foreground service; iOS through `kumo-relay` (ntfy now, APNs once an Apple Developer key exists).

| Feature | Logic |
|---|---|
| Register APNs / FCM token with Sendbird (`/v3/users/{id}/push/...`) | R + N |
| Show notifications in background; none in foreground for the open channel | N |
| Tap → open channel (and thread when present) | R + N |
| Background wake-up runs a chat sync, so the app opens on fresh data | R + N |
| Clear badge on resume | N |
| No ntfy for the chat open on the phone: temporary `PUT /v1/viewing` hint to the Relay, removed once Apple grants the notification filtering entitlement and the phone filters every push itself | R + N |

## M5. Space / office ("Space" tab)

| Feature | Logic |
|---|---|
| WS connect `wss://ws.kumospace.com/{space}?timeZone&idToken&platform[&reconnectAs]` | R |
| Reconnect state machine (2 s backoff, waits for network, close 4000 = banned) | R |
| Census / entities / zones store and office model: Lobby, floors, occupied rooms, Common Area, "More Rooms" | R |
| People search | R + N |
| Availability dots, mobile-user icon, "(you)" | N |
| Teleport to room / user (`join`), guards for lobby and common area | R |
| Knock / waiting room (`ZONE_CLOSED`), room password prompt (`ROOM_PASSWORD_REQUIRED`) | R + N |
| Server error toasts | N |

## M6. Session chat

| Feature | Logic |
|---|---|
| Nearby and Public chat over WS (`chat` op, `notification` type `chat`) | R |
| Unread per range, message sound, open from call screen | R + N |

## M7. Calls (Daily)

| Feature | Logic |
|---|---|
| Daily credentials from WS `room` frame; `refreshDailyToken` with backoff on join failure | R |
| Join / leave call (leave = WS `join {}`) | R + N |
| Participants grid: video or avatar, talking ring, mic-off, name tag, emote overlay | N |
| Mic / camera toggles, permission alerts | N |
| Hide all videos | N |
| Emotes (`emote` op) | R + N |
| Screen-share viewing: zoom, presenter switch | N |
| Recording indicator + "X started recording" (`GET /v1/recordings/{id}`) | R + N |
| Host mute | R + N |
| In-app PiP over other screens; system PiP | N |
| Audio route priority (wired > speaker > bluetooth); keep alive in background (CallKit / foreground service) | N |

## M8. Polish

- Tablet split view for chat with a resizable divider.
- Deep links `kumospace://chat/channel?channelUrl=...`.
- Offline banner and sync status.

## Beyond parity (later)

- Set own status (available / away / focusing + message). The mobile app cannot; the payload of `setUserStatus` /
  `updateUser` can be captured from the desktop app with the watcher's `KUMO_DEBUG_LOG`.
- Watcher rules on the phone (who approached, mention alerts) driven by the same Rust core.
- Message search.
