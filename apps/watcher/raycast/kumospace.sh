#!/bin/bash

# Required parameters:
# @raycast.schemaVersion 1
# @raycast.title Kumospace
# @raycast.mode silent
# @raycast.packageName Kumowatch

# Optional parameters:
# @raycast.icon /Applications/Kumospace.app/Contents/Resources/electron.icns
# @raycast.description Open Kumospace with the debug port kumowatch needs

WATCHER_DIR="$(cd "$(dirname "$0")/.." && pwd)"
NODE_BIN="${KUMOWATCH_NODE:-$HOME/.local/share/mise/installs/node/24/bin/node}"
exec "$NODE_BIN" "$WATCHER_DIR/launcher.js" launch
