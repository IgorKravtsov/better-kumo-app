# Kumospace mobile — user-facing feature inventory

Source: `decompiled.js` (hermes-dec of `index.android.bundle`, app version 202610.24.411, build 411), `x/base/assets/app.config`, `x/base/res/raw/assets_webviews_index.html`.
`L123` = line in decompiled.js. `mod N` = metro module id. "(inferred)" means I read it from structure or naming, not from a literal I traced.


---

## 0. Stack and global providers

- Expo Router 58 (route context map at L216823, mod 610). React Native 0.88. Native tabs (`NativeTabs` from expo-router, mod 2513).
- State: Redux Toolkit store (mod 5021). Slices: `firebase`, `chat`, `sessionChat`, `daily`, `debug`, `callPiP`, `recording`, `webSocket`. Server data goes through React Query (`useKumoApiQuery` and friends, mod 1567). The React Query cache is **not persisted**: there is no `persistQueryClient` in the bundle.
- Auth: Firebase Auth with RN persistence (mod 1554/1571).
- Chat: Sendbird Chat SDK 4.22.14 + UIKit RN 3.12.10 (`SendbirdUIKitContainer`, mod 4856).
- Calls: Daily (`@daily-co/react-native-daily-js`), with `DailyCallProvider` (mod 4850) and `DailyCallManager` (mod 4846).
- Realtime office: one Kumospace WebSocket (`wss://ws.kumospace.com`), driven by an XState machine in `WebSocketProvider` (mod 4915).
- Env (mod 1569, L499544): `EXPO_PUBLIC_API_URL=https://api.kumospace.com`, `EXPO_PUBLIC_WS_URL=wss://ws.kumospace.com`, Sentry DSN, Statsig client key, Segment write key, Sendbird app id `CA71E01B-…`, Google web client id.
- RootLayout (mod 4997) wraps the app in: Sentry (mod 5022, `tracesSampler`, native frames), Segment `AnalyticsProvider` (mod 2167/2170, `trackAppLifecycleEvents`), Redux `Provider`, `QueryClientProvider`, Statsig `FeatureFlagProvider` (mod 5086), `ScreenOrientationProvider` (mod 5087), `RootSiblingParent` (toasts, mod 2074), `TrackScreen` (mod 5084), `DebugOverlay` (mod 5085), `SplashScreen` (mod 5088: animated gif `assets/splash/animated.gif` until Firebase, fonts and Statsig are ready).
- AppLayout `(app)/_layout.tsx` (mod 4841, L1685460) mounts, once the user is authed: `AuthGuard` (mod 4845), `usePushNotificationHandler` (4842), `useRecentlyUsedEmojiPersistance` (4843), `useRefreshUnreadMessagesOnForegrounded` (4844), `DailyCallManager` (4846), `DailyCallProvider` (4850), `CallPiP` (4851), `RecordingManager` (4853), `SendbirdManager`/`SendbirdContainer` (4856), `WebSocketManager` (4913), `WebSocketProvider` (4915), `TeleporterManager` (4963).
- Fonts: Akkurat (Regular/Bold/Italic/BoldItalic), Sharp-Grotesk-Medium, Inconsolata-Regular (L797183–797288). Brand colours: forest `#213433`, kumo-green `#92db7b`.

## 1. Navigation map

### 1.1 Route tree (L216823)
```
_layout.tsx                RootLayout (providers, splash)
+native-intent.tsx         redirectSystemPath
+not-found.tsx             UnmatchedRoute (Sentry breadcrumb "Unmatched Route Loaded", redirects to "/")
(auth)/_layout.tsx         AuthLayout: AuthGuard whenAuthed→redirectToApp
  sign-in/_layout.ts       SignInLayout: Stack, slide_from_right, back title "Back"
  sign-in/index.ts         SignInScreen
  sign-in/password         PasswordSignInScreen
  sign-in/reset-password   ResetPasswordScreen
(app)/_layout.tsx          AppLayout: AuthGuard whenUnauthed→/sign-in, plus all managers listed in section 0
  index.ts                 HomeScreen: redirect
  (core)/_layout.ts        HomeLayout = bottom NativeTabs
    chat/_layout.ts        ChatLayout (Stack, slide_from_right; split-pane on wide screens)
      chat/index           ChatScreen (list) / ChatIndexScreen (empty detail pane)
      chat/channel         ChatChannelScreen          ?channelUrl= | ?dmUserId=
      chat/channel/details ChatChannelDetails         ?channelUrl=
      chat/session         SessionChatChannel         ?range=nearby|broadcast
      chat/thread/_layout  ChatThreadLayout (marks thread read)
      chat/thread          ChatThreadScreen           ?channelUrl=&messageId=
    profile/index          ProfileScreen ("You" tab; analytics name "Spaces")
    space/index            SpacesScreen ("Space" tab; analytics name "People")
  (call)/call/_layout.ts   CallLayout / CallRenderer
  (call)/call/index.ts     CallScreen
```
Route module ids: call/_layout 611→612, call/index 2237→2238, (core)/_layout 2418→2419, chat/_layout 2529→2530, details 4131→4132, channel 4133→4134, chat/index 4810→4119, session 4811→4812, thread/_layout 4816→4817, thread 4820→4821, profile 4823→4824, space 4832→4833, (app)/index 4964→4965, sign-in 4969→4970, password 4987→4988, reset 4992→4993.

### 1.2 Bottom tabs (HomeLayout, mod 2419, L960260–960420)
| Order | Route | Label | Icon | Condition |
|---|---|---|---|---|
| 1 | `chat` | **Chat** | `assets/tabs/chat.png` / `chat-filled.png` (24×24 template, L992263/992284) | `hidden` unless the Statsig gate `chat_mobile` is on; `disabled` while no `selected-space` |
| 2 | `space/index` | **Space** | `assets/tabs/space.png` / `space-filled.png` (L992305/992326) | `disabled` while no `selected-space` |
| 3 | `profile/index` | **You** | SF `person.crop.circle` / `.fill`; Material `account_circle` | always |

Tint and icon colour are `colorPrimaryForest`. No tab badge is set (no `badge` prop anywhere in HomeLayout).

### 1.3 Launch flow
1. The splash stays up until Firebase auth, fonts and Statsig are ready (mod 5088).
2. `AuthGuard` (mod 4845): Firebase status `loading` shows nothing, `registered` goes to `(app)`, otherwise `/sign-in`.
3. `HomeScreen` (mod 4965, L1726393): reads the AsyncStorage key `selected-space`. If set it redirects to `/chat`, otherwise to `/profile` (the space picker).
4. `ProfileScreen` calls `useSelectDefaultSpace` (mod 4830). If no space is selected, it fetches `users/me/defaultSpace` (mod 4831) and auto-selects it. `useRecentlyVisitedSpaces` (`users/me/recentSpaces`) is defined but never called.
5. Selecting a space (`useSelectSpace`, mod 4829) writes `selected-space`, tracks "Space Joined" and runs `router.replace('/space')`.

### 1.4 Deep links
- Scheme `kumospace://` (app.config `scheme`). Expo also registers the Android package scheme (inferred).
- `+native-intent.redirectSystemPath` (mod 4995, L1731190): the regex `^(kumospace:\/\/|\/)login([/?#]|$)` rewrites any `login` link to `/`. Every other path passes through unchanged, so `kumospace://chat/channel?channelUrl=…` maps straight onto routes (inferred).
- **No universal links.** There is no `associatedDomains` in the iOS config and no Android `intentFilters` with `autoVerify` in app.config.
- Push-tap deep routing is handled in JS (section 3.1), not through URLs.

### 1.5 Modals, sheets and overlays
- Message `ActionsSheet` (@gorhom/bottom-sheet, mod 4187/4795): dynamic sizing, pan-down to close.
- Emoji keyboard (`rn-emoji-keyboard`, mod 2316/2360): "recently used" category first, search bar on. Used for reactions and in-call emotes.
- Fullscreen image viewer: an RN `Modal` with slide animation and a zoomable view, swipe-down to dismiss (mod 4797).
- `PasswordPrompt` (mod 2232) and `WaitingRoom` (mod 2236), rendered inside the call route.
- In-app call PiP: a draggable floating tile (`CallPiP`, mod 4851) shown over other screens while in a call.
- Toasts (`react-native-root-toast`, mod 2072) for teleport errors, WS errors and recording start/stop.
- Native `Alert` for missing camera/mic permission, with "Open Settings" and "Cancel" (L934033).
- `DebugOverlay`, gated by the Statsig gate `mobile_debug`.

## 2. Screens

### 2.1 Sign-in (`(auth)/sign-in`, mod 4970)
- Logo (SVG, mod 4971).
- `OAuthSignInButton provider="google"`: "Sign in with Google" (`@react-native-google-signin`, then Firebase Google credential).
- `OAuthSignInButton provider="microsoft"`: "Sign in with Microsoft" (Windows icon; Firebase OAuth provider `microsoft.com`).
- `PasswordSignInButton` (mod 4986, L1730035): "Sign in with Password" (Mail icon), which pushes `/sign-in/password`.
- **Apple:** `useSignInWithOAuth` (mod 4973) does implement `signInWithApple` (`apple.com`, nonce, `fullName`; error "Apple sign-in returned no identity token", L1727179), and the iOS entitlement exists. However, `SignInScreen` only renders the google and microsoft providers. Apple is effectively hidden or dead code (inferred).
- Not present: magic link, SSO/SAML, sign-up.
- Errors are mapped by `handleFirebaseError` / `withFirebaseError` (mod 4984).

### 2.2 Password sign-in (`sign-in/password`, mod 4988)
- Form (react-hook-form + zod): **Email** and **Password** (secure). Validation message "Password is required".
- Buttons: **Sign In** (`useSignInWithPassword`, mod 4991, Firebase email/password) and **Forgot Password?** (ghost), which pushes `/sign-in/reset-password?email=`.
- On return from reset it shows "Instructions to reset your password was sent to {email}" (L1730209), driven by the `resetEmail` param.

### 2.3 Reset password (`sign-in/reset-password`, mod 4993)
- Copy: "Get instructions sent to your email to reset your password".
- **Email** input and a **Send** button. `useResetPassword` makes a `POST account/resetPassword` (mod 4994).
- Error: "We couldn’t send the reset email. Please try again."
- On success it navigates to `/sign-in/password` with `resetEmail`.

### 2.4 "You" tab: profile and space picker (`profile/index`, mod 4824, L1683050)
- Header "You" (DisplayMedium), `ToggleDebugOverlay` ("Debug - On/Off", only when `mobile_debug` is on), and **Sign Out** (destructive ghost). `useSignOut` (mod 4825) disconnects Sendbird, closes the WS, clears AsyncStorage keys and signs out of Firebase.
- Self row: the Firebase `photoURL` (48×48, radius 12) and `displayName`.
- Label "Join Space" (or "Loading..."), then a `FlatList` of `SpaceCard`s (mod 4828) built from `useSelfSpaces` (`GET spaces?members=true&excludeDemo=true`).
  - `SpaceCard`: a check icon if it is the selected space, the space `displayName` (bold), "kumospace.com/{name}" (grey), and an unread `Badge`. The badge is the sum of `unread_message_count` over `useSpaceChatConfig(space.name).channels`, which is **one chatConfig request per listed space**.
  - Tapping a card runs `useSelectSpace`, which saves `selected-space`, tracks "Space Joined" and goes to `/space`.
- **Not present on mobile:** editing your availability/status (available/away/focusing), a custom status message, changing your avatar or name, notification settings, delete account. Availability is only *displayed* (`AvailabilityBadge`, mod 4128: `available` / `away` / `focusing` / `offline`).

### 2.5 "Space" tab: office view (`space/index`, `SpacesScreen`, mod 4833)
- `KeyboardAvoidingView` + `ScrollView` (keyboard dismisses on drag), with a search bar at the bottom.
- `SearchInput` (mod 4839): placeholder "Search", accessibility label "People Search". It filters people through `useFilteredFloors(query)`.
- Data comes from the Kumospace WS. `webSocket` slice (mod 2102): `space` (config), `censusUsers`, `entities`, `rooms`, `teleporter`. Selectors are in mod 1292: `useFilteredFloors`, `sortFloorWithRooms`, `useUserCountForFloor`, `useUserIdsForRoom`, `useFilteredUserIdsForLobby`, `useFilteredUserIdsForCommonArea`, `useSelfLocation`, `useIsInLobby`, `useUserAvailabilityByFirebaseUid`, `useIsFreeSpace` (`subscription-tier` key).
- **LobbyAccordion** (mod 4838): title "Lobby" with a user count, then a list of `UserItem`s.
- **FloorAccordion** per floor (mod 4834): `FloorTitle` "{floor name} • {count}" (mod 4835).
  - Occupied rooms are listed as `RoomRow`s, each with its users.
  - A "Common Area" section lists users on the floor but not in a room.
  - A nested collapsed accordion, Door icon + **"More Rooms"** (L1684719), holds the remaining empty rooms (inferred).
- **RoomRow** (mod 4836): a `Door` / `OpenDoor` icon (locked/closed vs open zone status), the room name (bold), and a People icon with a count. Tap calls `teleport({type: zone, zone})`.
- **UserItem** (mod 4837): `ProfileImage` (xsmall, with availability dot), the name plus " (you)", and a `Mobile` icon if the user is on a mobile platform. Tap calls `teleport({type: user, user})`.
- **Teleport / Join** (`useTeleporter`, mod 2071, L749871):
  - Sends WS `{op:'join', data:{roomName: floorId|user.roomName, teleportTarget:{type:'zone',zoneId}|{type:'user',userId}}}`.
  - Dispatches `startTeleport` and pushes `/call`.
  - Guards (toasts): "Can't join users on lobby" (L749924) and "Can't join common area" (L749941).
  - Also exposes `retryTeleport`, `cancelTeleport` (reset, then `router.replace('/space')`), `knockWaitingRoom` and `submitPassword`.
- **TeleporterManager** (mod 4963):
  - Watches for arrival and tracks "Teleport" (`targetUserId`, `targetZoneId`).
  - Handles the WS error `zoneClosed`: enters the waiting room and tracks "Entry to Locked Room Requested" (L1726251).
  - Handles the WS error `roomPasswordRequired`: shows the password prompt.
- **WebSocketManager** (mod 4913) only tracks the analytics events "Range Entered" / "Range Exited" when users join or leave your room.
- Recording and screen-share state per user appear only inside the call view (section 2.6). Nothing in the space list renders them.

### 2.6 Call (`(call)/call`, `CallRenderer` + `CallLayout` mod 612, `CallScreen` mod 2238)
`CallRenderer` picks a state from `useTeleporterTarget`, `useIsInWaitingRoom`, `usePasswordPrompt`, `useIsDailyCallLoading` and WS status:

- **Loading**: centred spinner ("Loading..." `LoadingText`, mod 2148).
- **Waiting room** (knock, mod 2236): "Waiting for someone to open the door." (L838874), with **Knock** and **Cancel** buttons.
- **Password prompt** (mod 2232): Lock icon, "This Floor requires a password", "Enter password", **Join** and **Cancel**.
- **In call**: Android hardware back is intercepted (inferred: it goes to PiP or leaves). The status bar is light. The layout switches to a row when a screen share is fullscreen in landscape.
  - `CallHeader` (mod 2106):
    - `RoomName`.
    - Participants: "{n} people" / "1 person".
    - **Hide/show videos** toggle (`EyeOpen`/`EyeClosed`, which sets `daily.videosDisabled`).
    - **LeaveCall** (`Exit` icon). It sends WS `op:'join'` back to the lobby, then `router.replace('/space')`.
  - `CallParticipants` (mod 2379): renderers for single column, multi-column, wide grid (tablet), screen-share strip and PiP list.
    - Each `Avatar` (mod 2380) shows the Daily video (`DailyParticipantVideo`, mirrored for local) or the profile image / initials.
    - Overlays: talking ring (audio level threshold, `useIsParticipantTalking`), `MicOffIndicator`, `NameTag`, `AvatarEmoji` (the user's emote), `RecordingIndicator` "REC" (mod 2400), and `ScreenShareIndicator`.
  - **Screen share viewing only** (`CallScreenShare`, mod 2403):
    - Zoomable `DailyScreenShare` with `objectFit: contain`.
    - `ScreenShareUserChip` (mod 2417) chooses between presenters and tracks "Presenter Screen Opened" (L798850).
    - Subscriptions are managed by `useDailySubscriptions` / `useDailyScreenShareSubscription` (`subscribeToTracksAutomatically: false`).
    - There is no `startScreenShare` in app code. Mobile cannot share its screen.
  - `CallControls` (mod 2306):
    - **Camera** toggle (`Camera` / `CameraOffFilled`) and **Mic** toggle (`Mic` / `MicOffFilled`), via `useToggleLocalDeviceState`. Permission denial shows "Calls require Camera|Microphone access. In your phone settings, tap Kumospace and turn on …" with "Open Settings" / "Cancel".
    - **Emotes** (`Emojis`, mod 2314): an emoji picker that sends WS `op:'emote'` and tracks "Emoted".
    - **Chat** button (`EnterPiP`): shows a lavender `Badge` with the unread count of the *nearby* session chat. It enters in-app PiP and opens `/chat/session?range=nearby`.
    - **Leave**.
  - Devices: there is no device-picker UI. `usePreferredMicrophone` (mod 4849, gate `mobile_microphone_priority`) auto-selects WIRED_OR_EARPIECE > SPEAKERPHONE > BLUETOOTH (inferred order). It reacts to `available-devices-updated`, and shows the error "Device is no longer available".
  - `DailyCallProvider` (mod 4850): `startAudioOff` and `startVideoOff` are true, and the user-media video constraints set a framerate/size cap.
  - Host mute: WS `hostMute` emits the `hostMuted` event, and `useHandleHostMute` turns the mic off (L1686360).
  - Recording: `RecordingManager` (mod 4853) listens to Daily `recording-started` / `recording-stopped`. It fetches recording metadata (`recordings/{id}`) and shows the toast "{name} started recording" / "{name} stopped recording".
  - Errors: "Call not available", plus repeated-connect-failure reporting to Sentry (gate `mobile_daily_error_reporting`).
- **In-app PiP** (`CallPiP`, mod 4851): a draggable, clamped tile with the participants in PiP mode. Tapping it returns to `/call` (`useInAppPiPActions`, mod 2312).
- Credentials come from the WS: `room` message `{dailyToken, dailyUrl}` → `setDailyCredentials` and `markTeleportArrival` (mod 4959).
- The iOS `UIBackgroundModes: ["voip"]` is set. On Android, `FOREGROUND_SERVICE_CAMERA` and `FOREGROUND_SERVICE_MICROPHONE` permissions keep the call alive in the background.

### 2.7 Chat list (`chat/index`, `ChatScreen`, mod 4119, L1491038)
- Gated: the Chat tab is hidden unless the Statsig gate `chat_mobile` is on. On a free space (`useIsFreeSpace`), `SendbirdContainer` swaps chat for `FreeSpaceScreen` (mod 2591, L1007478): "Upgrade your Space for chat" / "Kumospace chat includes chat history, search history, custom channels, groups, threads, reactions, gifs, and more\n\nUpgrade to a paid plan on desktop".
- `ChatPage` header "Chat" (mod 2592), plus `ChannelListLoadingBar` (an indeterminate progress bar while channel queries load, mod 2593).
- Sections, top to bottom:
  1. **Session channels** (`SessionChannelRow`, mod 4129):
     - "Nearby Chat" (Chat icon, `Range.nearby`), probably hidden while you are in the lobby (`useIsInLobby`, inferred).
     - "Public Chat" (Broadcast icon, `Range.broadcast`).
     - Each shows an unread count from `useUnreadSessionMessageCountByRange`.
  2. **Channels** accordion: `CustomChannelRow` (mod 4122). Shows a `Channel` or `PrivateLock` icon, the name, an unread badge, and bold text when unread. Data: `useCustomChannels` (Sendbird custom type `custom`, metadata `spaceUid`).
  3. **Groups** accordion: `GroupDmsRow` (mod 4126).
     - Stacked avatar with a member count.
     - The name is the comma-joined member display names (`getGroupDmsChannelName`).
     - Hidden channels are filtered out (`useGetIsChannelHidden`).
     - Data: `useGroupDmsChannels` (custom type `group_dms`).
  4. **Online** accordion: `UserRow` (mod 4130) for each online space member. Shows the avatar with availability and the DM unread badge. Tap opens the 1:1 DM (`navigateToChannel({dmUserId})`).
  5. **Offline** accordion: the same rows, greyed.
- Members come from `useSpaceUsers` (mod 4108): REST user roles (`useLiveUserRoles`, mod 4109, all pages auto-fetched) merged with WS census for online status, availability and room.
- Row press: `navigateToChannel` (mod 2313) tracks "Chat Opened" and pushes `/chat/channel?channelUrl|dmUserId`.
- Unread state lives in the Redux `chat.channelsUnreadMessages` (`setChannelsUnreadMessages`, mod 4124). It is fed by:
  - (a) REST `spaces/{name}/chatConfig` → `channels[].unread_message_count` / `unread_mention_count` (SendbirdManager, L1689118).
  - (b) the Sendbird `onChannelChanged` handler.
- Muted status (`useGetChannelMutedStatus`) comes from WS `userSettings` → `setChatSettings(data.chat.channel)` (L1725530).
- **Not present:** chat search, creating a channel or group, hiding or muting from mobile, archive, pinning, swipe actions. The only "create" is the implicit DM creation via `useGroupChannelOrCreateDm` / `useCreateChatChannel` (`POST chat/channel`, mod 4104).
- **Split view** on wide windows (`useIsSplitChat`: width ≥ 700, L1491014). `ChatLayout` (mod 2530) shows the list pane, a draggable divider and a detail pane. The list width fraction is persisted in AsyncStorage `chat-list-fraction` (mod 4117). The empty detail pane says "Select a conversation".

### 2.8 Channel (`chat/channel`, `ChatChannelScreen`, mod 4134)
- It resolves the channel with `useGroupChannelOrCreateDm({channelUrl | dmUserId})` (mod 2604).
  - States: loading spinner; error with **Try again** / **Go back**; ready.
- It renders the Sendbird UIKit `createGroupChannelFragment` with custom `Header`, `Input`, `MessageList` and `renderMessage`. The message list comes from UIKit `useGroupChannelMessages`, which uses MessageCollection and the local cache.
- **Header** (`PersistentChannelHeader`, mod 4807):
  - Back.
  - Title: the channel name, the DM user's name with avatar and status, or the group's member names.
  - Subtitle: the member count ("Members"/"Member"), or the user's location ("Lobby" / "Common Area" / room name).
  - A lock icon for private channels.
  - "Chat Closed" / "Thread Closed" when the channel is frozen.
  - Tapping it opens `/chat/channel/details?channelUrl=`.
- **Messages** (`MessageRenderer`, mod 4797):
  - Avatar, name, time `h:mm a`, and message grouping.
  - Markdown via `@khanacademy/simple-markdown` (`useMarkdownParseText`, mod 4798): bold, italic, strikethrough, inline code, fenced `CodeBlock`, links (`Link`, which opens a URL), mentions (`Mention`, mod 4803, `@mention(...)` serialization in mod 4135; tapping a user mention opens their DM), and GIFs (rendered images).
  - "(edited)" (L1673030). Deleted messages show "Deleted message." (soft delete: `useSoftDeleteMessage` replaces the text with "This message has been deleted." and data `{deleted:true}`).
  - Images: a thumbnail (expo-image with cacheKey). Tap opens the fullscreen zoom viewer.
  - Other files show the name and size. Tap shows "Download not available on mobile. Please use the desktop app." (L1673248).
  - Pending state "Sending…". Failed state "Not sent · Tap to retry".
  - `ReactionList` (mod 4805): emoji + count pills. Tap toggles the reaction (`useSendbirdReactionToggle`). An AddEmoji button opens the picker.
  - `ThreadReplies` (mod 4809): up to N reply avatars, "{n} replies", and " · new unread" when the thread has unread replies (per-thread last-read timestamps in `chat.lastReadsInfo`). Tap calls `navigateToThread` (tracks "Thread Opened").
  - `DateSeparator` (mod 4796): weekday, month, day. Highlighted blue for the first unread day (uses `useChannelLastReadTimestamp`).
- **Long-press** opens `ActionsSheet` (mod 4795):
  - Quick reactions 👍 🙌 😂 ❤️ 🔥 and an AddEmoji picker.
  - "Reply to Thread".
  - "Edit Message" (own messages).
  - "Delete Message" (own messages).
  - Not present: copy, forward, pin, report, mark unread. These exist in Sendbird UIKit, but the custom sheet replaces them.
- **Composer** (`ChatInput`, mod 4137):
  - **Image** button: expo-image-picker for images and videos with editing allowed. Files are resized to JPEG (`prepareFileForUpload`, mod 4142).
  - A **WebView rich-text editor** (`ChatWebView`, section 4).
  - **Send**.
  - Edit mode: a Close button, and the editor is prefilled via `setInput`.
  - Mention autocomplete: `MentionSelector` (mod 4180) lists users plus `@here` ("Notify every online member") and `@channel` ("Notify everyone in this channel"). Empty state "No results".
  - Typing indicator: "{a} is typing...", "{a} and {b} are typing...", "Several people are are typing..." (typo in the source, L1495531). Typing is sent through UIKit `enableTypingIndicator`.
  - It tracks "Chat Message Sent" `{provider:'sendbird', type}`.
- Read receipts: the channel is marked read on open and on new messages (`setMyLastRead`, `markChannelMessagesAsRead`). Other users' read receipts are **not** displayed. `GroupChannelMessageOutgoingStatus` exists in UIKit but is overridden by the custom renderer (inferred).
- Not present: pinned messages, scheduled messages, polls, voice messages (expo-audio is bundled for the UIKit recorder, but the app's ChatInput has no mic button), message search.

### 2.9 Channel details (`chat/channel/details`, `ChatChannelDetails`, mod 4132)
- Back (ChevronLeft), the channel icon (`Channel` / `PrivateLockFilled`), the title, and "{n} Member(s)".
- The member list is sorted online first, then by name. Each row shows `ProfileImage` with availability and the name.
- No actions: no mute, leave, notification settings, rename, add or remove members.

### 2.10 Thread (`chat/thread`, `ChatThreadLayout` mod 4817 + `ChatThreadScreen` mod 4821)
- The layout marks the thread read (`markThreadAsRead` with timestamp) and provides `ChatMessagesProvider` (mod 4818), a local list with `addMessages` / `removeMessage` / `editMessage` / `newMessages`.
- The parent message preview is pinned at the top (max 30% height), followed by "{n} Replies"/"Reply" and the reply list.
- Data: `useFetchThreadParent` and `useFetchThreadReplies` (infinite query `getThreadedMessagesByTimestamp`, `reverse`, `includeReactions`). Live updates come from `useSendbirdThreadListeners` (mod 4822).
- The list has "new messages" and "scroll to bottom" buttons.
- It uses the same composer, long-press sheet and reactions as the channel. Error state: "Thread is not loaded".

### 2.11 Session chat (`chat/session?range=`, `SessionChatChannel`, mod 4812)
- This is **not Sendbird**. It is the ephemeral in-space chat carried over the Kumospace WS.
- `range=nearby` ("Nearby Chat", the people in your room or zone) or `range=broadcast` ("Public Chat", the whole space).
- Sending: WS `{op: MessageType.<chat>, data:{message, range, participantIds, recipientId, zoneId}}`. `participantIds` is built from `useUserFirebaseIdsForRoom`.
- Receiving: WS `notification` with `type:'chat'` → `addSessionMessage({...msg, id: uuidv4(), timestamp: Date.now(), unread:true})`. It plays `assets/sounds/message.mp3` when the sender isn't you (mod 4960/4961).
- Messages live only in the Redux `sessionChat` slice. There is no history and nothing is persisted across app restarts (inferred).
- Header (`SessionChannelHeader`, mod 4815): Broadcast or Conversation icon, title, "{n} people/person" subtitle.
- Opening it marks the range read (`markRangeAsRead`). It can be reached from the in-call Chat button.

## 3. Background behaviour

### 3.1 Push notifications
- Push comes **only from Sendbird**. Token registration uses UIKit `enableAutoPushTokenRegistration: true` (L1689410) with the expo-notifications service (FCM on Android, APNs on iOS, mod 4058). There are no Kumospace-server pushes in JS.
- Foreground: `setNotificationHandler` returns `shouldShowBanner/List/Sound/Badge: false` (L1750535), so **no banners while the app is open**.
- Android background/killed: Firebase `setBackgroundMessageHandler` (mod 5095).
  - If `isSendbirdNotification` is true, it parses the payload and calls `scheduleNotificationAsync` with title, body, data and `identifier = messageId`.
  - Android channel "Default Channel", importance 4 (L1747852).
- Tap handling (`usePushNotificationHandler`, mod 4842, L1685618): `addNotificationResponseReceivedListener` plus `getLastNotificationResponseAsync` for cold start. If it is a Sendbird notification it calls `navigateToChannel(channel.channel_url)`. Thread-reply pushes also open the channel, not the thread (inferred).
- Types: Sendbird message and mention pushes only. Nothing for knocks, calls or recordings.
- App icon badge: when the app becomes active, `ChatLayout` calls `dismissAllNotificationsAsync()` and `setBadgeCountAsync(0)` (L992441).

### 3.2 Foreground and resume refresh
- `useRefreshUnreadMessagesOnForegrounded` (mod 4844): on AppState `active` it runs `invalidateQueries(['spaces', selectedSpace, 'chatConfig'])`, which refetches the REST unread summary.
- The WS machine (mod 4915) has the states disconnected, connecting, connected, ready, reconnecting, waitingForNetwork, connectionFailed and closed. It reconnects on network regain (NetInfo, mod 4093/4848), and `clearDataBeforeConnect` / `clearSpaceData` wipe office state on reconnect.
- Sendbird connect (`useSendbirdConnect`) runs after chatConfig returns `{userId, sessionToken}`.
- Ghost-channel cleanup (`useMarkGhostChannelsAsRead`, L1012684):
  - Fetches up to 100 unread channels (`staleTime: Infinity`).
  - Marks as read every 1:1 `user` DM whose `spaceUid` is not one of your spaces, or whose peer isn't a space member. This keeps the badge from showing phantom unreads.

### 3.3 Persistence
- **AsyncStorage** (`useAsyncStorageItem`, mod 1502; keys are joined with `:`):
  - `selected-space`.
  - `subscription-tier`.
  - `custom-channels` and `group-dm-channels`: JSON.stringify of the Sendbird channel list, used as React Query `placeholderData` (mod 4113).
  - `chat-list-fraction`.
  - `recently-used-emojis`.
  - `useClearAsyncStorageKeys` clears `custom-channels`, `group-dm-channels`, `selected-space` and `subscription-tier` on sign-out.
- **MMKV**:
  - Sendbird SDK local cache, store id `sendbird-chat` (L1689837), `localCacheEnabled: true` (L1483522).
  - `LocalCacheConfig.maxSize` comes from the Statsig dynamic config `mobile_max_cache`, default 256 (L1689395).
- Firebase Auth persistence (AsyncStorage). Segment `storePersistor`.
- Redux and React Query are memory-only.

### 3.4 Analytics, flags and monitoring
- **Statsig** gates and configs (mod 2528, L992339):

  | Key | Enum name | Kind | Used? |
  |---|---|---|---|
  | `chat_mobile` | `ChatMobile` | gate | yes |
  | `mobile_debug` | `MobileDebug` | gate | yes |
  | `mobile_daily_error_reporting` | `DailyErrorReporting` | gate | yes |
  | `mobile_microphone_priority` | `MicrophonePriority` | gate | yes |
  | `chat_async_storage_cache` | `ChatAsyncStorageCache` | gate | **defined but never read**: the AsyncStorage channel cache is always on |
  | `mobile_max_cache` | `SendbirdMaxCacheSize` | DynamicConfig `{value}` | yes |

  The Statsig user carries `userID`, `email`, and custom `spaceName`, `accountCreationTime`, `spaceTier`, `isDemoSpace`, `isSpaceOwner`, `spaceCreatedAt` (mod 5086).
- **Segment**:
  - Screens (mod 5084): Landing `/`, Login `/sign-in`, Login Password, Reset Password, Spaces `/profile`, People `/space`, otherwise "{Path} Screen".
  - Tracks: Space Joined, Chat Opened, Thread Opened, Chat Message Sent, Teleport, Entry to Locked Room Requested, Range Entered, Range Exited, Emoted, Presenter Screen Opened. App lifecycle events are on.
- **Sentry**: React Navigation integration, native frames, an orientation breadcrumb, and WS/Daily/Sendbird error capture (`captureUnexpectedSendbirdError`, `captureUnexpectedAuthError`). Firebase Crashlytics is also present.
- Orientation: portrait is locked except in calls, where screen share allows landscape (mod 1265/5087, inferred).

## 4. Likely causes of slow chat

1. **The composer is a 2.1 MB WebView.**
   - `assets/webviews/index.html` is 2,152,243 bytes (React + Lexical editor + base64 fonts).
   - `ChatWebView` (mod 4166, L1507555) loads it as `source={{html}}`, after reading the whole file into JS through `useWebViewAsset` (React Query `['chat','webview']`, mod 4177).
   - A new WebView boots on every channel, thread and session screen. It stays at `opacity: 0` until it posts `ready`, so the input appears late and focus or keyboard is slow.
   - Every keystroke round-trips through `postMessage` JSON:
     - Web → RN: `ready`, `inputChange`, `focus`, `blur`, `heightChange` (ResizeObserver), `mentionQueryChange`.
     - RN → Web: `setInput`, `cleanInput`, `setEmoji`, `setMention`, `blur`.
     - Commands are serialized through `useTaskQueue` and `useSendWebViewCommand`.
   - `webviewDebuggingEnabled` is on in production (L1507804).
2. **Channel lists bypass the Sendbird local cache and refetch fully.**
   - `useCustomChannels` (L1010696) and `useGroupDmsChannels` (L1490295) use `createMyGroupChannelListQuery`, which is network-only, `limit: 100`. They loop `next()` until `hasNext` is false and then sort by name.
   - This is not `GroupChannelCollection`, so the MMKV cache and change-logs are never used for the list.
   - `useCustomChannelListeners` / `useGroupDmsChannelListeners` run a **debounced (1 s, leading + trailing) `invalidateQueries`** on every `onMessageReceived` / `onChannelChanged` in a channel of that type (L1010993). In a busy space the whole channel list is re-paginated from the network roughly once a second.
   - The AsyncStorage `placeholderData` holds plain JSON objects, not SDK instances, so it only papers over the first paint.
3. **Serial startup dependency chain**:
   - Firebase auth → Statsig init (the splash waits for it) → REST `spaces/{name}/chatConfig` → Sendbird `connect(userId, sessionToken)` → channel list queries → `useSpaceUsers` (paginated user roles plus WS census) before the Online/Offline sections can render.
   - On top of that, `useMarkGhostChannelsAsRead` runs another unread-channels query at limit 100.
4. **Unread counts come from REST, not Sendbird.** `chatConfig` is refetched on every foreground (mod 4844). The profile screen fires one `chatConfig` request per space (`SpaceCard`, L1683732).
5. **Space member list**:
   - `useLiveUserRoles` (L1488724) auto-fetches *all* pages of `spaces/{name}/userRoles?includeSpaceMetadata=true`.
   - It invalidates the whole infinite query whenever the WS `config.userRoles` changes.
   - Each change re-renders every `UserRow`, `GroupDmsRow` and mention list via `useNormalizedSpaceUsers`.
6. **Rendering cost**:
   - The chat list is a plain `ScrollView` with every row mapped (channels, groups, online, offline). It is not virtualized.
   - Markdown is parsed with simple-markdown in every `MessageRenderer` render.
   - `ChatMessagesProvider` re-sorts the full thread message array on every change.
   - Each row calls several selector hooks (`useChannelUnreadMessages`, `useNormalizedSpaceUsers`).
7. **Background/foreground churn**: on resume the WS reconnects and clears office data (`clearSpaceData`), and chatConfig is invalidated. Foreground pushes are suppressed, so nothing pre-warms.
8. **Smaller items**:
   - The `chat_async_storage_cache` gate is unused, so the cache can't be turned off remotely.
   - The Sendbird `maxSize` default is 256 MB (L1689395, presumably MB).
   - Thread replies page through `getThreadedMessagesByTimestamp` without any local cache.
   - Session chat has no history, so reopening it is empty after a restart (by design).

## 5. Feature checklist for the native rewrite
- Auth: Google, Microsoft, email+password, reset password. Apple exists in code but is unused.
- Space picker with unread per space, auto default space, sign out.
- Office: lobby, floors, rooms, common area, "More Rooms", people search, availability dots, mobile indicator, teleport to room or user, knock, room password, toasts.
- Call: Daily join from WS credentials, mic and camera toggles, permission alert, hide videos, emotes, active-speaker ring, mic-off indicator, REC badge and toasts, host mute, screen-share viewing with presenter switch and zoom, in-app PiP, leave to lobby, audio-route priority, background voip/foreground service.
- Chat list: Nearby/Public session chats, Channels, Groups, Online/Offline DMs, unread badges, split view on tablet with a resizable divider, free-tier upsell.
- Channel: markdown, mentions (@user/@here/@channel), image send, reactions with quick set, edit, soft delete, retry failed, threads, date separators with unread highlight, typing indicators, header with location/members, details screen with member list.
- Session chat over the WS, with a message sound.
- Push: Sendbird only. Tap opens the channel. Badge cleared on resume. No foreground banners.
