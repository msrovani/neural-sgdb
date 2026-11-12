#!/usr/bin/env bash
# Baixa o LongMemEval (v1) para evals/datasets/ e VERIFICA o sha256.
#
# Por que sha256 fixo: dataset público muda (upstream drift) e um número
# publicado deixa de ser reproduzível em silêncio. Falha alto é o ponto.
#
# Uso:
#   bash scripts/fetch-longmemeval.sh            # oracle (15 MB) + s (278 MB)
#   bash scripts/fetch-longmemeval.sh oracle     # só o oracle
#   bash scripts/fetch-longmemeval.sh s          # só o S
set -euo pipefail
cd "$(dirname "$0")/.."

DEST=evals/datasets
BASE="https://huggingface.co/datasets/xiaowu0162/longmemeval/resolve/main"
ORACLE_SHA="821a2034d219ab45846873dd14c14f12cfe7776e73527a483f9dac095d38620c"
S_SHA="08d8dad4be43ee2049a22ff5674eb86725d0ce5ff434cde2627e5e8e7e117894"

mkdir -p "$DEST"

fetch() { # nome sha
  local name="$1" want="$2" path="$DEST/$name"
  if [ -f "$path" ]; then
    local have
    have="$(sha256sum "$path" | cut -d' ' -f1)"
    if [ "$have" = "$want" ]; then
      echo "  $name ok (sha256 confere)"
      return
    fi
    echo "  $name com sha256 divergente — rebaixando"
    rm -f "$path"
  fi
  echo "  baixando $name ..."
  curl -fsSL -o "$path" "$BASE/$name"
  local have
  have="$(sha256sum "$path" | cut -d' ' -f1)"
  if [ "$have" != "$want" ]; then
    echo "  ERRO: sha256 de $name divergiu do pinado"
    echo "    esperado $want"
    echo "    recebido $have"
    echo "  O upstream mudou. Número publicado com o pin antigo não vale mais:"
    echo "  reavalie, atualize o pin aqui e registre a mudança em docs/BENCHMARKS.md."
    exit 1
  fi
  echo "  $name ok ($(du -h "$path" | cut -f1))"
}

case "${1:-all}" in
  oracle) fetch longmemeval_oracle "$ORACLE_SHA" ;;
  s)      fetch longmemeval_s "$S_SHA" ;;
  all)    fetch longmemeval_oracle "$ORACLE_SHA"; fetch longmemeval_s "$S_SHA" ;;
  *)      echo "uso: bash scripts/fetch-longmemeval.sh [oracle|s|all]"; exit 2 ;;
esac
