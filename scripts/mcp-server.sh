#!/usr/bin/env bash
# Launcher stdio para o MCP neural-sgdb (macOS/Linux).
# Ordem: .nsgdb/bin > mcp-release > release. Auto-build se cargo no PATH.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INSTALLED="$ROOT/.nsgdb/bin/mcp_server"
ALT="$ROOT/target/mcp-release/release/examples/mcp_server"
BIN="$ROOT/target/release/examples/mcp_server"
SRC="$ROOT/examples/mcp_server.rs"

pick_exe() {
  for c in "$INSTALLED" "$ALT" "$BIN"; do
    if [[ -x "$c" && "$SRC" -ot "$c" ]]; then
      echo "$c"
      return 0
    fi
  done
  return 1
}

if EXE="$(pick_exe)"; then
  :
elif command -v cargo >/dev/null 2>&1; then
  echo "[neural-sgdb] building MCP to target/mcp-release..." >&2
  cargo build --release --example mcp_server --target-dir "$ROOT/target/mcp-release"
  EXE="$ALT"
else
  echo "Binario MCP nao encontrado. Rode: bash scripts/mcp-install.sh" >&2
  exit 1
fi

export NEURAL_SGDB_DB="${NEURAL_SGDB_DB:-$ROOT/.nsgdb/memory.db}"
export NEURAL_SGDB_EMBEDDER="${NEURAL_SGDB_EMBEDDER:-demo}"
export NEURAL_SGDB_DEFAULT_SCOPE="${NEURAL_SGDB_DEFAULT_SCOPE:-project/neural-sgdb}"
exec "$EXE"
