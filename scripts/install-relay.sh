#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"
LOG_DIR="$HOME/Library/Logs/kumowatch"
PLIST="$HOME/Library/LaunchAgents/com.kumowatch.relay.plist"

cargo build --manifest-path "$ROOT/core/Cargo.toml" -p kumo-relay --release
mkdir -p "$BIN_DIR" "$LOG_DIR" "$(dirname "$PLIST")"
install -m 755 "$ROOT/core/target/release/kumo-relay" "$BIN_DIR/kumo-relay"

if [ ! -f "$HOME/.config/kumowatch/relay.toml" ]; then
  echo "No config yet. Run: $BIN_DIR/kumo-relay init --space <space> [--ntfy-topic <topic>]" >&2
  echo "Then sign in:   kumo-cli transfer-token | $BIN_DIR/kumo-relay login --token-stdin" >&2
  exit 1
fi

sed -e "s|__BIN__|$BIN_DIR/kumo-relay|" -e "s|__LOG__|$LOG_DIR/relay.log|" \
  "$ROOT/core/crates/kumo-relay/launchd/com.kumowatch.relay.plist.template" > "$PLIST"
launchctl bootout "gui/$(id -u)/com.kumowatch.relay" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$PLIST"
echo "kumo-relay installed; logs: $LOG_DIR/relay.log"
