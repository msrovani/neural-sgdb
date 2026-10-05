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

# Python do CI (ubuntu) e o do Windows nem sempre tem o mesmo nome.
PY=python3
command -v python3 >/dev/null 2>&1 || PY=python

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
run "target-none"   cargo check --no-default-features --target x86_64-unknown-none
run "host-crates"   cargo check --manifest-path crates/nsgdb-embed/Cargo.toml
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