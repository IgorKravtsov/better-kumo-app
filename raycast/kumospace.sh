#!/bin/bash

# Required parameters:
# @raycast.schemaVersion 1
# @raycast.title Kumospace
# @raycast.mode silent
# @raycast.packageName Kumowatch

# Optional parameters:
# @raycast.icon /Applications/Kumospace.app/Contents/Resources/electron.icns
# @raycast.description Open Kumospace with the debug port kumowatch needs

exec "/Users/shatori/.local/share/mise/installs/node/24/bin/node" "/Users/shatori/usr/projects/kumowatch/launcher.js" launch
