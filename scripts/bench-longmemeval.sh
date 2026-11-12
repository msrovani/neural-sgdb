#!/usr/bin/env bash
# Roda o benchmark público LongMemEval-S nos DOIS modos e grava os relatórios
# com proveniência (commit, sha256 do dataset, versão do crate, máquina, data).
#
# Não entra no `gate-all`: precisa de rede (dataset) e de minutos. É o número
# de release de retrieval — rode antes de publicar uma versão que mexa em
# ranking e cole o resultado em docs/BENCHMARKS.md.
#
# Uso:
#   bash scripts/bench-longmemeval.sh                       # S completo, 2 modos
#   bash scripts/bench-longmemeval.sh evals/datasets/longmemeval_oracle
#   bash scripts/bench-longmemeval.sh evals/datasets/longmemeval_s --limit 20
set -euo pipefail
cd "$(dirname "$0")/.."

DATASET="${1:-evals/datasets/longmemeval_s}"
[ $# -gt 0 ] && shift
if [ ! -f "$DATASET" ]; then
  echo "bench-longmemeval: $DATASET ausente — rode 'bash scripts/fetch-longmemeval.sh'"
  exit 1
fi

SHA="$(sha256sum "$DATASET" | cut -d' ' -f1)"
COMMIT="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
STAMP="$(date +%Y%m%d-%H%M%S)"
mkdir -p evals/runs

echo "bench-longmemeval: dataset=$DATASET"
echo "bench-longmemeval: sha256=$SHA commit=$COMMIT"
cargo build --release --example longmemeval
for MODE in component pipeline; do
  echo
  cargo run --release --quiet --example longmemeval -- \
    --dataset "$DATASET" --mode "$MODE" \
    --json "evals/runs/${STAMP}-${MODE}.json" \
    --sha256 "$SHA" --commit "$COMMIT" "$@"
done
echo
echo "bench-longmemeval: relatórios em evals/runs/${STAMP}-*.json"
