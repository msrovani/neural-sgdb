#!/usr/bin/env bash
# Gate total — uma passada, uma matriz.
#
# POR QUE ISTO EXISTE: fechar release era um ritual de oito comandos colados em
# prosa (AGENTS.md, docs/implementation-status.md, README), onde os numeros
# derivam em silencio. E gate que nao roda nao falha: foi o `--shuffle` do CI na
# v1.4.5 (verde sem executar nada) e os 220/0 do hot test contra um binario 1.5.0
# obsoleto (v1.5.6). Aqui a medicao vira ARQUIVO — docs/generated/gate-matrix.json
# — e o proprio hot test compara a matriz (crate, contrato e nenhum gate vazio),
# entao bump sem regerar a matriz falha em vez de passar.
#
# Uso:
#   bash scripts/gate-all.sh          # mede tudo e grava a matriz (gen)
#   bash scripts/gate-all.sh check    # mede tudo e falha se a matriz divergir
#
# Sai 0 sse TODOS os gates passarem E (no modo check) a matriz gravada bater
# byte a byte com a medicao atual.
set -uo pipefail
cd "$(dirname "$0")/.."

MODE="${1:-gen}"
OUT=docs/generated/gate-matrix.json
LOGS="${TMPDIR:-/tmp}/nsgdb-gate"
mkdir -p "$LOGS" docs/generated

CRATE="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')"
CONTRACT="$(sed -n 's/^const MCP_CONTRACT_VERSION: &str = "\([^"]*\)".*/\1/p' examples/mcp_server.rs | head -1)"
if [ -z "$CRATE" ] || [ -z "$CONTRACT" ]; then
  echo "gate-all: nao consegui ler a versao do crate ou do contrato"
  exit 1
fi

FAIL=0
LIB_T=0;  LIB_F=0
P2P_T=0;  P2P_F=0
NOSTD_T=0; NOSTD_F=0
MA_T=0;   MA_F=0
HOT_T=0;  HOT_F=0
BARE=failed; CLIPPY=failed; SURFACE=failed; DOCSV=failed

ok()  { printf '  %-18s %s\n' "$1" "$2"; }
bad() { printf '  %-18s FALHOU (log %s)\n' "$1" "$2"; tail -6 "$2" | sed 's/^/      /'; FAIL=1; }

# Suite de teste no formato do libtest: "test result: ok. N passed; M failed".
run_suite() { # nome logfile comando...
  local name="$1" log="$2"; shift 2
  if "$@" >"$log" 2>&1; then
    G_T="$(sed -n 's/^test result: ok\. \([0-9][0-9]*\) passed.*/\1/p' "$log" | tail -1)"
    G_F="$(sed -n 's/^test result: .* \([0-9][0-9]*\) failed.*/\1/p' "$log" | tail -1)"
    G_T="${G_T:-0}"; G_F="${G_F:-0}"
    ok "$name" "$G_T passed, $G_F failed"
    if [ "$G_T" -eq 0 ]; then
      printf '      GATE VAZIO: nenhuma assercao executada\n'
      FAIL=1
    fi
  else
    G_T=0; G_F=-1
    bad "$name" "$log"
  fi
}

# Hot test: o client NAO fala libtest — imprime "asserções: N total, M falhas".
# Um parser generico aqui leria 0 e a guarda de GATE VAZIO mataria o gate (ou
# pior: passaria com "0 assercoes" no arquivo, que foi o bug desta primeira
# versao — matriz vazia comparada com matriz vazia da "OK").
run_hot() { # logfile
  local log="$1"
  if cargo run --example mcp_client >"$log" 2>&1; then
    HOT_T="$(sed -n 's/^.*: \([0-9][0-9]*\) total, \([0-9][0-9]*\) falhas.*/\1/p' "$log" | tail -1)"
    HOT_F="$(sed -n 's/^.*: [0-9][0-9]* total, \([0-9][0-9]*\) falhas.*/\1/p' "$log" | tail -1)"
    HOT_T="${HOT_T:-0}"; HOT_F="${HOT_F:-0}"
    ok "hot_test" "$HOT_T asserções, $HOT_F falhas"
    if [ "$HOT_T" -eq 0 ]; then
      printf '      GATE VAZIO: nao achei o resumo do hot test no log\n'
      FAIL=1
    fi
  else
    HOT_T=0; HOT_F=-1
    bad "hot_test" "$log"
  fi
}

# `gen` REGENERA: a matriz no repo pode estar stale (bump de versao, contagem
# nova) e o hot test compara com ela — deixar o arquivo velho no lugar faria o
# proprio `gen` falhar por drift, que e justamente o que ele veio consertar.
# A anterior vai para o temp (nao para o repo) e so serve de diff no log.
if [ "$MODE" != "check" ] && [ -f "$OUT" ]; then
  mv "$OUT" "$LOGS/gate-matrix.previous.json"
fi

echo "gate-all: crate=$CRATE contrato=$CONTRACT modo=$MODE"
echo "gate-all: build do mcp_server (o hot test recusa binario mais antigo que o fonte)"
if cargo build --example mcp_server >"$LOGS/build-server.log" 2>&1; then
  ok "build mcp_server" "clean"
else
  bad "build mcp_server" "$LOGS/build-server.log"
  echo "gate-all: ABORTADO — sem servidor fresco o hot test seria um gate falso"
  exit 1
fi

run_suite lib         "$LOGS/lib.log"         cargo test --lib
LIB_T=$G_T;   LIB_F=$G_F
run_suite p2p         "$LOGS/p2p.log"         cargo test --features p2p --lib
P2P_T=$G_T;   P2P_F=$G_F
run_suite no_std      "$LOGS/no_std.log"      cargo test --no-default-features --lib
NOSTD_T=$G_T; NOSTD_F=$G_F
run_suite multi_agent "$LOGS/multi_agent.log" cargo test --test multi_agent
MA_T=$G_T;    MA_F=$G_F
run_hot "$LOGS/hot_test.log"

if cargo check --no-default-features --target x86_64-unknown-none >"$LOGS/bare_metal.log" 2>&1; then
  BARE=clean;  ok bare_metal clean
else
  bad bare_metal "$LOGS/bare_metal.log"
fi
# MESMA invocacao do CI (`--all-targets --all-features`), nao um subconjunto: a
# versao anterior olhava so os dois exemplos do MCP e o `utility_bench` (novo na
# 1.5.6) entrou na main com 4 lints sob `-D warnings` — o gate local dizia
# "clippy clean" e o CI ficou vermelho em 7d5217e. Gate que mede menos que o CI
# nao e um gate mais rapido, e um gate falso.
if cargo clippy --all-targets --all-features -- -D warnings >"$LOGS/clippy.log" 2>&1; then
  CLIPPY=clean; ok clippy clean
else
  bad clippy "$LOGS/clippy.log"
fi
if bash scripts/gen-mcp-surface.sh check >"$LOGS/surface.log" 2>&1; then
  SURFACE=clean; ok gen_mcp_surface clean
else
  bad gen_mcp_surface "$LOGS/surface.log"
fi
if bash scripts/docs-version-gate.sh >"$LOGS/docs.log" 2>&1; then
  DOCSV=clean; ok docs_version_gate clean
else
  bad docs_version_gate "$LOGS/docs.log"
fi

TMP_MATRIX="$LOGS/gate-matrix.json"
cat >"$TMP_MATRIX" <<EOF
{
  "generated_by": "scripts/gate-all.sh",
  "crate": "$CRATE",
  "mcp_contract": "$CONTRACT",
  "checks": {
    "lib": { "command": "cargo test --lib", "total": $LIB_T, "failed": $LIB_F },
    "p2p": { "command": "cargo test --features p2p --lib", "total": $P2P_T, "failed": $P2P_F },
    "no_std": { "command": "cargo test --no-default-features --lib", "total": $NOSTD_T, "failed": $NOSTD_F },
    "multi_agent": { "command": "cargo test --test multi_agent", "total": $MA_T, "failed": $MA_F },
    "hot_test": { "command": "cargo run --example mcp_client", "total": $HOT_T, "failed": $HOT_F },
    "bare_metal": { "command": "cargo check --no-default-features --target x86_64-unknown-none", "result": "$BARE" },
    "clippy": { "command": "cargo clippy --all-targets --all-features -- -D warnings", "result": "$CLIPPY" },
    "gen_mcp_surface": { "command": "bash scripts/gen-mcp-surface.sh check", "result": "$SURFACE" },
    "docs_version_gate": { "command": "bash scripts/docs-version-gate.sh", "result": "$DOCSV" }
  }
}
EOF

# A matriz tem de ser um documento COMPLETO: nove gates, nada de arquivo vazio
# (o bug do "check OK" comparando dois vazios). Se faltar chave, o gate falhou.
if ! grep -q '"checks"' "$TMP_MATRIX" || [ "$(grep -c '"command"' "$TMP_MATRIX")" -ne 9 ]; then
  echo "gate-all: matriz incompleta em $TMP_MATRIX — abortando"
  exit 1
fi

if [ "$FAIL" -ne 0 ]; then
  echo "gate-all: FALHOU — matriz NAO atualizada (candidata em $TMP_MATRIX)"
  exit 1
fi

if [ "$MODE" = "check" ]; then
  if [ ! -s "$OUT" ]; then
    echo "gate-all: $OUT ausente ou vazio — rode 'bash scripts/gate-all.sh' e commite a matriz"
    exit 1
  fi
  if ! diff -u "$OUT" "$TMP_MATRIX" >"$LOGS/matrix.diff" 2>&1; then
    echo "gate-all: STALE — a matriz gravada divergiu da medicao atual"
    sed 's/^/      /' "$LOGS/matrix.diff" | head -30
    echo "      rode: bash scripts/gate-all.sh  (e commite $OUT)"
    exit 1
  fi
  echo "gate-all: OK (todos os gates verdes e matriz em dia: $OUT)"
  exit 0
fi

cp "$TMP_MATRIX" "$OUT"
echo "gate-all: OK (todos os gates verdes; matriz gravada em $OUT)"
