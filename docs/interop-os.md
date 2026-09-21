# Interop — neural-sgdb ↔ neural-os-core (AIOS)

> Origem do substrato: extraído / espelhado a partir do stack **neural-os-core**
> (AIOS). Contrato de fio: **NMD1** + **TKLV** byte-identical. Side-tables
> cognitivas (MDM1) evoluem no standalone sem mudar NMD1/TKLV.

## Premissas que **não** podem partir

| Contrato | Onde |
|----------|------|
| Zero deps na lib; `no_std` + `std` | `Cargo.toml`, gate bare-metal |
| NMD1 layout | `src/memory_doc.rs` — `golden_nmd1_bytes` |
| TKLV / TKCK | `src/tickv.rs` — magic `TKLV`, tombstone `TKL\0` |
| Core não decide; ADD-only | `docs/doctrine.md` |
| Lexical-first MCP | ADR-0008 |

## Evidência de paridade (revalidado 2026-09-18)

Tree OS local de referência: `C:\DEV\neural-os-core-latest` (não
`neural-os-core` sem sufixo).

| Artefacto | Standalone (`neural-sgdb`) | OS |
|-----------|----------------------------|-----|
| NMD1 golden L1 `"k"` / `0xAA` | `src/memory_doc.rs::golden_nmd1_bytes` | `crates/k_ai/src/sgdb/memory_doc.rs::golden_nmd1_bytes_match_neural_sgdb` — **mesmo vetor de bytes** |
| META (MDM1) | `META_VERSION = 7` (scope_dims + model_id) | vendored `crates/neural-sgdb` também `META_VERSION = 7` |
| Crate SemVer vendored | **1.1.23** | **1.1.16** (`crates/neural-sgdb/Cargo.toml`) |

**Conclusão:** o **fio NMD1** (e o espelho TKLV no nano stack) continua o
contrato com o OS. O **lag de produto** (1.1.16→**1.1.23**, 7 releases) afecta
APIs cognitivas / MCP / harness (`commit_run`, null-scoping `ScopeDims`, oráculo
`index_fingerprint`, `recall_adaptive`), **não** o layout NMD1 dos goldens acima —
e é exatamente por isso que o re-sync vendored continua sendo o item de interop
em aberto no `ROADMAP.md`.

## O que sincronizar no OS (quando fizer bump)

1. Subir `crates/neural-sgdb` do OS para o tag/commit standalone desejado **ou**
   path-dep / git submodule — sem fork silencioso de `memory_doc`/`tickv`.
2. Manter o teste `golden_nmd1_bytes_match_neural_sgdb` verde no `k_ai`.
3. Não reintroduzir deps no core; não alterar encode NMD1/TKLV sem ADR + golden
   nos **dois** repos no mesmo release train.

## O que **não** é interop OS

- MCP stdio / Cursor / connectors — host-only.
- `commit_run` / MOM entities — convenção de agente; o OS consome Hits/NMD1.
- DB `.nsgdb/memory.db` partilhado entre projetos IDE ≠ sync CRDT com o kernel.

## Portabilidade multi-projeto (IDE)

| Padrão | OK? |
|--------|-----|
| Workspace `.cursor/mcp.json` com `DEFAULT_SCOPE=project/<este-repo>` | Sim |
| Global `~\.cursor\mcp.json` com `DEFAULT_SCOPE` de **outro** repo + mesmo `NEURAL_SGDB_DB` | **Anti-padrão** — null-scoping “esconde” o projeto aberto |
| Um DB por projeto **ou** scopes sempre explícitos nas tools | Sim |
| Dois writers no mesmo FileStorage | Não |
| Dois DBs / nós → `p2p` / `telepathy_two_db` | Sim |

Ver também: [`harness-prompts.md`](harness-prompts.md), [`MCP-RELOAD.md`](MCP-RELOAD.md).
