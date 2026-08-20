#!/usr/bin/env bash
# Smoke test do MCP server: install + initialize + tools/list + health structured.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
bash scripts/mcp-install.sh >/dev/null
DB="${TMPDIR:-/tmp}/neural-sgdb-mcp-smoke-$$.db"
export NEURAL_SGDB_DB="$DB"
BIN="$ROOT/.nsgdb/bin/mcp_server"
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
echo "$out" | sed -n '3p' | jq -e '.result.structuredContent.mcp_tool_count == 23' >/dev/null
echo "$out" | sed -n '3p' | jq -e '.result.structuredContent.onboarding != null' >/dev/null
rm -f "$DB"
echo "MCP smoke OK ($count tools, structured health)"
