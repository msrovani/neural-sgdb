#!/usr/bin/env bash
# Launcher stdio para o MCP neural-sgdb (macOS/Linux).
# Usa o binario release — evita depender de cargo no PATH do IDE.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/release/examples/mcp_server"
if [[ ! -x "$BIN" ]]; then
  echo "Binario nao encontrado. Rode: cargo build --release --example mcp_server" >&2
  exit 1
fi
export NEURAL_SGDB_DB="${NEURAL_SGDB_DB:-$ROOT/.nsgdb/memory.db}"
export NEURAL_SGDB_EMBEDDER="${NEURAL_SGDB_EMBEDDER:-demo}"
exec "$BIN"
