#!/usr/bin/env bash
# Docs version gate (P1.1).
#
# Implements the v1.1.23 lesson mechanically: "um bump que nao passa o grep do
# estado antigo deixa a divergencia entrar em silencio". Fails when the version
# BEFORE the current one (previous release) still appears in the LIVE docs —
# README, api contract, architecture, status, codemaps — where it means drift,
# not history. CHANGELOG / hot_test / release notes / ADRs are history and are
# allowed to mention old versions.
#
# Usage: bash scripts/docs-version-gate.sh
set -uo pipefail

cur=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')
# previous released version = the 2nd '## [x.y.z]' heading in CHANGELOG.
prev=$(grep -oE '^## \[[0-9]+\.[0-9]+\.[0-9]+\]' CHANGELOG.md | sed -n '2p' | tr -d '#[] ')
if [ -z "$prev" ]; then
  echo "docs-version-gate: não achei a versão anterior no CHANGELOG"; exit 0
fi
echo "docs-version-gate: current=$cur previous=$prev"

# LIVE docs: version headers here MUST track the current release.
live=(
  README.md
  docs/api.md
  docs/implementation-status.md
  docs/architecture/README.md
  codemap.md
  src/codemap.md
  examples/codemap.md
)
# Escapa o ponto do semver para regex.
pat="${prev//./\\.}"
hits=""
for f in "${live[@]}"; do
  [ -f "$f" ] || continue
  m=$(grep -nE "\b${pat}\b" "$f" || true)
  if [ -n "$m" ]; then
    hits="${hits}${f}:\n${m}\n"
  fi
done

if [ -n "$hits" ]; then
  echo "STALE: versao anterior ($prev) ainda aparece em docs vivos:"
  printf '%b' "$hits"
  echo "Atualize esses docs para $cur (históricos em CHANGELOG/hot_test/adr são ok)."
  exit 1
fi
echo "docs-version-gate: OK (nenhuma referência stale a $prev em docs vivos)"
