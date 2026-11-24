# Sendbird Chat wire protocol (as used by the Kumospace Android app)

Reference for a minimal Rust client: connect, authenticate, list channels, load messages, receive and send messages, mark as read, and sync via changelogs.

Source: `~/.cache/kumowatch-re/an/decompiled.js` (hermes-dec output). `L` = line in that file.

**Evidence tags**
- **(bundle Lxxx)**: seen in decompiled code at that line.
- **(bundle str)**: the literal exists in the Hermes string table (`hstr.txt`), but the code that uses it was not decompiled.
- **(inferred)**: from knowledge of the public `@sendbird/chat` v4 bundle or the Sendbird Platform API. Check these against live traffic before relying on them.

**Main limitation.** hermes-dec does not emit class constructor bodies (`// Unsupported instruction: <CreateDerivedClass>`). In Sendbird v4, every `*RequestCommand` and WS command class sets its `path`, `params` and `payload` in the constructor. So exact REST paths, query parameter lists and outgoing WS payloads are mostly **(bundle str)/(inferred)**. Everything around them was decompiled: URL builder, headers, framing, ping, LOGI parsing, response payloadify. A traffic capture would confirm the rest; see §11.

---

## 0. Verified against the live backend (2026-11-11, `kumo-cli`, Rust core)

- WS login works exactly as in §2: `p=JS`, empty `pv`, `sv=4.22.14`, `user_id=<firebase uid>`, token from
  `chatConfig.sessionToken` in `Sec-WebSocket-Protocol: urlencode({"token":…})`. LOGI arrives with `key`,
  `ping_interval=15`, `pong_timeout`, `reconnect.retry_cnt=-1` (signed). Login including `chatConfig` takes about 0.7 s.
- The connection stays up with `PING{"id":<ms>,"active":1,"req_id":""}` every `ping_interval`.
- REST with `Session-Key` plus the §7.2 headers is accepted with simplified agent strings
  (`SB-User-Agent: JS/c4.22.14///omacos`).
- `GET /v3/users/{uid}/my_group_channels` (`limit=100`, `order=latest_last_message`, `show_member`, `show_empty`,
  `show_metadata`): 80 channels in about 1.4 s (one page). Channel types seen: `user` (DM), `group_dms`, `custom`.
  Custom channel `name` is `"<name> | <space name>"`; for `group_dms` the name is the space display name, so the title
  has to be built from the members. A self-DM `<space>_<uid>_<uid>` exists.
- `GET /v3/group_channels/{url}/messages` with `message_ts=<now+60s>&prev_limit=100&next_limit=0&include=false&reverse=false`
  returns 100 messages in one request (about 0.75 s). Do **not** send `include_reactions` and
  `include_reactions_summary` together: the server answers `400111 Invalid value`. `include_reply_type=all` adds thread replies.
- `GET /v3/group_channels/{url}/messages/changelogs?change_ts=…` returns only edited and deleted messages
  (`{updated, deleted, has_more, next}`), not new ones. New messages come from `messages?message_ts=` and from the socket.
- Mentions are inline in `message` as `@mention(type=user&id=<uid>)`.
- Messages are markdown (`**bold**`, `*italic*`, `~~strike~~`, `` `code` ``, fenced blocks, `[label](url)`, and
  `![alt w,h](https://media.giphy.com/…)` for GIFs).
- GIF picker (desktop composer, `SpacePage-BqaQev5S.js`, read 2026-11-24): `@giphy/js-fetch-api` with
  Kumospace's web key (`REACT_APP_GIPHY_KEY`), rating `pg-13`, a 2-column grid of `trending` until something is
  typed, then `search`. A pick inserts an image node that the composer exports as
  `![<gif title> <images.original.width>,<images.original.height>](<images.original.url>)`, so a GIF is an ordinary
  `MESG`. The phone apps do the same (`kumo-core::chat::gifs`), sending the GIF as its own message right away.
  The phones read the key at runtime from the public web app, so a rotated key keeps working: `GET
  https://www.kumospace.com/app` lists every chunk as `/assets/<name>-<hash>.js`; the env chunk (`useFetch-*.js`,
  2026-11-24) contains ``REACT_APP_GIPHY_KEY:`<key>` ``. Read once per app run, read again when Giphy answers 401/403,
  and a copy built into the app is used when the page can't be fetched.
- Ack frames for `MESG` / `FILE` / `MEDI` carry `msg_id`, `ts` (no `created_at`), `req_id`, `channel_id`,
  `previous_message_id`; `channel_url` may be missing. Live events send the sender as `user: {guest_id, name, image}`
  (REST uses `user_id, nickname, profile_url`).
- Edit: WS `MEDI {channel_url, msg_id, message[, data, mentioned_user_ids]}` (ack required).
- Reactions: `POST /v3/group_channels/{url}/messages/{id}/reactions {reaction, user_id}`,
  `DELETE …/reactions?reaction=&user_id=`.
- Delete: `DELETE /v3/group_channels/{url}/messages/{id}`. A message with thread replies fails with
  `400920 "The message has a reply"`; Kumospace then edits it to `"This message has been deleted."` with
  `data: {"deleted": true}`, and when its last reply is deleted it removes the parent through Kumospace
  `DELETE /v1/chat/channel/{url}/messages/{parentId}`. File messages are deleted through Kumospace
  `DELETE /v1/chat/channel/{url}/file-messages/{id}`.
- Upload: `POST /v3/storage/file` (multipart `file`, `channel_url`) → `{url, file_size, require_auth, thumbnails}`,
  then WS `FILE {channel_url, url, name, type, size, require_auth, custom, custom_type[, thumbnails, parent_message_id]}`.
- Multiple files message (MFM): SDK 4.22 has `sendMultipleFilesMessage` (`fileInfoList`, 2 to
  `multipleFilesMessageFileCountLimit` files, a `message` caption; the wire command carries `files`), but the official
  app's picker opens with `multipleFiles: false` and nothing shows its composer sending one. MFM is a per-app premium
  feature, and other Kumospace clients may not render `files`. Kumowatch therefore sends photos as one `FILE` per photo,
  then the text as `MESG` (see the Outbox batch in `GLOSSARY.md`). `MessageItem.images` are markdown images (GIFs)
  only; incoming MFM `files` are not parsed yet.
- Grouped sends (desktop composer, `SpacePage-BfLB7cqw.js`, read 2026-11-17): the desktop app does the same, one
  `FILE` per file in order and then the text, and when there is more than one part it puts the same
  `data: {"msgGroupId": <random UUID>}` on every part (`custom` on `FILE`, `data` on `MESG`). Only the send path
  reads `msgGroupId`; the client does not group by it when rendering. Kumowatch sets it the same way.
- Message text is Markdown, converted by Lexical (`WebSocketProvider-CYYCyQ0j.js`, read 2026-11-17): `**bold**`,
  `*italic*` / `_italic_`, `***bold italic***`, `~~strike~~`, `==highlight==`, `` `code` ``, fenced code blocks,
  `-`/`*`/`+` and `1.` lists, links and autolinks, `![…](url)` images, and mentions as
  `@mention(type=user&id=<uid>&name=…)` (URL-encoded query fields; `type=channel`/`here` for group mentions).
  Kumowatch renders all of it; list items show as `•` lines.
- Deletions and edits made while offline arrive through `GET /v3/group_channels/{url}/messages/changelogs?change_ts=`.
- File messages (`type: FILE`) keep the file name in `message`. File URLs on `file-us-3.sendbird.com` need auth:
  plain GET returns 401, `?auth=<LOGI ekey>` returns 200.

## 1. Versions and identity

| Item | Value | Evidence |
|---|---|---|
| Chat SDK | `@sendbird/chat` **4.22.14** (`SDK_VERSION`) | bundle L1032618 |
| UIKit | `@sendbird/uikit-react-native` 3.12.10 (`sb_uikit` extension) | bundle L1276805, L1483601 |
| App id | `CA71E01B-3C4B-4311-A953-B3904C6D7AD9` | bundle L1018625 |
| REST host | `https://api-<APP_ID>.sendbird.com` | bundle L1018620 |
| WS host | `wss://ws-<APP_ID>.sendbird.com` | bundle L1018629 |
| Platform enum | `react-native` (SendbirdPlatform.REACT_NATIVE), `android`, `ios`, `js` | bundle L1038100–1038116 |
| Local cache | enabled (MMKV), so `use_local_cache=1` | bundle L1070867 + api-spec §4 |
| Extensions set by UIKit | `sb_uikit=<3.12.10>`, `device-os-platform=<android/ios>`, sendbirdExtensions `[{product: UIKIT_CHAT, version, platform: react-native}]` | bundle L1483590–1483612 |

---

## 2. WebSocket connect

### 2.1 URL
Built by `_url({configTs, useSessionKey})` (bundle L1070860–1070985). It is one concatenation, in this order:

```
wss://ws-<APP_ID>.sendbird.com/?p=JS&pv=<OS_VERSION>&sv=4.22.14&ai=<APP_ID>[&av=<appVersion>][&user_id=<userId>]
  &pmce=<0|1>&active=1&device_token_types=gcm%2Chuawei%2Capns
  &SB-User-Agent=<enc>&SB-SDK-User-Agent=<enc>&Request-Sent-Timestamp=<ms>
  &include_extra_data=<enc list>[&expiring_session=1]&use_local_cache=<0|1>
  &include_poll_details=1[&uikit_config=1]&config_ts=<configTs>
```

| Param | Value | Condition | Evidence |
|---|---|---|---|
| path | `/` (the URL is `host + "/?p=JS..."`) | always | bundle L1070978 |
| `p` | `JS` (literal, also on React Native) | always | bundle L1070978 |
| `pv` | `encodeURIComponent(OS_VERSION)`. The getter returns `navigator.userAgent` with `,`→`.`, or `''` if absent. On RN this is probably empty or the RN UA | always | bundle L1070888, L1032622–1032645 |
| `sv` | `4.22.14` | always | bundle L1070891 |
| `ai` | app id | always | bundle L1070976 |
| `av` | app version | only if `appVersion` set (Kumospace: unknown) | bundle L1070903 |
| `user_id` | `encodeURIComponent(userId)` | only when **not** reconnecting with a session key (`!useSessionKey`) | bundle L1070910–1070917 |
| `pmce` | `1` if `websocket.compression` else `0` (permessage-deflate) | always | bundle L1070870, L1070974 |
| `active` | `1` (literal, part of `&active=1&device_token_types=`) | always | bundle L1070973 |
| `device_token_types` | `encodeURIComponent("gcm,huawei,apns")` | always | bundle L1070925–1070929 |
| `SB-User-Agent` | encoded `userAgentWithExtension(extensions)` (see §7.2) | always | bundle L1070971, L1038500–1038545 |
| `SB-SDK-User-Agent` | encoded `sendbirdSdkUserAgentWithExtension(runtimeEnv)` | always | bundle L1070970, L1038250–1038441 |
| `Request-Sent-Timestamp` | `Date.now()` in ms | always | bundle L1070969, L1070934 |
| `include_extra_data` | `encodeURIComponent(String([...]))` = `premium_feature_list,file_upload_size_limit,application_attributes,emoji_hash,multiple_file_send_max_size,notifications,message_template,ai_agent` | always | bundle L1070431, L1070968 |
| `expiring_session` | `1` | only when a `SessionHandler` is set | bundle L1070941–1070946 |
| `use_local_cache` | `1` if local cache enabled else `0` | always | bundle L1070966 |
| `include_poll_details` | `1` | always | bundle L1070965 |
| `uikit_config` | `1` | when the `sb_uikit` extension is present (true in this app) | bundle L1070955–1070960 |
| `config_ts` | cached app-info timestamp (`configTs`). On the first run without cache, send `0` **(inferred)** | always | bundle L1070964, L1071199 |

Before opening the socket, the SDK fires a throwaway `fetch(api.host)` warm-up and ignores the result (bundle L1055686–1055700). You can skip it.

### 2.2 Credentials go in `Sec-WebSocket-Protocol`, not the query
`new WebSocket(url, encodeURIComponent(token))`. The second argument is the subprotocol (bundle L1055745–1055756). `token` is `JSON.stringify(obj)` (bundle L1071266–1071276), where `obj` is:

| Situation | `obj` | Evidence |
|---|---|---|
| First connect with access/session token (`connect(userId, {accessToken})`) | `{"token":"<accessToken>"}` | bundle L1071219–1071226, L1071303–1071307 |
| Reconnect with an existing session key (`useSessionKey`; `user_id` is then omitted from the URL) | `{"auth":"<sessionKey>"}` | bundle L1071249–1071253 |
| No token | `{}` | bundle L1071193 |

So the header is `Sec-WebSocket-Protocol: %7B%22token%22%3A%22<token>%22%7D`. The server must echo it back. A Rust client (tungstenite) must send that header and accept the echo.

Login timeout is `websocket.responseTimeout` (`websocketResponseTimeout`, 5–30 s, default 10 s **(inferred default)**) (bundle L1071391–1071402, L1034794–1034815). Without a LOGI in time you get `LOGIN_TIMEOUT` 800190.

---

## 3. Frame format

| Aspect | Detail | Evidence |
|---|---|---|
| Outgoing | `CODE + JSON.stringify(payload) + "\n"`, e.g. `READ{"channel_url":"…","req_id":"…"}\n` | bundle L1037903–1037917 (`convertToMessage`) |
| Incoming | one WS text message can hold several frames. Split on `\n`, skip empty lines | bundle L1055450–1055458 |
| Parse | `code = s.substring(0,4)`, `payload = JSON.parse(s.substring(4))`. If parsing fails, the frame is treated as `NOOP` | bundle L1037860–1037880 |
| Dispatch | `LOGI` → login handler. `PONG` → pinger. `EXPR` → session-expiring event (if `expires_in >= 0`). `BUSY` → rate-limit event. `NOOP` → ignored. Everything else → `message` event | bundle L1055470–1055556 |
| Compression | only via `pmce=1` (permessage-deflate). No custom framing | bundle L1070870 |

### 3.1 `req_id` / ack
- Commands with `ackRequired` carry a `req_id` string. The server echoes the same `req_id` in its reply frame (same code, e.g. `MESG`/`READ`), which resolves the pending promise (bundle L1056745–1056800, `_ackStateMap`). The format is opaque: v4 uses a unique string, roughly a timestamp plus a counter **(inferred)**. Any unique string works.
- `READ` acks are tracked in a separate `_batchableCommandAckStateMap` keyed by `requestId`, with `channel_url` (bundle L1056750–1056760, L1056852–1056870).
- Ack timeout = `responseTimeout` → `ACK_TIMEOUT` 800180 (bundle L1056812–1056822).
- A server error reply is an `EROR` frame (or the same code with `"error":true`) with `{code, message, req_id}` **(bundle str EROR; shape inferred)**. The LOGI error shape is `{user_id:"", error:true, code, message}` (bundle L1059882–1059895).
- Incoming events that echo a REST action carry `unique_id`. The SDK drops `SYEV`/`READ` echoes of its own REST calls by `unique_id` (bundle L1056575–1056590).
- Any non-PING send resets the ping timer (`ping-refresh`) (bundle L1056470–1056480).

---

## 4. Commands

The full code set in the string table (bundle str): `LOGI PING PONG EXPR BUSY NOOP EROR MESG FILE BRDM ADMM AEDI MEDI FEDI PEDI DELM READ DLVR MACK MRCT TPST TPEN SYEV USEV ENTR EXIT MTHD MCNT VOTE`. `LEAV`, `UNRD` and `JOIN` are **not** in the string table.

| Code | Dir | Purpose | Payload (key fields) | Evidence |
|---|---|---|---|---|
| `LOGI` | S→C | login result, also the session-key refresh reply | see §5 | bundle L1059587–1059660, L1075103 |
| `LOGI` | C→S | session refresh over WS | `{"token":"<new token>","expiring_session":1,"req_id":"…"}` (ackRequired) | bundle L1073249–1073270 |
| `PING` | C→S | keepalive | `{"id":<Date.now()>,"active":1,"req_id":""}` | code bundle L1056468. Payload **(inferred)**, `active` and `id` are bundle str |
| `PONG` | S→C | keepalive reply | `{"id":…,"ts":…}` (contents ignored) | bundle L1055479, L1055549 |
| `EXPR` | S→C | session expiring | `{"expires_in":<s>,…}` → refresh the token if a handler is set | bundle L1055483, L1055532–1055545. `expires_in` is bundle str |
| `BUSY` | S→C | server soft rate limit | `{"retry_after":<s>,"reason_code":…,"message":…}`. Back off `retry_after*1000` ms | bundle L1055487, L1055518–1055526 |
| `MESG` | C→S | send user message (ackRequired) | `{"channel_url","message","data","custom_type","mention_type":"users"\|"channel","mentioned_user_ids":[],"push_option"?,"sorted_metaarray"?,"target_langs"?,"parent_message_id"?,"reply_to_channel"?,"poll_id"?,"silent"?,"req_id"}` | **(inferred)** shape. Field strings are bundle str. Client param set: bundle L1100403 |
| `MESG` | S→C | new user message, or the ack of your MESG (same `req_id`) | message object §9.2 plus `req_id`, `channel_type`, `is_op_msg`, `silent`, `mesg_token`? | bundle L1107776, L1096172 |
| `FILE` | both | file message | as MESG plus `url`, `name`, `type`, `size`, `thumbnails`, `require_auth` | bundle L1097272, L1094695 |
| `ADMM` | S→C | admin message | `type:"ADMM"`, `message`, `data`, `custom_type` | bundle str L1037193 |
| `BRDM` | S→C | broadcast message | like ADMM | bundle L1097198 |
| `MEDI` / `FEDI` / `AEDI` | S→C | user / file / admin message updated | full message object | bundle str |
| `DELM` | S→C | message deleted | `{"channel_url","channel_type","msg_id"}` **(inferred)** | bundle str |
| `READ` | C→S | mark channel as read (`channel.markAsRead()`) | `{"channel_url":"…","req_id":"…"}` | code bundle L1056750; arg `{channelUrl}` bundle L1206907+. Payload **(inferred)** |
| `READ` | S→C | read receipt / ack | `{"channel_url","channel_type","user":{…},"ts","req_id"}` **(inferred)** | bundle str |
| `DLVR` | both | delivery receipt (`markAsDelivered`) | `{"channel_url", …}` | bundle str. Args `{channelUrl,userId}` bundle L1207000+ |
| `MACK` | C→S | message ack for super groups | **(inferred)** | bundle str |
| `TPST` / `TPEN` | both | typing start / end | `{"channel_url":"…","time":<ms>,"req_id":""}`. Inbound adds `user` | args `{channelUrl,time}` bundle L1207538–1207700. Payload **(inferred)** |
| `MRCT` | S→C | reaction event | `{"channel_url","msg_id","reaction","user_id","operation":"ADD"\|"DELETE","updated_at"}` **(inferred)** | bundle str |
| `MTHD` | S→C | thread info updated | `{"channel_url","parent_message_id","thread_info":{…}}` **(inferred)** | bundle str |
| `SYEV` | S→C | system / channel events (join, leave, invite, hide, freeze, metadata, …). Has `cat` (category int) and `data` | **(inferred)** | bundle str |
| `USEV` | S→C | user events (block, friend discovery, total unread…) | **(inferred)** | bundle str |
| `ENTR` / `EXIT` | C→S | open channels only, not needed | | bundle str |
| `MCNT` | S→C | open-channel participant count | | bundle str |
| `PEDI` / `VOTE` | both | polls | | bundle str |

---

## 5. LOGI response (S→C)

`payloadify` of the login command shows what the SDK reads (bundle L1059587–1059660):

| Field | Meaning | Evidence |
|---|---|---|
| user fields at top level: `user_id`, `nickname`, `profile_url`, `require_auth_for_profile_image`, `metadata`, `is_online`, `is_active`, `last_seen_at`, `preferred_languages`, `friend_discovery_key`, `friend_name` | current user | bundle L1058547 |
| app info at top level: `emoji_hash`, `file_upload_size_limit`, `application_attributes`, `premium_feature_list`, `use_reaction`, `allow_sdk_log_ingestion`, `unread_cnt_threading_policy`, `last_msg_threading_policy`, `channel_rotation`, `uikit_config`, `notifications`, `message_template`, `log_publish_config`, `disable_supergroup_mack`, `config_sync_needed`, `request_dedup_interval_ms`, `sdk_background_connection_close_timeout` | app config | bundle L1058201 |
| `key` | **session key**, sent as the `Session-Key` header on REST and as `{"auth":key}` on reconnect | bundle L1075102 (`r10.key` → `session.sessionKey`) |
| `ekey` | encryption key for file URLs (`?auth=` on protected files) | bundle L1075103, L1073176 |
| `services` | list of session services (e.g. `["chat"]`) | bundle L1075104 |
| `new_key` | in a refresh reply (C→S LOGI with token): the new session key | bundle L1073333 (`newKey`), str `new_key` |
| `login_ts` | server login time (ms) | bundle L1059611 |
| `ping_interval` | **seconds**. SDK uses `*1000` | bundle L1059777–1059780 |
| `pong_timeout` | **seconds**. SDK uses `*1000` | bundle L1059783–1059785 |
| `reconnect` | `{interval, max_interval, retry_cnt, mul}` (seconds, seconds, count, multiplier) | bundle L1059618–1059630 |
| `max_unread_cnt_on_super_group`, `profile_image_encryption`, `concurrent_call_limit`, `back_off_delay` | misc | bundle L1059611 |
| error form | `{"error":true,"code":4003xx,"message":"…"}` | bundle L1059882 |

---

## 6. Keepalive and reconnect

| Item | Value | Evidence |
|---|---|---|
| PING interval | `setInterval(ping, ping_interval*1000)`, default **15000 ms** if not given | bundle L1070340–1070370 |
| First PING | right after login (`pinger.start()` = `refreshTimer(); ping()`) | bundle L1070375–1070387 |
| Pong timeout | `pong_timeout*1000`, default **5000 ms**. If it fires, the connection counts as dead (`pingDelegate.error` → close, then reconnect) | bundle L1070245–1070262 |
| Timer reset | every non-PING send dispatches `ping-refresh` and restarts the interval | bundle L1056470–1056480 |
| Reconnect delay | `calcTimeout(n) = n>0 ? min(interval * mul^(n-1), max_interval) * 1000 : 10` ms, from LOGI `reconnect` | bundle L1059793–1059815 |
| Max retries | `reconnect.retry_cnt` (`reconnectMaxRetry`) | bundle L1059787–1059789 |
| Reconnect auth | when a session key exists: no `user_id` in the URL, protocol `{"auth":"<sessionKey>"}` | bundle L1071199–1071253 |

---

## 7. REST API

### 7.1 Base and encoding
| Item | Detail | Evidence |
|---|---|---|
| Base | `https://api-<APP_ID>.sendbird.com` | bundle L1018620 |
| Prefixes | `/v3/users`, `/v3/sdk/users`, `/v3/group_channels`, `/v3/sdk/group_channels`, `/v3/storage/file`, `/v3/emojis`, `/v3/polls`, `/v3/sdk/ui_kit/configuration`, `/v3/sdk/statistics`, `/v3/sdk/applications/settings`, … | bundle L1037296–1037410 |
| Query encoding | `?k=v&…`. Arrays: each item encoded, joined with `,`. Objects: `encodeURIComponent(JSON.stringify(v))`. Scalars: `encodeURIComponent(String(v))`. `undefined`/`null` keys are dropped | bundle L1037605–1037700 |
| Body | JSON for POST/PUT/PATCH/DELETE | bundle L1037705–1037740 |
| Error body | `{"error":true,"code":<int>,"message":"…"}` **(inferred)** | |

### 7.2 Headers (`_createHeader`, bundle L1038557–1038650)
| Header | Value |
|---|---|
| `SendBird` | `JS,<OS_VERSION>,4.22.14,<APP_ID>` plus `,<appVersion>` if set |
| `SB-User-Agent` | `userAgentWithExtension`: `<prefix>/c4.22.14/<s+syncmgrVer\|''>/<u+uikitVer\|''>/o<device-os-platform>`. For this app: `reactnative/c4.22.14//u3.12.10/oandroid` (structure bundle L1038500–1038541, exact string **inferred**) |
| `SB-SDK-User-Agent` | `main_sdk_info=chat/react-native/4.22.14&device_os_platform=<android>&os_version=<Platform.Version>&extension_sdk_info=<product>/<platform>/<version>`. Keys bundle L1038268–1038320, `&`-join L1038430. Product string for UIKit is probably `uikit-chat` **(inferred)** |
| `Request-Sent-Timestamp` | `Date.now()` ms |
| `Content-Type` | `application/json; charset=utf-8` (omitted for multipart) |
| `App-Id` | app id |
| `Session-Key` | `session.sessionKey` (LOGI `key`), for every request with `requireAuth` (the default). Written as `Session-key`, headers are case-insensitive |
| `Access-token` | `session.authToken`, used **instead** for non-auth requests (session-key refresh over REST) |
| `Accept` | not set explicitly |

### 7.3 Endpoints
All paths are **(inferred)**. The path fragments in the right column are bundle str. Prefixes are from L1037296+.

| Use | Method + path | Query / body | Path strings |
|---|---|---|---|
| My group channels | `GET /v3/users/{user_id}/my_group_channels` | `token`, `limit`, `order` (`latest_last_message`\|`chronological`\|`channel_name_alphabetical`\|`metadata_value_alphabetical`), `show_member=true`, `show_read_receipt=true`, `show_delivery_receipt=true`, `show_empty`, `show_frozen`, `show_metadata`, `show_pinned_messages`, `show_latest_message`?, `show_conversation`?, `include_chat_notification`, `custom_types`, `custom_type_startswith`, `channel_urls`, `metadata_key`, `metadata_values`, `metadata_value_startswith`, `unread_filter` (`all`\|`unread_message`), `member_state_filter`, `hidden_mode`, `public_mode`, `super_mode`, `distinct_mode`, `name_contains`, `members_exactly_in`, `members_include_in`, `query_type`, `search_query`, `search_fields`, `user_ids`, `created_after`, `created_before` | `/my_group_channels` |
| App's custom-channel query | same | `custom_types=custom&metadata_key=spaceUid&metadata_values=<spaceUid>&show_empty=true&show_metadata=true&show_frozen=false&limit=100` (from api-spec §4) | |
| Single channel | `GET /v3/sdk/group_channels/{url}` (v4 SDK) or `/v3/group_channels/{url}` | `show_member=true`, `show_read_receipt=true`, `show_delivery_receipt=true`, `show_metadata`?, `is_feed_channel`? | |
| Messages | `GET /v3/group_channels/{url}/messages` | `is_sdk=true`, `message_ts` (ms), `prev_limit`, `next_limit`, `include` (inclusive, bool), `reverse`, `message_type` (`MESG`\|`FILE`\|`ADMM`), `custom_types`, `sender_ids`, `include_reply_type` (`none`\|`all`\|`only_reply_to_channel`), `with_sorted_meta_array`, `include_reactions` / `include_reactions_summary`, `include_thread_info`, `include_parent_message_info`, `include_poll_details`, `show_subchannel_messages_only`, `include_meta_array`? | strings `message_ts`, `prev_limit`, `next_limit`, `include`, `reverse`, `include_reply_type`, `include_reactions_summary`, `include_thread_info`, `include_parent_message_info`, `with_sorted_meta_array`, `show_subchannel_messages_only`, `is_sdk`. `include_reactions` was not found. Enum values `none`/`all`/`only_reply_to_channel` at bundle L1037150–1037229 |
| Message changelogs | `GET /v3/group_channels/{url}/messages/changelogs` | `change_ts` (ms) **or** `token`, plus the same `include_*`/`with_sorted_meta_array`/`include_reply_type` flags | `/messages/changelogs` |
| Channel changelogs | `GET /v3/users/{user_id}/my_group_channels/changelogs` | `change_ts` or `token`, `custom_types`, `show_empty`, `show_frozen`, `show_member`, `show_read_receipt`, `show_delivery_receipt`, `show_metadata`, `include_chat_notification` | `/my_group_channels/changelogs` |
| Mark as read (one channel) | WS `READ` (normal path). REST fallback `PUT /v3/group_channels/{url}/messages/mark_as_read` with `{"user_id"}` | | `/messages/mark_as_read` |
| Mark all / several read | `PUT /v3/users/{user_id}/mark_as_read_all` with body `{"channel_urls":[…]}` (optional) | | `/mark_as_read_all` |
| Total unread | `GET /v3/users/{user_id}/unread_message_count?custom_types=…&super_mode=all\|super\|nonsuper` → `{"unread_count":n}` | | `/unread_message_count`, `unread_count` |
| Unread breakdown | `GET /v3/users/{user_id}/unread_item_count?item_keys=…` → `group_channel_unread_message_count`, `non_super_group_channel_unread_message_count`, `super_group_channel_unread_message_count`, … | | those three keys are bundle str |
| Session key refresh | `POST /v3/users/{user_id}/session_key` with header `Access-token: <token>`, body `{"expiring_session":true,"services":["chat"]}` → `{"key":"<new>", "services":[…]}` | args `{userId,authToken,services,expiringSession:true}` bundle L1073455+; response `.key`/`.services` read | `/session_key` |
| FCM push token | `POST /v3/users/{user_id}/push/gcm` body `{"gcm_reg_token":"…","always_push":false,"is_unique":false}`. Delete: `DELETE /v3/users/{user_id}/push/gcm/{token}` | | `/push/gcm`, `/push/gcm/` |
| APNs push token | `POST /v3/users/{user_id}/push/apns` body `{"apns_device_token":"…",…}`. Delete: `DELETE …/push/apns/{token}` | | `/push/apns`, `/push/apns/` |

Pagination: list responses return `next` (opaque token, empty string when done). Changelogs return `{updated, deleted, has_more, next}` (`has_more` bundle str).

---

## 8. Session key and token expiry

| Topic | Behavior | Evidence |
|---|---|---|
| Session key source | LOGI `key` → `session.sessionKey`. `ekey` and `services` are stored too | bundle L1075103–1075112 |
| REST auth | `Session-Key: <key>` on every authenticated call | bundle L1038626–1038643 |
| Error codes | `400108 NON_AUTHORIZED`, `400111 INVALID_TOKEN`, `400300 USER_AUTH_DEACTIVATED`, `400301 USER_AUTH_DELETED_OR_NOT_FOUND`, **`400302 SESSION_TOKEN_EXPIRED`**, `400304 APPLICATION_NOT_FOUND`, **`400309 SESSION_KEY_EXPIRED`**, **`400310 SESSION_REVOKED`**, `400312 INVALID_SESSION_TYPE`, `400313 INVALID_AUTH_FOR_SERVICE`, `500910 RATE_LIMIT_EXCEEDED`, `900021 USER_DEACTIVATED`, `900050 CHANNEL_IS_FROZEN`. Client-side: `800180 ACK_TIMEOUT`, `800190 LOGIN_TIMEOUT`, `800200 WEBSOCKET_CONNECTION_CLOSED`, `800160 MARK_AS_READ_RATE_LIMIT_EXCEEDED` | bundle L1032686–1032960 |
| Session-related errors | `isSessionExpiredError` (400309), `isSessionTokenExpiredError` (400302), `isSessionInvalidatedError`, `isSessionRevokedError` (400310) | bundle L1032963–1033020, L1074616 |
| Key refresh (400309 or `EXPR`) | if WS is CONNECTED: send `LOGI{"token":<authToken>,"expiring_session":1,"req_id"}` and read `new_key`. Otherwise use REST `/session_key` with `Access-token`. Then replay the queued requests | bundle L1073290–1073455, L1073786–1074094 |
| Token expiry (400302) | asks `sessionHandler.onSessionTokenRequired(resolve, reject)` for a new token. Without a handler, the session closes (`onSessionClosed`/`onSessionError`) | bundle L1074094–1074330, L1075279–1075556 |
| `expiring_session=1` | only added to the WS URL when a session handler is set (bundle L1070941). Kumospace calls plain `connect(userId,{accessToken})`, and no app-side `setSessionHandler` was found, so it probably does not send it **(inferred)** |
| Practical rule for the Rust client | treat `sessionToken` from `/v1/spaces/{space}/chatConfig` as the access token. On 400302/400310 (or a LOGI error), fetch a new chatConfig and reconnect. On 400309, refresh with WS LOGI or REST `/session_key` |

---

## 9. Response shapes

### 9.1 Group channel (payloadify, bundle L1201296 + base L1102578)
| Group | Fields |
|---|---|
| base | `channel_url`, `name`, `cover_url`, `custom_type`, `data`, `freeze`, `is_ephemeral`, `created_by`, `created_at` (ms), `metadata` (when `show_metadata`), `pinned_message_ids` |
| flags | `is_access_code_required`, `is_distinct`, `is_super`, `is_broadcast`, `is_exclusive`, `is_public`, `is_discoverable`, `is_muted`, `is_push_enabled`, `has_bot`, `has_ai_bot`, `is_desk_channel`, `is_ai_agent_channel` |
| counters | `unread_message_count`, `unread_mention_count`, `total_unread_thread_message_count`, `member_count`, `joined_member_count` |
| my state | `member_state` (`joined`\|`invited`), `my_role`, `push_trigger_option`, `count_preference`, `hidden_state`, `user_last_read`, `ts_message_offset`, `message_purge_offset`, `inviter`, `invited_at`, `joined_ts` |
| embedded | `members` (user objects plus `state`, `role`, `is_muted`, …), `last_message` (message object), `read_receipt` (`{user_id: ts}`), `delivery_receipt`, `latest_pinned_message`, `latest_message`, `message_survival_seconds`, `conversation`, `helpdesk_info` |

### 9.2 Message
| Group | Fields | Evidence |
|---|---|---|
| base | `message_id` (WS frames also use `msg_id`), `type` (`MESG`\|`FILE`\|`ADMM`\|`BRDM`), `channel_url`, `channel_type` (`group`), `message`, `data`, `custom_type`, `mention_type`, `mentioned_user_ids`, `mentioned_users`, `sorted_metaarray`, `extended_message`, `extended_message_payload`, `created_at`, `updated_at` (ms) | bundle L1094434, L1094847 |
| threading / extras | `parent_message_id`, `parent_message_info`, `thread_info` {`reply_count`, `unread_message_count`, `member_count`, `most_replies`, `push_enabled`, `last_replied_at`, `updated_at`}, `reactions` [{`key`, `user_ids`, `updated_at`}], `reactions_summary` [{`key`, `sampled_user_ids`, `sampled_user_info`, `count`, `is_self_included`, `last_ts`}], `og_tag` {`og:title`, …, `og:image` {`url`, `secure_url`, `type`, `width`, `height`, `alt`}}, `silent`, `is_op_msg`, `poll`, `scheduled_message_id`, `scheduled_at`, `scheduled_status`, `translations`, `message_survival_seconds`, `mentioned_message_template`, `plugins`, `review_info`, `is_reply_to_channel` | bundle L1093809, L1094050, L1094249, L1094847, L1096607 |
| sendable | `user` (sender) {`user_id`, `nickname`, `profile_url`, `require_auth_for_profile_image`, `metadata`, `is_active`, `role`?}, `req_id`, `request_state`, `error_code`, `mesg_token` | bundle L1096172, L1107776, L1058547 |
| file | `url`, `require_auth`, `file` {`url`, `name`, `type`, `size`, `data`}, `files` (multiple), `thumbnails` [{`url`, `width`, `height`, `real_width`, `real_height`}] | bundle L1094695, L1097047, L1097272 |
| admin | `type`, `ts`, `user`, `message`, `data`, `custom_type` | bundle L1094975 |

---

## 10. Minimal client flow

1. `GET https://api.kumospace.com/v1/spaces/{space}/chatConfig` → `{userId, sessionToken, channels[]}`.
2. WS connect to the §2.1 URL with `user_id=<userId>`, plus header `Sec-WebSocket-Protocol: urlencode('{"token":"<sessionToken>"}')`.
3. Read `LOGI`: keep `key`, `ping_interval`, `pong_timeout`, `reconnect`. Start PING every `ping_interval` s (default 15 s) and treat a missing PONG after `pong_timeout` s (default 5 s) as a dead socket.
4. REST with `Session-Key: <key>` plus the §7.2 headers: list channels (`my_group_channels`), load history (`messages?message_ts=<now>&prev_limit=30&next_limit=0&include=false&reverse=true&include_reply_type=none`).
5. Live: handle `MESG`/`FILE`/`ADMM`/`BRDM` (new), `MEDI`/`FEDI`/`AEDI` (edits), `DELM`, `READ`, `TPST`/`TPEN`, `MRCT`, `SYEV`.
6. Send: `MESG{"channel_url":…,"message":…,"req_id":"<unique>"}\n`. Match the echoed `MESG` by `req_id`.
7. Read: `READ{"channel_url":…,"req_id":"<unique>"}\n`. The SDK rate-limits this to about 1/s per channel (`MARK_AS_READ_RATE_LIMIT_EXCEEDED`).
8. After a reconnect: `my_group_channels/changelogs?change_ts=<lastSync>` and per-channel `messages/changelogs?change_ts=<lastSync>`, then follow `next` while `has_more`.

---

## 11. Not determined from the bundle

- Exact outgoing JSON for `MESG`, `READ`, `PING`, `TPST`/`TPEN`, `DLVR`, and the exact query-param sets and paths of every REST request. All of these are set in class constructors, which hermes-dec did not decompile.
- Whether channel GET and message list use `/v3/group_channels` or `/v3/sdk/group_channels`. Both prefixes exist (L1037314–1037319).
- The `req_id` generator format.
- `include_reactions` vs `include_reactions_summary` naming. Only the latter is in the string table.
- The `pv` (OS_VERSION) value on React Native, and whether Kumospace sets `appVersion` (`av`).
- The exact `SB-User-Agent` / `SB-SDK-User-Agent` strings. The structure is known, the runtime values are not.
- The best way to settle all of these: capture real traffic (mitmproxy on the Android app, or the Kumospace web client through `~/.cache/kumowatch-re/sniff.js`, which already filters `sendbird.com`). The web client uses the same `@sendbird/chat` v4 wire protocol, with `p=JS` and a different user agent.
