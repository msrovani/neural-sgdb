#!/usr/bin/env bash
# Bootstrap neural-sgdb MCP on a Linux Cloud Agent VM.
# Idempotent: safe to re-run from environment install.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if command -v rustup >/dev/null 2>&1; then
  rustup update stable
  rustup default stable
fi

bash scripts/mcp-install.sh
chmod +x scripts/mcp-server.sh

MCP_JSON="${HOME}/.cursor/mcp.json"
mkdir -p "${HOME}/.cursor"
LAUNCHER="$ROOT/scripts/mcp-server.sh"

# Merge neural-sgdb into ~/.cursor/mcp.json without wiping other servers.
python3 - "$MCP_JSON" "$LAUNCHER" <<'PY'
import json, os, sys

path, launcher = sys.argv[1], sys.argv[2]
cfg = {}
if os.path.isfile(path):
    with open(path, "r", encoding="utf-8") as f:
        try:
            cfg = json.load(f) or {}
        except json.JSONDecodeError:
            cfg = {}

servers = cfg.get("mcpServers")
if not isinstance(servers, dict):
    servers = {}

servers["neural-sgdb"] = {
    "type": "stdio",
    "command": "bash",
    "args": [launcher],
    "env": {"NEURAL_SGDB_DEFAULT_SCOPE": "project/neural-sgdb"},
}
cfg["mcpServers"] = servers

tmp = path + ".tmp"
with open(tmp, "w", encoding="utf-8") as f:
    json.dump(cfg, f, indent=2)
    f.write("\n")
os.replace(tmp, path)
print(f"OK: merged neural-sgdb into {path}")
print(f"    launcher={launcher}")
PY

echo "OK: cloud-agent-install complete (rustc=$(rustc --version 2>/dev/null || echo missing))"
