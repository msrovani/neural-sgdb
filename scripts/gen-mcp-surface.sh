#!/usr/bin/env bash
# Superficie MCP GERADA (tools/aliases/contrato) a partir de examples/mcp_server.rs.
#
# POR QUE ISTO EXISTE: a contagem de tools/aliases e a versao do contrato viviam
# em prosa (docs/MCP.md) e derivaram varias vezes — 23 vs 34 vs 38 vs 40 aliases
# (v1.1.8/1.1.21/1.4.5), tool `decide` ausente da lista, `serverInfo.version`
# atras. Aqui elas sao DERIVADAS das constantes do proprio servidor
# (LISTED_TOOLS / ALIAS_SURFACE / CORE_ALIASES / DEPRECATED_ALIASES /
# MCP_CONTRACT_VERSION): a prosa passa a ser geracao, nao digitacao.
#
# Uso:
#   bash scripts/gen-mcp-surface.sh gen     # regenera os artefatos
#   bash scripts/gen-mcp-surface.sh check   # falha (exit 1) se estiverem stale
set -uo pipefail
cd "$(dirname "$0")/.."

MODE="${1:-check}"
GEN_FILE=docs/generated/mcp-surface.md
MCP_DOC=docs/MCP.md
BEGIN='<!-- BEGIN GENERATED: mcp-surface -->'
END='<!-- END GENERATED: mcp-surface -->'
# shellcheck disable=SC2016
HEADER='<!-- GERADO por scripts/gen-mcp-surface.sh a partir de examples/mcp_server.rs — NAO editar a mao (rode o script) -->'

# SEMPRE rebuilda: um binario velho IGNORA --print-surface e sobe o server
# stdio, bloqueando no stdin (a licao "build ancient = falha fantasma").
BIN=target/release/examples/mcp_server
[ -x "$BIN.exe" ] && BIN="$BIN.exe"
echo "gen-mcp-surface: build do mcp_server (release)..."
cargo build --release --example mcp_server >/dev/null 2>&1 \
  || { echo "gen-mcp-surface: build falhou"; exit 1; }

FRAG="$("$BIN" --print-surface)" \
  || { echo "gen-mcp-surface: --print-surface falhou"; exit 1; }

render_file() { printf '%s\n\n%s\n' "$HEADER" "$FRAG"; }

# Substitui o bloco entre marcadores em docs/MCP.md pelo FRAG atual.
# Escreve bytes (binario) para nao sofrer traducao de \n no Windows.
render_doc() {
  FRAG="$FRAG" BEGIN="$BEGIN" END="$END" python - "$MCP_DOC" <<'PY'
import os, re, sys
path = sys.argv[1]
frag = os.environ["FRAG"]
begin = os.environ["BEGIN"]
end = os.environ["END"]
with open(path, encoding="utf-8", newline="") as f:
    s = f.read()
if begin not in s or end not in s:
    sys.stderr.write("gen-mcp-surface: marcadores BEGIN/END ausentes em %s\n" % path)
    sys.exit(3)
block = begin + "\n" + frag + "\n" + end
out = re.sub(re.escape(begin) + r".*?" + re.escape(end), lambda _m: block, s, count=1, flags=re.S)
sys.stdout.buffer.write(out.encode("utf-8"))
PY
}

case "$MODE" in
  gen)
    mkdir -p docs/generated
    render_file > "$GEN_FILE"
    render_doc > "$MCP_DOC.tmp" && mv "$MCP_DOC.tmp" "$MCP_DOC"
    echo "gen-mcp-surface: regenerado ($GEN_FILE + bloco em $MCP_DOC)"
    ;;
  check)
    fail=0
    tf="$(mktemp)"; render_file > "$tf"
    if ! diff -u "$GEN_FILE" "$tf" >/dev/null 2>&1; then
      echo "STALE: $GEN_FILE desatualizado vs mcp_server (rode: bash scripts/gen-mcp-surface.sh gen)"
      fail=1
    fi
    td="$(mktemp)"; render_doc > "$td" 2>/dev/null
    if ! diff -u "$MCP_DOC" "$td" >/dev/null 2>&1; then
      echo "STALE: bloco gerado em $MCP_DOC desatualizado vs mcp_server (rode: bash scripts/gen-mcp-surface.sh gen)"
      fail=1
    fi
    rm -f "$tf" "$td"
    [ "$fail" -eq 0 ] && echo "gen-mcp-surface: OK (artefatos em dia com as constantes)"
    exit "$fail"
    ;;
  *)
    echo "uso: bash scripts/gen-mcp-surface.sh [gen|check]"
    exit 2
    ;;
esac
