#!/bin/bash
set -euo pipefail

WATCHER_DIR="$(cd "$(dirname "$0")/.." && pwd)"
NODE_BIN="${KUMOWATCH_NODE:-$HOME/.local/share/mise/installs/node/24/bin/node}"
AGENTS_DIR="$HOME/Library/LaunchAgents"
DOMAIN="gui/$(id -u)"
SERVICE_ONLY="${1:-}"

[ -x "$NODE_BIN" ] || { echo "node not found at $NODE_BIN (set KUMOWATCH_NODE)" >&2; exit 1; }
mkdir -p "$WATCHER_DIR/logs" "$AGENTS_DIR"

install_agent() {
  local label="$1"
  local target="$AGENTS_DIR/$label.plist"
  sed -e "s#__NODE__#$NODE_BIN#g" -e "s#__WATCHER_DIR__#$WATCHER_DIR#g" \
    "$WATCHER_DIR/launchd/$label.plist.template" > "$target.tmp"
  plutil -lint "$target.tmp" > /dev/null
  mv "$target.tmp" "$target"
  launchctl bootout "$DOMAIN/$label" 2> /dev/null || true
  launchctl bootstrap "$DOMAIN" "$target"
  echo "installed $label -> $target"
}

install_agent com.kumowatch.service
if [ "$SERVICE_ONLY" != "--service-only" ]; then
  plist="$AGENTS_DIR/com.kumowatch.kumospace.plist"
  sed -e "s#__NODE__#$NODE_BIN#g" -e "s#__WATCHER_DIR__#$WATCHER_DIR#g" \
    "$WATCHER_DIR/launchd/com.kumowatch.kumospace.plist.template" > "$plist"
  plutil -lint "$plist" > /dev/null
  echo "updated com.kumowatch.kumospace -> $plist (takes effect at next login; not restarted to keep the Kumospace session)"
fi
