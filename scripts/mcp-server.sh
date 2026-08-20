#!/usr/bin/env bash
# Launcher stdio para o MCP neural-sgdb (macOS/Linux).
# Prefer target/mcp-release — rebuild nao conflita com binario em uso pelo MCP.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ALT="$ROOT/target/mcp-release/release/examples/mcp_server"
BIN="$ROOT/target/release/examples/mcp_server"
if [[ -x "$ALT" ]]; then
  EXE="$ALT"
elif [[ -x "$BIN" ]]; then
  EXE="$BIN"
else
  echo "Binario nao encontrado. Rode:" >&2
  echo "  cargo build --release --example mcp_server --target-dir target/mcp-release" >&2
  exit 1
fi
export NEURAL_SGDB_DB="${NEURAL_SGDB_DB:-$ROOT/.nsgdb/memory.db}"
export NEURAL_SGDB_EMBEDDER="${NEURAL_SGDB_EMBEDDER:-demo}"
exec "$EXE"
