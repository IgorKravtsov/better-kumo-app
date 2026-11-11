# kumowatch

Listens to the Kumospace desktop app (Electron) over the Chrome DevTools Protocol and reacts
to its WebSocket traffic: ws.kumospace.com (presence, statuses, positions) and Sendbird (chat).

## Pieces
- `kumowatch.js` — the service: CDP attach, frame parsing, events, reconnect, launch watchdog.
- `rules.js` — what to do on events (ntfy pushes). Edit this to add scenarios.
- `launcher.js` — `node launcher.js launch` starts/focuses Kumospace with `--remote-debugging-port=9222`.
- `raycast/kumospace.sh` — Raycast Script Command wrapping the launcher.
- `replay-test.js` — replays a frame dump (`KUMO_DEBUG_LOG=frames.jsonl`) through rules with ntfy stubbed.

## launchd agents (~/Library/LaunchAgents)
- `com.kumowatch.service` — the service, at login, auto-restarted. Logs: `logs/kumowatch.log`.
- `com.kumowatch.kumospace` — starts Kumospace with the debug port at login.

    launchctl kickstart -k gui/$(id -u)/com.kumowatch.service   # restart after editing rules.js
    launchctl bootout gui/$(id -u)/com.kumowatch.service        # stop / disable

If Kumospace is opened without the port (Dock, ⌥6 bound to the app) the watchdog relaunches it
within a few seconds — only if it is younger than 2 minutes, so an active session is never killed.

## Settings (env vars in the plist)
`NTFY_TOPIC` (kumospace_ihor), `NTFY_SERVER`, `FORWARD_WHEN` (away,focusing), `KUMO_NEAR_RADIUS` (300),
`KUMO_DEBUG_LOG` (dump raw frames), `KUMO_WATCHDOG=0` (disable relaunch).

Note: the debug port gives any local process full access to your Kumospace session.
