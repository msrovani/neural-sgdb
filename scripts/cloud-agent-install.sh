#!/usr/bin/env bash
# Bootstrap Cloud Agent VM: rustc stable ≥1.100 + MCP binary + mcp.json.
# Idempotent. Run from repo root (environment.json install).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

need_rust_update() {
  local ver major minor
  ver="$(rustc --version 2>/dev/null | awk '{print $2}' || true)"
  [[ -z "$ver" ]] && return 0
  major="${ver%%.*}"
  minor="${ver#*.}"
  minor="${minor%%.*}"
  # rustc < 1.100
  if (( major < 1 )) || (( major == 1 && minor < 100 )); then
    return 0
  fi
  return 1
}

if ! command -v rustup >/dev/null 2>&1; then
  echo "rustup missing — install Rust before cloud-agent-install" >&2
  exit 1
fi

if need_rust_update; then
  rustup update stable
  rustup default stable
fi

bash "$ROOT/scripts/mcp-install.sh"

# Workspace mcp.json must launch via scripts/mcp-server.sh (Linux Cloud Agents).
MCP_WS="$ROOT/.cursor/mcp.json"
mkdir -p "$ROOT/.cursor"
cat >"$MCP_WS" <<'EOF'
{
  "mcpServers": {
    "neural-sgdb": {
      "type": "stdio",
      "command": "bash",
      "args": [
        "${workspaceFolder}/scripts/mcp-server.sh"
      ],
      "env": {
        "NEURAL_SGDB_DEFAULT_SCOPE": "project/neural-sgdb"
      }
    }
  }
}
EOF

# Merge ~/.cursor/mcp.json with neural-sgdb entry (absolute launcher path).
mkdir -p "$HOME/.cursor"
python3 - "$ROOT/scripts/mcp-server.sh" <<'PY'
import json, sys
from pathlib import Path
script = sys.argv[1]
home = Path.home() / ".cursor" / "mcp.json"
entry = {
    "type": "stdio",
    "command": "bash",
    "args": [script],
    "env": {"NEURAL_SGDB_DEFAULT_SCOPE": "project/neural-sgdb"},
}
data = {"mcpServers": {}}
if home.exists():
    try:
        data = json.loads(home.read_text())
    except json.JSONDecodeError:
        data = {"mcpServers": {}}
data.setdefault("mcpServers", {})["neural-sgdb"] = entry
home.write_text(json.dumps(data, indent=2) + "\n")
print(f"OK: merged {home}")
PY

echo "OK: cloud-agent-install complete (rustc=$(rustc --version | awk '{print $2}'), bin=$ROOT/.nsgdb/bin/mcp_server)"
