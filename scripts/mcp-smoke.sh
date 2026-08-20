#!/usr/bin/env bash
# Smoke test do MCP server: initialize + tools/list (23 tools, era_report) + health JSON.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
cargo build --release --example mcp_server
DB="${TMPDIR:-/tmp}/neural-sgdb-mcp-smoke-$$.db"
export NEURAL_SGDB_DB="$DB"
BIN="$ROOT/target/release/examples/mcp_server"
INIT='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}'
LIST='{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
HEALTH='{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"health","arguments":{}}}'
out=$(printf '%s\n' "$INIT" "$LIST" "$HEALTH" | "$BIN" 2>/dev/null)
count=$(echo "$out" | sed -n '2p' | jq '.result.tools | length')
if [[ "$count" -ne 23 ]]; then
  echo "tools/list: expected 23 tools, got $count" >&2
  exit 1
fi
echo "$out" | sed -n '2p' | jq -e '.result.tools[] | select(.name=="era_report")' >/dev/null
echo "$out" | sed -n '2p' | jq -e '.result.tools[] | select(.inputSchema.properties.type!=null) | select(.name=="remember")' >/dev/null
echo "$out" | sed -n '2p' | jq -e '.result.tools[] | select(.inputSchema.properties.format!=null) | select(.name=="recall")' >/dev/null
health_json=$(echo "$out" | sed -n '3p' | jq -r '.result.content[0].text' | jq -e '.mcp_tool_count == 23 and .onboarding != null')
rm -f "$DB"
echo "MCP smoke OK ($count tools, era_report present, health onboarding)"
