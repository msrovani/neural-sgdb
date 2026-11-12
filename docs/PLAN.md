# Plan — neural-sgdb

Incremental improvement plan for the `neural-sgdb` crate. Written for the next
implementer: only approved phases are listed, with enough detail to resume
without re-deriving the reasoning.

This repository **is** the standalone crate (`https://github.com/msrovani/neural-sgdb`).
The crate README is the entry point; this file is the plan behind it.

## Status

- Release line: **v1.5.7** (crate `version` in `Cargo.toml`, MCP contract
  `1.5.7`). `1.5.7` = fase 1 do plano de ideias copiadas (benchmark público
  LongMemEval-S). `1.5.5` entregou as fases 1-5; `1.5.6` fecha o que o uso real
  apontou: `recall(path=)` honra scope, envelope único de erro nas recusas de
  `path`, gate de build no hot test, `DEFAULT_SCOPE` consistente e
  `recall(all_scopes=true)` (cold-start em 1 chamada).
- Sensitive-memory host policy (ADR-0021) is **implemented** in
  `connectors/`: policy metadata pass-through, sensitive recall controls,
  third-party write logging, and the scenario test in
  `connectors/tests/test_contract.py`. Docs: `docs/sensitive-agent-memory.md`.
- **Phase 1 (Contract reliability)** is **implemented**: recall failures are no
  longer converted silently to empty results; all MCP tool errors use a uniform
  structured envelope (`isError` set by the call site, never inferred from text)
  with `structuredContent` carrying error codes/data; `recall`/`rag_context`
  propagate storage/corruption errors as typed failures; responses distinguish
  empty result vs applied filter vs route/modality via `recall` metadata
  (`status`/`mode`/`route`/`filter_applied`/`filter`); contract coverage added
  for all modes and storage failure paths including `rag_context` (hot test
  updated). Tool counts and MCP/cold-start instructions aligned with `1.5.0`.
- **Phase 2 (Functional onboarding)** is **implemented**: cold-start ritual
  (`nsgdb://session`) now states concrete prerequisites for triggers, entities
  and scope; explicitly documents that `recall(path=...)` does **not** create
  rules nor extract entities (rules only via `curate op=set_trigger`), reports
  empty with reason when no rule matches (never global pool), and end-to-end
  path is covered; recommended entity format (`kind/name`, canonical strings)
  documented per application. `docs/MCP.md` aligned.
- **Phase 3 (Real utility measurement)** is **implemented**: benchmark
  `examples/utility_bench.rs` measures decision quality (not hit count) across
  exact questions, paraphrases, conflicting evidence, scopes and temporal
  evolution; compares lexical/semantic/hybrid/path; records latency and
  candidate volume alongside quality; covers at least two domains (infra/ops,
  produto/docs). Results show scope isolation and mode comparison.
- **Phase 4 (Maintenance and operations policy)** is **implemented**: host
  presets documented (decay/expire_old/consolidate/audit/checkpoint/GC) with
  `examples/host_scheduler.rs`; durability/recovery semantics per backend
  documented in `docs/MCP.md` (InMemory/FileStorage/TickvFile, retention
  rules); churn/compaction and cold-open measurements added to
  `BENCHMARKS.md`; snapshot auto remains **opt-in by default**
  (`NEURAL_SGDB_INDEX_SNAPSHOT=off`) with metrics-gated auto-persist when
  enabled.
- **Phase 5 (Distribution)** is **addressed/documented**: distribution gaps
  (tombstone/purge propagation, amnesiac-peer recovery, transport identity/
  authentication, partial topologies/prolonged partitions) documented with
  current constraints and seams; kept behind explicit requirement as intended.
- The 5-phase plan below is approved; Phase 1 executed, Phase 2 executed,
  Phase 3 executed, Phase 4 executed, Phase 5 addressed, sensitive-memory executed.

## Approved plan

1. **Contract reliability** — eliminate silent conversion of recall failures
   into empty results, make MCP errors uniform and structured (no
   `isError` inferred from text), contract tests covering every mode and
   storage failure including `rag_context`, tool counts and MCP/cold-start
   instructions aligned with `1.5.0`, and responses that distinguish empty
   result vs applied filter vs route/modality used.
2. **Functional onboarding** — cold-start ritual with concrete prerequisites
   for triggers, entities and scope; make explicit that `recall(path)` does
   not create rules nor turn text into entities; verify the end-to-end path
   (memory with entity → trigger → path query → scope does not leak → no-rule
   case reports the reason); define a recommended entity format per application.
3. **Real utility measurement** — benchmark of exact questions, paraphrases,
   conflicting evidence, scopes and temporal evolution; compare lexical,
   semantic, hybrid and path on *decision quality*, not hit count; record
   latency and candidate volume alongside quality; corpora from at least two
   domains (real anonymizable corpus or representative tasks).
4. **Maintenance and operations policy** — **implemented**: host presets
   for decay, expire_old, consolidation, audit/checkpoint and GC documented
   (`examples/host_scheduler.rs`, `docs/MCP.md`); durability/recovery semantics
   per backend documented in the MCP config; churn/compaction and cold-open
   on durable DBs measured in `BENCHMARKS.md`; snapshot auto remains opt-in
   by default.
5. **Distribution (only if a product requirement)** — **addressed/documented**:
   tombstone/purge propagation path documented (see `docs/architecture/04-distributed-memory.md` §7; `forget_purge` writes audit evidence and storage tombstone; replication of tombstones via `MemoryDelta`/side-metadata tracked), amnesiac-peer recovery constraints documented, transport identity/authentication seams present (`SignedEnvelope`, `Signer`/`TrustStore` traits), partial topologies and prolonged partitions noted with reconciliation behavior; local use remains primary path.

## Sensitive-memory policy (executed — ADR-0021)

The core stays a memory substrate: it never interprets content. Everything in
ADR-0021 lives in the host/connector layer:

- `MemoryConnector.store(...)` accepts optional policy metadata
  (`memory_category`, `subject_type`, `requires_consent`, `retention_rule`)
  and forwards it without core interpretation.
- `RecallPolicy` gains `only_own_scope` / `allow_cross_scope` /
  `block_sensitive_recall` / `vulnerable_context` / `warn_on_sensitive`, so
  the host can refuse cross-scope recall and warn in vulnerable contexts.
- Sensitive writes are logged lightly (host audit); `forget` is the hook for
  consent revocation and retention expiry.
- `connectors/tests/test_contract.py` pins the scenario: sensitive write →
  out-of-scope recall filtered → vulnerable-context warning present.


## Ideias copiadas do ai-memory (aprovado 2026-11-12)

Origem: estudo de `github.com/akitaonrails/ai-memory` (v3.3.x) — comparação em
`docs/PLAN.md` §"O que é melhor que o nsgdb" registrada na sessão. Só entram os
itens que sobrevivem ao critério deste repo: medem algo, não quebram `no_std`
nem a paridade byte-exata, e o gate prova que falham quando devem falhar.

**Versionamento (revisado pelo dono do repo):** a linha continua em **1.5.x**;
o sexto item vira **1.6.0**. Cinco fases em 1.5.7–1.5.11, a sexta em 1.6.0.

| # | fase | release | critério de aceite |
|---|---|---|---|
| 1 | Benchmark público LongMemEval-S | **1.5.7** | `hit@5`/`recall@5` medidos em modo `component` e `pipeline`, com proveniência (commit, sha256 do dataset, máquina, data), por fatia, reproduzível por comando documentado |
| 2 | `explain` por stream no recall híbrido | **1.5.8** | `Hit.rrf` com rank+contribuição por stream; default byte-idêntico; mutação provada |
| 3 | Escrita condicional (`expect_version`/`create_only`) | **1.5.9** | recusa `precondition_failed` com `expected` e `current`; nenhuma escrita parcial; mutação provada |
| 4 | Feed incremental (`changes_since` + cursor) | **1.5.10** | upsert **e delete** visíveis; cursor estável e re-leitura sem repetição |
| 5 | Retenção por tier + breadth | **1.5.11** | λ por camada com default bit-idêntico ao escalar; só executar se a fase 1 mostrar que aging importa |
| 6 | Captura por hooks (`nsgdb-hook`) | **1.6.0** | hook grava → recall acha; orçamento de 200 ms; spool quando o DB está ocupado |

**Fora de escopo, por decisão:** markdown como fonte da verdade (quebraria
`no_std` e a paridade byte-exata; o princípio "índice derivado e reconstruível"
já existe no `index_fingerprint`) e auth multi-usuário (não há servidor).

### Regras que cada fase tem de respeitar

- `bash scripts/gate-all.sh gen && bash scripts/gate-all.sh check` verdes, com a
  matriz regerada no mesmo commit; contagem que mudar é atualizada em prosa.
- Pin de contrato no **mesmo commit** quando a superfície MCP muda:
  `MCP_CONTRACT_VERSION`, `contract.json`, `examples/mcp_client.rs`,
  `connectors/mcp_client/client.py` (`EXPECTED_CONTRACT`), `docs/MCP.md`,
  `docs/generated/mcp-surface.md`.
- Teste de fix/comportamento novo só vale com **mutação provada**.
- Arquivos CRLF: `encoding='utf-8', newline=''` em qualquer edição por script.
- O benchmark (fase 1) **não** entra no `gate-all`: precisa de rede e de
  minutos. Ele vive em `scripts/bench-longmemeval.sh`.


## How to continue

- Read `docs/PLAN.md` (here) first, then `docs/MCP.md` and
  `docs/implementation-status.md`.
- On resume after a session break, confirm: which phase was last active,
  whether the MCP contract was changed (pins move together:
  `contract.json`, `examples/mcp_client.rs`, `connectors/mcp_client/client.py`,
  `docs/MCP.md`), and whether a failing check was fixed or only annotated.
- Then continue the smallest unfinished phase, not the whole plan at once.
- `docs/generated/mcp-surface.md` is generated by `scripts/gen-mcp-surface.sh`
  from server constants — never hand-edit it.
