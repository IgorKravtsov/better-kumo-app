#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"
REGISTER=0
FLAGS=()

usage() {
  cat <<USAGE
Usage: scripts/install-mcp.sh [--register] [--read-only | --self-only] [--space <name>]

Builds kumo-mcp and installs it to $BIN_DIR.
  --register    also add it to Claude Code (user scope) and Codex, when their CLIs are installed
  --read-only   register it with only the reading tools
  --self-only   register it so it writes only to your own notes channel (for trying it out)
  --space NAME  Kumospace space (default: the Relay's space, then your default space)
USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    --register) REGISTER=1 ;;
    --read-only | --self-only) FLAGS+=("$1") ;;
    --space) FLAGS+=("--space" "$2"); shift ;;
    -h | --help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
  shift
done

cargo build --manifest-path "$ROOT/core/Cargo.toml" -p kumo-mcp --release
mkdir -p "$BIN_DIR"
install -m 755 "$ROOT/core/target/release/kumo-mcp" "$BIN_DIR/kumo-mcp"
echo "kumo-mcp installed to $BIN_DIR/kumo-mcp"

if ! "$BIN_DIR/kumo-mcp" check ${FLAGS[@]+"${FLAGS[@]}"}; then
  echo "kumo-mcp could not connect. Start Kumospace desktop through Better Kumo, then run: $BIN_DIR/kumo-mcp login" >&2
fi

if [ "$REGISTER" = 1 ]; then
  if command -v claude > /dev/null; then
    claude mcp remove --scope user kumo > /dev/null 2>&1 || true
    claude mcp add --scope user kumo -- "$BIN_DIR/kumo-mcp" ${FLAGS[@]+"${FLAGS[@]}"}
    echo "Added to Claude Code as 'kumo'."
  fi
  if command -v codex > /dev/null; then
    codex mcp remove kumo > /dev/null 2>&1 || true
    codex mcp add kumo -- "$BIN_DIR/kumo-mcp" ${FLAGS[@]+"${FLAGS[@]}"}
    echo "Added to Codex as 'kumo'."
  fi
else
  echo "Add it to Claude Code:  claude mcp add --scope user kumo -- $BIN_DIR/kumo-mcp ${FLAGS[*]+"${FLAGS[*]}"}"
  echo "Add it to Codex:        codex mcp add kumo -- $BIN_DIR/kumo-mcp ${FLAGS[*]+"${FLAGS[*]}"}"
fi
