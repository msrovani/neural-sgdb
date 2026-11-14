#!/usr/bin/env bash
# Instala o binario MCP em .nsgdb/bin — rebuild seguro com MCP rodando.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/.nsgdb/bin"
TARGET="$ROOT/target/mcp-release"
mkdir -p "$OUT"
cd "$ROOT"
cargo build --release --example mcp_server --target-dir "$TARGET"
cp -f "$TARGET/release/examples/mcp_server" "$OUT/mcp_server"
chmod +x "$OUT/mcp_server"
echo "OK: $OUT/mcp_server"
