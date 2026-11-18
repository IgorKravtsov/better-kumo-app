---
name: kumospace-devtools
description: Start the Kumospace desktop app with its DevTools port and observe its live API - Office WebSocket frames, HTTP calls, Sendbird and Daily sockets, the Redux store and the JS bundles - locally or on another Mac over SSH. Use when reverse-engineering Kumospace, verifying a wire format, debugging the Relay's Desktop bridge, or updating docs/kumospace-api.md.
---

# Kumospace DevTools

The Kumospace desktop app is Electron, and it loads `https://www.kumospace.com/<space>`. Started with
`--remote-debugging-port=9222`, it exposes the Chrome DevTools Protocol (CDP) on `127.0.0.1:9222`, on loopback only.
Over CDP you can read every frame on its WebSockets, every HTTP call, its Redux store, and the JS bundles that
define the protocol. The Relay's Desktop bridge works the same way (`core/crates/kumo-relay/src/desktop/`, ADR-0001).

Everything here is **read-only by default**. The page is the user's live Kumospace session.

## What you know before you start

| Topic | Where |
|---|---|
| Office WebSocket ops (sent and handled), REST, Daily, Firebase | `docs/kumospace-api.md` |
| Sendbird chat protocol | `docs/sendbird-protocol.md` |
| Features and screens of the official apps | `docs/kumospace-features.md` |
| Handoff, Reclaim and the Lease | `GLOSSARY.md`, `docs/architecture.md` |
| Domain terms (Office, Floor, Zone, Desktop app, Relay, Alert…) | `GLOSSARY.md` |
| How the docs were first produced (APK decompile) | `docs/reverse-engineering.md` |

Read the relevant section first. Then use CDP to confirm, or to fill gaps.

## 1. Get a DevTools port

### Is it already up?
```bash
curl -s http://127.0.0.1:9222/json/version && node .claude/skills/kumospace-devtools/scripts/kumo-cdp.mjs targets
```
If you see a `page https://www.kumospace.com/<space>` target, skip to step 3.

**Better Kumo** (the tray app for macOS, Windows and Linux, `kumo-desktop`) starts Kumospace with port 9222 by itself. Its supervisor
restarts a freshly started Kumospace that lacks the port. If Better Kumo is running, the port is normally already there.
CDP allows several clients at once, so you can attach next to the Relay.

### Start Kumospace with the port (macOS)
```bash
pgrep -fl "Kumospace.app/Contents/MacOS/Kumospace"
open -a Kumospace --args --remote-debugging-port=9222
```
- If Kumospace is running **without** `--remote-debugging-port` in that `pgrep` line, it has to be restarted:
  `osascript -e 'quit app "Kumospace"'`, wait until `pgrep` is empty, then run the `open` above.
- **Ask the user before restarting.** It drops them from their Zone, their call and any screen share.
- **Windows:** `"%LOCALAPPDATA%\Programs\Kumospace\Kumospace.exe" --remote-debugging-port=9222`
- **Linux:** run the `kumospace` binary with the same flag.

Then wait until a page target appears. The app needs a signed-in session showing a space:
```bash
for i in $(seq 1 30); do node .claude/skills/kumospace-devtools/scripts/kumo-cdp.mjs targets 2>/dev/null | grep -q '^page.*kumospace.com' && break; sleep 1; done
```

## 2. Kumospace on another Mac (SSH)

The port listens on the remote Mac's loopback, so tunnel it. Use the host alias from `~/.ssh/config` (for example
`office-mac`) and **a free local port**; 9222 and 9333 may already be in use locally:
```bash
lsof -nP -iTCP:9444 -sTCP:LISTEN || ssh -o BatchMode=yes -o ExitOnForwardFailure=yes -f -N -L 9444:127.0.0.1:9222 office-mac
export KUMO_CDP=http://127.0.0.1:9444
node .claude/skills/kumospace-devtools/scripts/kumo-cdp.mjs targets
```
- The target's `webSocketDebuggerUrl` still says `127.0.0.1:9222`; `kumo-cdp.mjs` rewrites it to `KUMO_CDP`'s host.
- To check or start Kumospace there, use the same commands from step 1, prefixed with `ssh office-mac '…'`. If that Mac runs Better Kumo, the port is usually up.
- **Close the tunnel when done:** `pkill -f "L 9444:127.0.0.1:9222"`.

## 3. Observe

`node .claude/skills/kumospace-devtools/scripts/kumo-cdp.mjs <command>` needs Node 22 or newer (built-in WebSocket) and has no
dependencies. It reads `KUMO_CDP` (default `http://127.0.0.1:9222`). Output redacts tokens, secrets, passwords and
cookies; pass `--raw` to `tail` only when you truly need them, and never paste them anywhere.

| Command | Use |
|---|---|
| `targets` | List the DevTools targets |
| `tail --seconds 60 --out <file>` | Office WebSocket frames as JSON lines `{t, kind:"ws", dir:"in"\|"out", op, data}` |
| `tail … --http [--bodies]` | Also log HTTP calls to kumospace.com, sendbird.com and daily.co (`--bodies` adds JSON responses) |
| `tail … --all-sockets` | Also log the Sendbird and Daily sockets (see below) |
| `state` / `state users.userSelf` | Top-level Redux slices, or one dotted path as JSON |
| `eval '<expression>'` | Evaluate a **read-only** expression in the page |
| `grep '<term>' … [--context N] [--max N]` | Search the loaded bundles (`kumospace.com/assets/*.js`) |
| `ops` | The server ops and `notification` types the client handles, and the client op enum, read from the live bundle |
| `dump <dir>` | Save all bundles for offline grepping |

**Write files to the session scratchpad, never into the repo.** Frame logs contain names and messages; bundles are
Kumospace's code. `*.jsonl` is gitignored, but keep them out of the tree anyway.

### Reading what you capture
- **Office socket** (`wss://ws.kumospace.com`): JSON `{op, data}`. Frames on a socket that already existed when you
  attached are captured too, but CDP doesn't know its URL (`socket: null`), so `tail` recognises Office frames
  by their `op`.
- **Sendbird socket:** text frames as `CMD{json}`, e.g. `MESG{…}`, `READ{…}`, `TPST{…}` (typing), `SYEV{…}`, `PING{…}`.
  See `docs/sendbird-protocol.md`.
- **Daily socket:** `{"msgStr":"sig-ack"|"sig-presence"|…, "msgData":…}`. This is signalling noise unless you're debugging calls.
- **Frequent Office ops:** `entities` (positions), `cursorPositionV2`, `census`.
  Filter with `jq 'select(.op != "entities" and .op != "cursorPositionV2")' <file>`.
- To catch a specific event (a Broadcast, a Knock, a Public Chat message), start `tail`, ask the user to do the
  action in Kumospace or on a second account, then stop and read the frames.

### Finding a wire format in the bundle
1. Run `ops` to see whether the op exists, and which bundle file handles it.
2. Run `grep 'case\`<op>\`' --context 600` to read the handler: which Redux action it dispatches, and which fields it reads.
3. Run `grep '<actionName>(e,t){'` (or the reducer name) to see how the payload is stored.
4. Confirm with a live `tail` capture if you can. A handler shows which fields the client *uses*, not everything the server sends.

The bundles are minified and their file names carry a hash (`WebSocketProvider-<hash>.js`). Record the file name
and the date next to any fact you add to the docs. Search for string literals and op names, not minified identifiers,
because those change on every release.

### Redux store
- `state` finds the store from the React root (`__reactContainer$…` fiber → `memoizedProps.store`), the same way
  `page.js` does.
- **Useful slices:**
  - `users`: `userSelf`, `usersMap` (entities on this Floor), `censusMap` (everyone in the space, keyed by census id).
  - `space`: `zones`, `roomConfig`, `spaceConfig.rooms` (Floors).
  - `daily`: `mediaEnabledState`, `participants`.
  - `broadcasts.broadcasts`.
  - `chat.session.messages`: Public and Nearby Chat.

## Rules
- **Don't send anything** (`Runtime.evaluate` that calls `send`, `Input.*`, `Page.reload`, `Page.navigate`) unless the
  user asked for that exact action. A sent Office frame acts as the user: it moves the avatar, nudges people,
  changes their status.
- **Ask before restarting or reloading Kumospace.**
- **Turn off what you turn on** (`Network.disable`, `Debugger.disable`; `kumo-cdp.mjs` does this), and close tunnels.
- **Report what you saw, not what you assume.** Mark inferred fields as inferred in the docs.
- **New verified facts go into `docs/kumospace-api.md`** (with bundle file name and date), and new terms into `GLOSSARY.md`.
