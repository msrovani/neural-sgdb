#!/usr/bin/env bash
# Gate de release — a matriz completa, LOCAL, antes de dar push.
#
# POR QUE ISTO EXISTE (v1.4.5): nesta release, tres gates do CI estavam
# vermelhos e ninguem tinha rodado —
#   1. `cargo test --lib -- --shuffle` (job novo) quebrava em TODA execucao;
#   2. `scripts/docs-version-gate.sh` reprovou o release ja fechado;
#   3. 5 lints novos do clippy travavam `-D warnings`.
# Os tres so apareceram quando alguem rodou a coisa na mao. A licao nao e
# "rode os testes" — e que gate que NINGUEM roda nao e gate, e o lugar que
# dispara "rode os testes" precisa ser um comando, nao uma boa intencao.
#
# Uso:
#   bash scripts/release-gate.sh              # tudo
#   SKIP_HOT=1 bash scripts/release-gate.sh   # sem o hot test (o lento)
#
# Sai != 0 se QUALQUER gate falhar, e imprime o resumo de todos (nao para no
# primeiro) — quem fecha release precisa da lista inteira de quebras.
set -uo pipefail

cd "$(dirname "$0")/.."

RED=$'\033[31m'; GREEN=$'\033[32m'; DIM=$'\033[2m'; OFF=$'\033[0m'
declare -a RESULTS=()
FAILED=0

run() {
  local name="$1"; shift
  local log=".release-gate.${name//\//_}.log"
  printf '%s>>> %s%s\n' "$DIM" "$name" "$OFF"
  if "$@" > "$log" 2>&1; then
    RESULTS+=("ok   $name")
    printf '%sok%s   %s\n' "$GREEN" "$OFF" "$name"
  else
    local e=$?
    RESULTS+=("FAIL $name (exit $e) -> $log")
    FAILED=1
    printf '%sFAIL%s %s (exit %s) -> %s\n' "$RED" "$OFF" "$name" "$e" "$log"
    tail -15 "$log" | sed 's/^/     | /'
  fi
}

# Alvos de cross-compile: o gate depende de `x86_64-unknown-none` (contrato
# no_std) e `wasm32-unknown-unknown` (o backend WASM do host crate). Numa
# maquina limpa os dois passos falhariam com um erro de CARGO, que e o
# diagnostico errado para "target faltando" — a mesma clase de bug que o
# conector pinado. `rustup target add` e idempotente: aqui e instantaneo.
ensure_target() {
  local t="$1"
  if rustup target list --installed 2>/dev/null | grep -qx "$t"; then
    return 0
  fi
  if command -v rustup >/dev/null 2>&1; then
    printf '%s>>> instalando target %s%s\n' "$DIM" "$t" "$OFF"
    rustup target add "$t" >/dev/null 2>&1 \
      || { printf '%sFAIL%s nao consegui instalar o target %s\n' "$RED" "$OFF" "$t"; return 1; }
  else
    printf '%sSKIP%s target %s (sem rustup no PATH)\n' "$DIM" "$OFF" "$t"
    return 2
  fi
}

# Python do CI (ubuntu) e o do Windows nem sempre tem o mesmo nome.
# python3 pode ser o alias quebrado da Microsoft Store no Windows (exit 49,
# imprime aviso e nao roda): so confiar se --version funcionar; senao python.
PY=python3
if ! "$PY" --version >/dev/null 2>&1; then PY=python; fi
command -v "$PY" >/dev/null 2>&1 || PY=python

echo "== neural-sgdb release gate =="
run "docs-version"  bash scripts/docs-version-gate.sh
run "clippy"        cargo clippy --all-targets --all-features -- -D warnings
run "lib"           cargo test --lib
run "lib-serial"    cargo test --lib -- --test-threads=1
run "p2p"           cargo test --features p2p
run "no-std-tests"  cargo test --no-default-features
run "integration"   cargo test --test multi_agent
run "goldens"       cargo test --lib golden
run "wire-fuzz"     cargo test --release --lib wire_fuzz -- --nocapture
run "rustdoc"       env RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
ensure_target x86_64-unknown-none \
  || RESULTS+=("SKIP target-none (target x86_64-unknown-none ausente)")
run "target-none"   cargo check --no-default-features --target x86_64-unknown-none
run "host-crates"   cargo check --manifest-path crates/nsgdb-embed/Cargo.toml
ensure_target wasm32-unknown-unknown \
  || RESULTS+=("SKIP host-wasm32 (target wasm32-unknown-unknown ausente)")
run "host-wasm32"   cargo check --manifest-path crates/nsgdb-wasm/Cargo.toml \
                        --features wasm --target wasm32-unknown-unknown
run "connectors"    "$PY" -m unittest discover -s connectors/tests

if [ "${SKIP_HOT:-0}" != "1" ]; then
  # O hot test precisa do server REBUILDADO (build ancient = falha fantasma).
  run "build-server" cargo build --release --example mcp_server
  if [ -x ./.nsgdb/bin/mcp_server.exe ] || [ -x ./target/release/examples/mcp_server ]; then
    run "hot-test" cargo run --release --example mcp_client
  else
    RESULTS+=("SKIP hot-test (binario nao encontrado apos o build)")
    printf '%sSKIP%s hot-test\n' "$DIM" "$OFF"
  fi
else
  RESULTS+=("SKIP hot-test (SKIP_HOT=1)")
fi

echo
echo "== resumo =="
for r in "${RESULTS[@]}"; do
  case "$r" in
    ok*)  printf '%s%s%s\n' "$GREEN" "$r" "$OFF" ;;
    FAIL*) printf '%s%s%s\n' "$RED" "$r" "$OFF" ;;
    *)    printf '%s%s%s\n' "$DIM" "$r" "$OFF" ;;
  esac
done
echo
if [ "$FAILED" -eq 0 ]; then
  echo "${GREEN}release gate: TUDO VERDE${OFF}"
else
  echo "${RED}release gate: FALHOU (ver resumo acima)${OFF}"
fi
exit "$FAILED"