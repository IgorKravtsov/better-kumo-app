# Kumospace mobile (Android, Expo/RN, Hermes v99): backend protocol spec

Source: `decompiled.js` of the Android app 202610.24.411 (see `docs/reverse-engineering.md`) (L = line number in that file). Everything below was read from bytecode unless marked **(inferred)**.

Short version: the mobile app's REST surface is small (about 11 endpoints). Live space state (presence, rooms, teleport/join, session chat, Daily credentials) comes over `wss://ws.kumospace.com`. Persistent chat goes directly to Sendbird. Push tokens go to Sendbird, not to Kumospace.

---

## 0. Environment / constants

| Key | Value | L |
|---|---|---|
| EXPO_PUBLIC_API_URL | `https://api.kumospace.com` | 499606 |
| EXPO_PUBLIC_WS_URL | `wss://ws.kumospace.com` | 499606 |
| EXPO_PUBLIC_SENDBIRD_APP_ID | `CA71E01B-3C4B-4311-A953-B3904C6D7AD9` | 499606 |
| EXPO_PUBLIC_GOOGLE_WEB_CLIENT_ID | `927524943108-imd1omra1hn1absli84ka9n6qrjhr975.apps.googleusercontent.com` | 499606 |
| Assets | `https://assets.kumospace.com` | 427230 |

Firebase native config (not in JS; taken from `x/base/resources.arsc` with `strings`):
- `google_api_key` = `AIzaSyDMr7rTdxo97qyyVQcD5iM0VVDD8yJ6mfY`
- `google_app_id` = `1:927524943108:android:bd4b92a26e51dbea5e4c51` (sender/project number `927524943108`)
- `firebase_database_url` = `https://kumospace.firebaseio.com`, `google_storage_bucket` = `kumospace.appspot.com` → project id `kumospace` **(inferred from the DB/bucket names)**.

---

## 1. HTTP client (`@kumo/api`-style package, Metro modules 1550/1563–1568)

### Configuration
`configureKumoApiClient({baseUrl: env.EXPO_PUBLIC_API_URL, getCurrentUser, queryClient})` (L451128–451140). The QueryClient defaults are `refetchOnWindowFocus:false, staleTime:1000` (L451117). `getCurrentUser` = `getAuth().currentUser` (Firebase, L499952).

### URL building: `parseReactQueryKey(queryKey)` (L497782–497990)
- `queryKey` is an array. Every element except the last must be a string with no `/` (otherwise it throws `Expect queryKey to be an array of strings with out "/" except last that can be an object`, L497810).
- The last element is either another path segment, or an object `{params?, headers?}`.
- path = `segments.join('/')`. `params` are reduced to string values (`toString()`) and serialised with `URLSearchParams` → `path + '?' + qs` (L497870–497900, 497985).
- `headers` must be a flat `Record<string,string>`, otherwise TypeError (L497972).
- `safeFetch` builds `new URL('v1/' + path, baseUrl)` (L498295, 498541–498556). Example: `['users','me','recentSpaces',{params:{excludeDemo:'true'}}]` → `GET https://api.kumospace.com/v1/users/me/recentSpaces?excludeDemo=true`.

### Transport: `oauthFetch(url, user, init)` (L498004–498190)
- If there is a user: `idToken = await user.getIdToken()` (no forceRefresh argument, L498198), then the headers are
  `{'Content-Type':'application/json', 'Authorization':'Bearer ' + idToken, ...init.headers}` (L498069–498086), then `fetch(url.toString(), init)`.
- If there is no user: `fetch` with `{'Content-Type':'application/json', ...init.headers}` (L498183).
- If a Firebase auth error has code `auth/network-request-failed`, a synthetic `Response(undefined,{status:401})` is returned. Any other auth error is rethrown (L498110–498140).
- **No other headers** are added: no client version, no platform, no app id.
- Body: `JSON.stringify(body)` when a body is present (L498587–498592). Init passed = `{method, signal, headers, body}`.

### Response and error handling (`safeFetch`, L498265–498720)
- `res.ok && status === 204` → `undefined`. `res.ok` → `res.json()`. A JSON parse failure logs `[Kumo API] Failed to parse JSON response` and returns undefined (L498430–498460).
- `!res.ok` → `text = await res.text()`, which is parsed with zod `{title:string, detail:string}` (L498746). Then it throws
  `ApiError({title: parsed.title ?? 'Non-successful response', detail: parsed.detail ?? 'Response was not successful', reason, status, response: text})`.
  `reason` is mapped from status: 401→`unauthorized`, 403→`forbidden`, 400→`badRequest`, 500→`internalServerError`, otherwise `unknown` (L498757–498771).
- If fetch throws an Error → `ApiError{title:'Network failure', detail:'Could not connect; please try again later.', reason: networkFailure, status:-1}`. Any other throw → `'Unknown error' / 'Failed to get a response'` (L498510–498530).

### Hooks
| Helper | Behaviour | L |
|---|---|---|
| `useKumoApiQuery({queryKey,...opts})` | react-query `useQuery`, `GET` with signal and headers from the key | 498789 |
| `useKumoApiQueries` | `useQueries`, each entry `GET` | 498845 |
| `useKumoApiMutation({mutationKey, method, invalidateQueries?, onSuccess?})` | `useMutation`. `mutationFn(vars)` → `safeFetch({url, method, headers, body: vars})`, so **the mutate variables are the JSON body**. On success it invalidates each key in `invalidateQueries` | 497584–497700 |
| `useKumoApiPaginatedQuery({queryKey})` | `useInfiniteQuery`, `GET`. Each page request merges `{next: String(pageParam)}` into `params` (L498950–498965). `getNextPageParam = page => page.metadata.next`. It warns if the response is not paginated (L499040) | 498899 |
| `fetchKumoApiQuery` / `fetchKumoApiPaginatedQuery` | `queryClient.fetchQuery` / `fetchInfiniteQuery` versions of the above | 499105 / 499269 |

**Pagination shape** (`isPaginatedResponse`, L499503): `{ items: T[], metadata: { count: number, next: string|null } }`. `items` is confirmed by `pages.flatMap(p => p.items)` in useLiveUserRoles (L1488760–1488800). The next page is requested as `?…&next=<metadata.next>`.

---

## 2. REST endpoints used by the mobile app

These are all the call sites of the helpers. A full-file scan for `.useKumoApi*/.fetchKumoApi*/.safeFetch` found 22 hits: 6 are re-export getters, 6 are internal, and the rest are listed below. `EXPO_PUBLIC_API_URL` is read only once (L451131), so no other code talks to the API base.

| # | Method & path | Body / params | Hook (L) | Used for |
|---|---|---|---|---|
| 1 | `GET /v1/users/me/recentSpaces?excludeDemo=true` | – | `useRecentlyVisitedSpaces` (L1684122) | recent spaces list |
| 2 | `GET /v1/users/me/defaultSpace` | – | `useDefaultSpace({enabled})` (L1684137), used by `useSelectDefaultSpace` (L1684036). It reads `data.defaultSpace` and writes it to AsyncStorage `selected-space` | choosing the space at startup |
| 3 | `GET /v1/spaces?members=true&excludeDemo=true` | – | `useSelfSpaces` (L1488225). Consumers read `data.items[]` (`useMarkGhostChannelsAsRead` L1012738, `ProfileScreen` L1683065) | "my spaces" |
| 4 | `GET /v1/spaces/{spaceName}` | – | `useGetSpaceByName({spaceName})` (L1488201), enabled when spaceName is set | space metadata |
| 5 | `GET /v1/spaces/{spaceName}/chatConfig` | – | `useSpaceChatConfig(spaceName)` (L1488240). Response used: `{userId, sessionToken, channels:[{channel_url, unread_message_count, unread_mention_count}]}` (SendbirdManager L1689160–1689200, SpaceCard L1683758) | **Sendbird credentials** and unread counts |
| 6 | `GET /v1/spaces/{spaceName}/userRoles?includeSpaceMetadata=true[&next=…]` (paginated) | – | `useFetchUserRoles({enabled})` (L1488288). `keepPreviousData`, cacheTime and staleTime 300000. The key comes from `getUserRolesQueryKey` (L1488270). Items carry `userId, displayName, avatarUrl, role` (useNormalizedSpaceUsers L1488596–1488640) | the member directory, including offline users |
| 7 | `GET /v1/recordings/{recordingId}` | – | `fetchRecordingById` (L1688981), called from `RecordingManager` (L1688448, L1688729) when Daily reports a recording. It reads `startedBy.firebaseUid` and `layout.participants.video` | recording banner metadata |
| 8 | `POST /v1/chat/channel` | `{channelType:'user', spaceUid, userIds:[<uidA>, <uidB>]}` (self firebaseUid and the other user's uid; order inside the array **(inferred)**) | `useCreateChatChannel` (L1488168), called by `useGroupChannelOrCreateDm` (L1011498–1011560) when `sdk.groupChannel.getChannel(url)` fails with `ResourceNotFoundCode` | server-side creation of a DM Sendbird channel |
| 9 | `DELETE /v1/chat/channel/{channelUrl}/file-messages/{messageId}` | none | `useSoftDeleteFileMessage({channelUrl,messageId})` (L1488150), used by `useSoftDeleteMessage` (L1012275) for file messages | deleting a file message |
| 10 | `POST /v1/account/resetPassword` | `{email}` (lower-cased) | `useResetPassword` (L1731149), `ResetPasswordScreen` (L1730977). On success it routes to `/sign-in/password?resetEmail=` | password-reset email |

Desktop endpoints the mobile bundle does **not** call: `users/me/recentSpaces/{space}`, `users/me/profile/form/...`, `spaces/{s}/trial`, `spaces/{s}/customChannels`. Custom channels are listed directly from Sendbird instead (see §4).

Error codes surfaced: `ApiError.detail` is shown in the UI (e.g. "This chat couldn't be loaded.", L1011580).

---

## 3. WebSocket `wss://ws.kumospace.com`

### URL / handshake: `buildConnectionUrl({spaceName, idToken, cachedUser})` (L1703467–1703517)
```
url = new URL(`${EXPO_PUBLIC_WS_URL}/${spaceName}`)
url.searchParams.set('timeZone', Intl.DateTimeFormat().resolvedOptions().timeZone)
url.searchParams.set('idToken', <Firebase ID token>)
url.searchParams.set('platform', 'android')        // literal in this Android bundle (iOS presumably 'ios', inferred)
if (cachedUser) url.searchParams.set('reconnectAs', JSON.stringify(cachedUser))
return encodeURI(url.href)
```
- Parameter names come from `WebSocketParams` (L426312–426330): `idToken, timeZone, platform, reconnectAs, inviteToken, name, advancedChat`. Mobile sets only the first four.
- `spaceName` = AsyncStorage `selected-space` (L1703545).
- The token comes from `getFirebaseUserIdToken(user)` (L499961). There is a `true` literal there, so it probably calls `getIdToken(true)` / force refresh **(inferred)**. It is fetched only when the Firebase user status is `registered` (L1703863–1703880).
- `cachedUser` = `selfUserEntitySelector(state)`, i.e. the last self entity. It is captured on exit from `awaitingMessages` (`cacheUser`, L1704078) and used on reconnect.
- Auth happens entirely in the query string. **No first message is sent by the client.** The server pushes `config`, `census`, and so on.

### Connection state machine `websocketConnection` (xstate, L1725211–1725260)
- `disconnected` --connect--> `connecting` (invoke `connectToSocket`: `new WebSocket(url)`, then `open`→`socketOpened`, `error`→`socketFailedToConnect`)
- `connecting`: on socketOpened run `setSocket` → `awaitingMessages`. On socketFailedToConnect run `setUserBanned` → `connectionFailed`.
- `awaitingMessages`: entry `clearDataBeforeConnect` (resetSpaceData and setDailyCredentials(null)), exit `cacheUser`, invoke `handleSocketMessages`. On `socketDisconnected` → `connectionFailed`. On `userBanned` → `closed`.
- `connectionFailed`: entry `closeSocket`. **After 2000 ms**: if userBanned → `closed`; else if networkConnected → `connecting`; else → `waitingForNetwork`.
- `waitingForNetwork`: waits for NetInfo `isInternetReachable` → `connecting`.
- `closed`: final, entry `clearSpaceData`.
- **Close code 4000 = user banned** (L1704000, 1704067). Any other close, an `error` while not CLOSED/CLOSING (also reported to Sentry), or NetInfo offline → `socketDisconnected`.
- **No heartbeat or ping** is implemented client-side (no ping/pong strings in app code).
- The provider exposes `{status, error, cachedSelf, connect, sendMessage, close}` (L1704315). Status values: `closed|connected|reconnecting|ready|...`. `connect()` closes any open socket first and is called whenever `selected-space` changes (L1704260–1704280).

### Framing
Each frame is JSON `{op, data}`. `sendMessage(msg)` checks `readyState===OPEN` and calls `socket.send(JSON.stringify(msg))` (L1704283–1704300). Received frames go through `safeParseJson(event.data)` and a switch on `.op` (L1725478+).

### Ops the client SENDS (all mobile call sites of `useWebSocket().sendMessage`)
| op | data | Where (L) | Purpose |
|---|---|---|---|
| `join` | `{roomName, teleportTarget:{type:'user', userId}}` where roomName = target user's `roomName` (floor) | `useTeleporter` (L749886–749990) | teleport to a user ("join"). Rejected client-side if the user is in the lobby |
| `join` | `{roomName: zone.floorId, teleportTarget:{type:'zone', zoneId}}` | `useTeleporter` (L749950–749980) | join a room/zone. "Can't join common area" if no zoneId |
| `join` | same as above plus `password` | `useTeleporter` retry (L750060–750100) | password-protected room |
| `join` | `{}` | `LeaveCall` (L792664) | leave the call or room (then leaves the Daily call) |
| `emote` | `{emoji}` | `Emojis` (L935878) | emoji reaction |
| `chat` | `{message, range, participantIds, recipientId, zoneId}` with range = route param, default `nearby` (L1680497); participantIds = `useUserFirebaseIdsForRoom(roomId, floorId)`; zoneId = self `roomId`, undefined if null. In bytecode `recipientId` gets the same register as `range`, probably a decompiler artefact, so treat it as unset **(inferred)** | `SessionChatChannel` (L1680652–1680676) | ephemeral nearby/room chat (not Sendbird) |
| `refreshDailyToken` | (no data) | `onUpdateMeetingState` onError (L1686686–1686697), after `min(1000*n, 3000)` ms backoff when Daily join fails | asks the server for a fresh Daily token |

The shared protocol package defines further client ops that mobile **never calls** (L434510–434700, L434780–434840): `interactFurniture{roomId,furnitureId}`, `pushApp{roomId,furnitureId}`, `openDoor/closeDoor{roomId,zoneId,wallSegmentId}`, `timerAction`, `playSound{id,url,range,overrideMute,notificationType}`, `openApp{roomId,furnitureId}`. The `MessageType` enum also lists: `chat, privateMessage, startBroadcast, endBroadcast, pushApp, setUserStatus, setNetworkThreshold, setCensusCalendarEvent, setCurrentCalendarEvent, startGalleryView, endGalleryView, refreshDailyToken, startScreenshare, stopScreenshare, joinScreenshare, leaveScreenshare, cursorPosition, stampEmoji, updateUser, updateUserProfile, userProfiles, handlePendingAccessRequest, startMeeting, stopTranscribing, endMeeting, transcribeRawAudioData, uploadSelfCalendar, deafenSelf, floorEntryUpdated, userSettings`. The status/availability change op is presumably `setUserStatus` or `updateUser`, but mobile has no sender for it, so its payload is not visible here **(inferred)**.

### More client ops used by the web/desktop app (from its bundle)

| op | data | purpose |
|---|---|---|
| `command` | `{action: "destination", x, y}` | walk to a point on the current floor (click to move) |
| `command` | `{action: "navigateToZone", zoneId}` | walk into a room |
| `command` | `{action: "followUser" \| "stopFollowing" \| "removeFollower", userId}` | follow a person (census id) |
| `join` | `{roomName: <floorId>, destination?: {x, y}}` | change floor |
| `setUserStatus` | `{availability: available\|away\|focusing, message, setBy?}` | status |
| `nudge` | `{userId: <census id>}` | nudge someone |
| `emote`, `setHandRaised`, `requestFollow`, `muteParticipant`, `openDoor`/`closeDoor`, … | | other UI actions |

The desktop app keeps its state in a Redux store reachable from the React root
(`__reactContainer$…` fiber → `memoizedProps.store`): `users.censusMap`, `users.usersMap` (entities on the current
floor with `x, y`), `users.userSelf`, `space.zones` (rects on the current floor), `space.roomConfig.room`
(`width, height`), `space.spaceConfig.rooms` (floors), `daily.mediaEnabledState`, `daily.call` (the Daily call object).
`kumo-relay` uses this over CDP to mirror nudges and to drive the desktop avatar from the phone.

### Ops the client HANDLES (`useHandleWebSocketMessages`, L1725475–1725640)
| op | Handling |
|---|---|
| `error` | `setError(data)`. `data = {code, message}`. If code ∈ {`ZONE_CLOSED`, `ROOM_PASSWORD_REQUIRED`} it is handled specially (teleporter/password prompt), otherwise a toast shows `data.message`. Codes (L434610–434640): `DISALLOWED, DOES_NOT_EXIST, EMAIL_DOMAIN, FULL_CAPACITY, ROOM_CLOSED, ROOM_PASSWORD_REQUIRED, ROOM_PASSWORD_INCORRECT, MUST_BE_IN_ZONE, NOT_IN_FURNITURE_RANGE, NO_PATH_FOUND, ZONE_CLOSED, GUEST_TIME_LIMIT_REACHED, USER_CANNOT_BE_REMOVED` |
| `config` | `setSpace(data)` → `state.webSocket.space`. Fields read elsewhere: `space.uid` (useSpaceUid L398983), `space.name` (useSpaceConfig L397899), plus subscription tier (useIsFreeSpace L399045) |
| `room` | `{roomName, dailyToken, dailyUrl}`. Runs `leftRoom()`, then if a room is present `setDailyCredentials({token: dailyToken, url: dailyUrl})` and `markTeleportArrival(roomName)` (L1725595–1725625) |
| `guestTimeLimitReached` | `leftRoom()` |
| `census` | `updateCensus(data)`. `data` is an object `{[id]: user | null}`: entries with a value are upserted, null/absent values are removed (L755720–755797). Census user fields: `firebaseUid, name, avatarUrl, userStatus.availability, zoneId, roomName` (L1488600–1488620) |
| `entities` | `updateEntities(data)`. `data` is an array of entities `{id, type, isDeleted?, self?, zoneId, roomName,...}`. Deleted ones are removed (also from census); the entity with `self:true` and `type==='user'` becomes `state.self` (L755798–755883) |
| `spaceZones` | `updateRooms(data)`. Same `{[zoneId]: zone|null}` upsert/delete logic, rooms keyed by `zoneId` (L755897, adapter selectId L749840). Zone fields used: `zoneId, floorId, status` (`ZoneStatus.open`) |
| `hostMute` | emits local event `hostMuted` |
| `playSound` | (web client) `{notificationType, url?, …}`. `notificationType` ∈ `doorKnock, nudge, doorOpen, doorClose, emoji, other`; a nudge and a door knock arrive this way. Sender field name not verified yet (the core tries `senderId`, `fromUserId`, `sourceUserId`, `userId`, `firebaseUid`) |
| `userSettings` | `setChatSettings(data.chat.channel)` |
| `notification` | `useHandleWSNotification` (L1725660–1725720). If `data.type==='chat'` and `data.senderId !== selfId`, it plays a sound and runs `addSessionMessage({id: uuidv4(), ...data, timestamp: Date.now(), unread: true})` |

Terminology: the self entity's `zoneId` is the room and `roomName` is the floor (`useSelfLocation` → `{roomId: zoneId, floorId: roomName}`, L398837).

---

## 4. Sendbird

- App id `CA71E01B-3C4B-4311-A953-B3904C6D7AD9`. Hosts are the SDK defaults `https://api-<appId>.sendbird.com` and `wss://ws-<appId>.sendbird.com` (L1018625–1018632).
- **Connect** (`SendbirdManager`, L1689125–1689360): `chatConfig = GET /v1/spaces/{selected-space}/chatConfig` (§2 #5), then `useConnection().connect(chatConfig.userId, {accessToken: chatConfig.sessionToken})` (L1689340–1689355, `useSendbirdConnect` L1011712). Whether `userId` equals the Firebase uid is not stated here. DM channel URLs are built from firebase uids, so it is likely **(inferred)**.
- UIKit container (`SendbirdContainer`, L1689373+): `appId`, `chatOptions: {enableAutoPushTokenRegistration:true, localCacheStorage: MMKV, localCacheConfig: new LocalCacheConfig({maxSize: DynamicConfig.SendbirdMaxCacheSize ?? 256})}`, `uikitOptions.groupChannelList.enableTypingIndicator=true`, Expo platform services. It is gated by the Statsig feature gate `ChatMobile` and is off for free spaces (`useIsFreeSpace`).
- **Unread counts** come first from `chatConfig.channels[]` and then from `useSendbirdOnChannelChanged` (`channel.unreadMessageCount/unreadMentionCount`) (L1689180–1689260).

### Channel URL scheme (`getChatChannelUrl` / `findChatChannelUrl`, L425359–425540)
| channelType (`customType`) | URL |
|---|---|
| `user` (1:1 DM) | `${spaceUid}_${[uidA,uidB].sort().join('_')}` |
| `group_dms` | `${spaceUid}_group_dms_${nanoid12}` (alphabet `0-9a-z`, length 12, L425760–425770) |
| `custom` | `${spaceUid}_custom_${nanoid12}` |
| nearby | prefix `getNearbyChannelPrefix(spaceUid)` = `${spaceUid}_nearby_` (L425767). `isOtherUserNearbyChannel(url, spaceUid, selfUid)` = `url.startsWith(prefix) && url.split(prefix)[1] !== selfUid` (L425614), so the URL is `<spaceUid>_nearby_<ownerUid>` **(inferred)** |

- Custom channel display name: `buildCustomChannelName({name, spaceDisplayName})` = `"${name} | ${spaceDisplayName}"`, parsed back by `parseCustomChannelName` (L425414, 426387).
- Range ↔ channelType: `user→Range.user`, `group_dms→Range.groupDm`, `custom→Range.custom` (L425584). The Range enum values are `user, nearby, room, space, custom, groupDm` (L426300–426312).
- DMs are opened with `sdk.groupChannel.getChannel(url)` under queryKey `['sendbirdSdk','groupChannel',url]` (L1011449). On `ResourceNotFound` the app calls `POST /v1/chat/channel` (§2 #8), then refetches.

### Queries / collections
| Purpose | Call | L |
|---|---|---|
| Custom channels (`useCustomChannels`) | `createMyGroupChannelListQuery({customTypesFilter:['custom'], metadataKey:'spaceUid', metadataValues:[spaceUid], includeEmpty:true, includeMetaData:true, includeFrozen:false, limit:100})`, looped while `hasNext`, sorted by `name` | 1010827 |
| Group DMs (`useGroupDmsChannels`) | same with `customTypesFilter:['group_dms']` | 1490414 |
| Unread list (`useUnreadChannelList`, key `['sendbird-unread-channels']`) | `createMyGroupChannelListQuery({includeEmpty:true, includeFrozen:false, includeMetaData:true, unreadChannelFilter: UNREAD_MESSAGE, limit:100})` | 1012610 |
| Ghost channels (`useMarkGhostChannelsAsRead`) | For each unread channel: `getChannelMetadata` → `spaceUid`. If `customType==='user'` and the other member is not a current space user or the space is not in `useSelfSpaces`, call `channel.markAsRead()` once per URL | 1012701–1012930 |
| Message list | UIKit `createGroupChannelFragment` with custom Header/Input/MessageList (L1493986). The message collection uses UIKit defaults; the app does not override them. UIKit default list query: `{includeEmpty:false, limit:20}` (L1311808) | 1493986 |
| Thread replies (`useFetchThreadReplies`) | `parent.getThreadedMessagesByTimestamp(ts, {prevResultSize: <module const>, nextResultSize:0, includeReactions:true, reverse:true})`, infinite query | 1012022 |
| Send | `channel.sendUserMessage / sendFileMessage` | 1682043, 1682108 |
| Read | `channel.markAsRead()` | 1681343, 1681421 |

Nearby/room "session" chat is **not** Sendbird. It goes through the WS `chat` op and `notification` (§3).

---

## 5. Daily.co (verified live 2026-11-11)

- After `join {roomName: <floorId>, teleportTarget: {type: zone, zoneId}}` the server sends
  `room {roomName: <floorId>, dailyUrl: "https://kumospace.daily.co/<id>", dailyToken: <JWT, ~350 chars>}`.
  The Daily room is **per floor**, not per zone. Then `entities` with the self entity (`zoneId` set) and
  `notification {type: "enter", message: "<name> joined the Floor"}`.
- The official app creates the call object with `startAudioOff: true, startVideoOff: true,
  subscribeToTracksAutomatically: false` (camera constraints 640×640 @24 fps) and subscribes only to the
  participants it should hear.
- **Daily `participant.info.userId` is the census key (the per-session entity id), not the Firebase uid.**
  `userName` is the display name. One user can be present twice (desktop and phone) with the same Firebase uid and
  different entity ids, so "self" must be identified by the entity id from `entities[self:true]`, never by uid.
- Rule used by our client: subscribe (camera, microphone, screen) only to participants whose `userId` is the key of a
  census person in the same `zoneId` as us; everyone else stays unsubscribed. The base subscription profile is set to
  unsubscribed before joining.

### Before this section was verified
- No REST endpoint. Credentials only arrive in the WS `room` frame: `{roomName, dailyToken, dailyUrl}` → `setDailyCredentials({token, url})` (L1725595–1725612). `useDailyCredentials` just reads `state.daily.credentials` (L798113).
- The call is joined with `callObject.join({url, token})` **(inferred; `r8.join(r9)` at L1686560)**. On failure it captures to Sentry, sets credentials to null, waits `min(1000*attempt, 3000)` ms and sends `{op:'refreshDailyToken'}` (L1686660–1686700). The server answers with a new `room` frame **(inferred)**.
- Leaving: `useLeaveDailyCall().mutate()` plus WS `join {}` (L792664).

## 6. Push notifications
- `usePushTokenRegistration` (L1405266–1405500): token = `platformService.notificationService.getFCMToken()`. With `createExpoNotificationService` (L1689805) that is `expo-notifications.getDevicePushTokenAsync().data`, the native FCM token (L1478026). It is registered with **Sendbird** via `sdk.registerFCMPushTokenForCurrentUser(token)` and re-registered on `onTokenRefresh`. `unregisterFCMPushTokenForCurrentUser` is available too. On iOS the UIKit's `enableAutoPushTokenRegistration` uses `getAPNSToken` **(inferred)**.
- **No Kumospace backend endpoint** for push tokens. Expo push tokens are not used.

## 7. Firebase Auth
- SDK: `@react-native-firebase/auth` (modular API `getAuth()`); config is native (§0).
- Sign-in methods (L1726960–1727260, L1730881):
  - Email/password: `signInWithEmailAndPassword` (`useSignInWithPassword`).
  - Google: `GoogleSignin.configure({webClientId: EXPO_PUBLIC_GOOGLE_WEB_CLIENT_ID})` → `GoogleSignin.signIn()` → `GoogleAuthProvider.credential(idToken)` → `signInWithCredential`.
  - Apple: `appleAuth.performRequest({requestedOperation: LOGIN, requestedScopes:[EMAIL, FULL_NAME]})` → `OAuthProvider('apple.com').credential({idToken: identityToken, rawNonce: nonce})` → `signInWithCredential`.
  - Microsoft: `signInWithRedirect(getAuth(), new OAuthProvider('microsoft.com'))`.
  - No email-link, custom-token or sign-up flow in app code. The Firebase JS-SDK REST paths at L465294–474855 are library code.
- Password reset goes through the Kumospace backend (`POST /v1/account/resetPassword {email}`), not `sendPasswordResetEmail`.
- Sign-out (`useSignOut`, L1683329): clears AsyncStorage keys, `ws.close()`, Sendbird disconnect, `signOut(getAuth())`.
- Backend auth: REST uses the `Authorization: Bearer <Firebase ID token>` header. WS uses the `?idToken=<Firebase ID token>` query parameter. Sendbird uses its own `sessionToken` from chatConfig.

### 7.1 Sign-in options for a third-party client (verified 2026-11-11)

- Both Firebase keys work from any client (no Android/iOS app restriction): the Android key from §0 and the web key
  `AIzaSyBE7rhrGFL4k4cq_c9LN6N-3bjoBkMV7j0` used by www.kumospace.com and the desktop app.
- Native Google Sign-In is not available to us: it needs an OAuth client registered for our package/bundle and
  signing cert in Kumospace's Google Cloud project. Google accepts only these redirect URIs for the web client
  `927524943108-imd1…`: `https://kumospace.firebaseapp.com/__/auth/handler`, `https://www.kumospace.com/__/auth/handler`
  (`localhost` and custom schemes give `redirect_uri_mismatch`).
- **Desktop hand-off (the way to do Google, Microsoft, SSO in our app).** The desktop app opens
  `https://www.kumospace.com/auth/redirect/desktop` in the system browser. The page signs the user in on the web if
  needed (any provider), then calls `POST /v1/account/desktop/token` (Bearer id token, no body) → `{idToken: <Firebase
  custom token>}` and navigates to `kumospace://login?token=<custom token>[&linkedPath=…]`. The desktop app
  (`app.asar` → `src/deepLinks.js`) registers the `kumospace` scheme and signs in with the custom token.
  A client exchanges it with `POST identitytoolkit/v1/accounts:signInWithCustomToken {token, returnSecureToken:true}`
  → `{idToken, refreshToken, expiresIn}`, then `accounts:lookup` for email and name. Custom tokens are short-lived (1 h).
- **Session transfer.** Any signed-in client can mint such a token with `POST /v1/account/desktop/token` and hand it
  to another device (QR code, deep link). The receiver gets its own independent Firebase session; the long-lived
  refresh token never leaves the first device. Verified with `kumo-cli transfer-token | kumo-cli --session … login --token-stdin`.
- The desktop app stores its Firebase user in `localStorage["firebase:authUser:<webKey>:[DEFAULT]"]`
  (`stsTokenManager.refreshToken`); `kumo-cli login --from-desktop` reads it over CDP for development.
- Sendbird `userId` from `chatConfig` equals the Firebase uid.
