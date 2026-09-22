# 06 — Cognitive API

> Status: **current (v1.1.26)** — the cognitive surface ships in `Sgdb` +
> MCP server (**4 tools**, aliases for the 23 legacy names). **implemented** =
> code + tests; **remaining** = honest gap. All English per repo policy.

## 1. Principle

The API speaks **memory verbs**, not storage verbs:

```text
Storage trait:  put / get / scan_prefix / delete     (backend ABI)
Sgdb:           remember / recall / associate / reinforce / supersede / …
MCP (examples): JSON-RPC tools mirroring Sgdb + observability
```

The core **does not decide** — it supplies ranked, typed, provenance-bearing
material for the agent/LLM above.

## 2. Write surface (implemented)

| Verb | Layer | Notes |
|------|-------|-------|
| `remember_exchange` | L1+L2 | RAM until checkpoint |
| `remember_episodic` | L2 | verbatim user/response pairs (v1.1.4) |
| `remember_text_with` | L3 | lexical write, no BQ era (v1.1.10) |
| `remember_semantic` | L4+L2 companion | era guard on live corpus |
| `remember_fact` | L3 | timestamped |
| `set_importance` / `set_confidence` | meta | clamped [0,1] |
| `set_scope` / `set_entities` / `set_content_type` | meta | MDM1 v4–v6 seams |
| `set_validity` / `invalidate` | validity | invalidate-not-delete |
| `associate` / `associate_checked` | L6 | relations in sys/rel/ |

## 3. Read surface (implemented)

| Verb | Path | Notes |
|------|------|-------|
| `recall` | semantic | BQ + FP32, typed Hit |
| `recall_lexical` / `recall_hybrid` | lexical/hybrid | BM25 + optional semantic |
| `recall_entities` | entities | 1-hop declared strings |
| `recall_temporal` | semantic+time | bi-temporal intent |
| `recall_weighted` | semantic | recency + importance |
| `recall_scoped` / `_historical` | all | multi-agent scoping |
| `rag_context` / `_reranked` / `_limited` | RAG | byte-capped prompt block |
| `scan_prefix` / `_page` | symbolic | ART |
| `diary` / `profile` | episodic/profile | agent-scoped views (v1.1.4) |

## 4. Lifecycle and cognition (implemented)

| Verb | Role |
|------|------|
| `supersede` | history-preserving update |
| `reinforce` / `feedback` | importance/confidence |
| `forget` | archive |
| `delete` | physical removal |
| `explain` | provenance narrative |
| `transfer_to` / `merge_memories` | layer move / fusion |
| `conflicts` / `resolve_conflict` / `dismiss_conflict` | conflict model |
| `expire_old` | validity sweep |
| `MemoryLifecycle::tick` | deterministic promotion/decay |
| `commit_run` / `deprecate_run` | ADR-0010 harness flush / soft-close run |
| `consolidate_recurrences_scoped` | recurrence consolidate filtered by ScopeFilter |

## 5. Observability (implemented)

| API | Role |
|-----|------|
| `health()` | counts, backend, open conflicts, and the open-cost contract (`opens`, `open_rebuild_ms_last`/`_max`, ADR-0009 §4) |
| `validate()` | integrity walk — incl. §5: `corpus_mean` counts (exact) + sums (relative tolerance `1e-9`, because `f64` addition is not associative) |
| `era_report()` | embedding era diagnostic (ADR-0007) |
| `index_fingerprint()` | oracle of the DERIVED state, `fp(open) == fp(rebuild_indices())` (ADR-0011) — O(n log n), so never in the default `health`; MCP `health(view=index)` |
| `recall_adaptive()` | report *why* a recall stopped escalating (`oversample_used`/`escalations`/`boundary_decisive`/`probe`, ADR-0012) |

## 6. MCP surface (implemented — 4 tools + aliases)

`cargo run --release --example mcp_server` — JSON-RPC 2.0 stdio, handshake
`2025-11-25`. `serverInfo.version` = `MCP_CONTRACT_VERSION` = **1.1.26**, e o
`examples/mcp_client.rs` é pinado no MESMO commit — o hot test falha alto se
divergirem.

**Listed tools:** `remember`, `recall`, `health`, `curate`. Dispatch by args
(`user+response` → episodic L2; `entities` / `at` / `rag=true`;
`health(view=status|validate|era|tensions|staleness|index)`; `curate.op=…`).
The previous names remain valid aliases on `tools/call` (the alias table lives in
`ALIAS_SURFACE` and is pinned by test — prose drifts, the table does not).

**Default retrieval (ADR-0008):** `mode=lexical` when the caller does not pass
`embedding=`. Semantic/hybrid require a caller vector or explicit
`NEURAL_SGDB_EMBEDDER=demo`. `remember(text=)` without a vector writes **L3**
(`Sgdb::remember_text_with`) and does not open a fake BQ era.

**Resources:** `memory://{layer}/{key}`, `nsgdb://doctrine`, **`nsgdb://session`**
(cold-start JSON). Opaque `nextCursor` pagination.

**v1.1.6 extras (still current):**
- `remember(type=)` — write-side content type (MDM1 v6)
- `recall(format=json)` — structured typed hits for machine consumers
- `recall(mode=)` — semantic | lexical | hybrid (default lexical)
- `rag_context(rerank=, mode=, format=)` via `recall(rag=true)` or alias

**Embedder:** unset host embedder = none. `DemoEmbedder` (trigram) is **not**
the product default — set `NEURAL_SGDB_EMBEDDER=demo` only if requested.
Same model on write and query (S1 guard).

Hot test: `mcp_client` — **84/0** checks (v1.1.10).

## 7. Machine→machine contract (v1.1.6)

`examples/two_ai_protocol.rs` — 16/16 checks: writer declares types,
reader consumes typed hits (`content_type`, `payload_type`, `rel`,
`matched_terms`) without `from_utf8_lossy` on binary/embedding payloads.

## 8. Agent protocols (examples, not core)

| Example | Role |
|---------|------|
| `agent_protocol.rs` | decision discipline (entities, facts, two-pass, P1–P6) |
| `memory_arena_eval.rs` | utility eval (protocol v2 vs naive hoarder) |
| `two_ai_protocol.rs` | typed hit consumption |
| `embedder_http.rs` | real HTTP Embedder seam |

## 8b. Escopo autoritativo + ledger de negativos (v1.1.24)

**Descoberta de escopo: o que `scope=` alcanca e o que so as dims alcancam**
([ADR-0015](../adr/0015-scope-discovery-probes-and-announced-surface.md), v1.1.26).
Um doc gravado so com `run`/`agent`/`app` tem `scope == ""` (o legado espelha
`user`) e NAO e global: o recall global o filtra por null-scoping. Antes do
v1.1.26 ele era contado como global (`global_memory_count` mentia), ficava fora
 de `scope_labels`/`scopes_to_probe`, e nao havia rota ate ele — a funcao que o
enxergava (`scope_distribution_dims`) nao era exposta. Agora `Sgdb::scope_probes`
devolve `legacy` (por label) e `dims_only` (so por filtro multi-dim), o
`nsgdb://session` publica `scopes_to_probe_dims`, e o `recall` do MCP roteia para
as variantes `_dims` quando qualquer dim e passada. Regra de ouro: **a label nao
identifica a rota** (um `scope` legado pode conter `/`), entao a procedencia vem
do core, nao de parse.

**`ScopeDims` é autoritativo; o `scope` legado é o espelho de `user`**
([ADR-0013](../adr/0013-scope-dims-is-authoritative.md)). Os dois mecanismos já
concordavam em toda LEITURA (o decode promove o legado → `user` quando as dims
vêm vazias, e `effective_scope_dims` faz o mesmo nos filtros de recall); o
desacordo morava nos BYTES gravados. Agora `set_scope` faz write-through nos
dois campos, o `sys/meta/` fica canônico e `validate` §6 sinaliza
`legacy scope disagrees with scope_dims.user` em meta divergente (só alcançável
por escrita externa/import). Não há bump de MDM1: é canonicalização de escrita,
não reinterpretação de bytes.

**Ledger de negativos** ([ADR-0014](../adr/0014-negative-ledger.md)) —
"o que já procurei e não estava lá", o complemento do `mom/anti-pattern`
(que é uma *memória positiva* com lição negativa). Side-table
`sys/negative/<fnv1a64(scope‖0x1f‖query):016x>`:

- identidade = tokens do BM25 (caixa/pontuação não fragmentam; acento e
  paráfrase sim — mesma limitação do índice lexical);
- **escopado**: sem `scope` só as ausências globais aparecem (null-scoping);
- `note_absence` **reforça** (`probes++`), não duplica;
- `recall_with_ledger` = probe lexical **e** ledger numa chamada, com
  **self-healing** (achou memória ⇒ remove a ausência obsoleta);
- `prune_absences` é a retenção do host (`0` desliga); nunca é chamada pelo
  core por conta própria.

O `recall` default **não** tem efeito colateral — mutar uma leitura só acontece
por API cujo nome diz que escreve (a mesma postura do ADR-0012 quanto a mudar a
ordem de um recall por conta própria).

## 9. Remaining gaps

- **`gc()` public verb** — compaction exists; no high-level GC report API.
- **`consolidate()` alias** — use `MemoryLifecycle::tick` directly.
- **Relation inference** — deliberate non-goal (upper layer asserts).
- **MCP transfer tool** — use p2p examples (`p2p_telepathy`, `mesh_simulation`).

## 10. Relationship to other docs

- Doc 01 — Memory Model: fields behind verbs
- Doc 02 — Lifecycle: tick + supersede semantics
- Doc 03 — Retrieval: recall modes + typed hits
- Doc 04 — Distributed: export/import/merge_remote
- Doc 05 — Storage: checkpoint + validate

See also: [`docs/api.md`](../api.md) (full contract),
[`examples/codemap.md`](../../examples/codemap.md) (runnable demos).
