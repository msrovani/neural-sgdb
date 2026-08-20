# 06 — Cognitive API

> Status: **current (v1.1.6)** — the cognitive surface ships in `Sgdb` +
> MCP server (23 tools). **implemented** = code + tests; **remaining** =
> honest gap. All English per repo policy.

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

## 5. Observability (implemented)

| API | Role |
|-----|------|
| `health()` | counts, backend, open conflicts |
| `validate()` | integrity walk |
| `era_report()` | embedding era diagnostic (ADR-0007) |

## 6. MCP surface (implemented — 23 tools)

`cargo run --release --example mcp_server` — JSON-RPC 2.0 stdio, handshake
`2025-11-25`.

**Tools:** remember, remember_episodic, recall, recall_temporal,
recall_entities, rag_context, feedback, diary, profile, expire_old, explain,
reinforce, forget, associate, related_to, contradicts, supersede, conflicts,
resolve_conflict, merge_memories, health, validate, era_report.

**Resources:** `memory://{layer}/{key}` with opaque `nextCursor` pagination.

**v1.1.6 extras:**
- `remember(type=)` — write-side content type (MDM1 v6)
- `recall(format=json)` — structured typed hits for machine consumers
- `recall(mode=)` — semantic | lexical | hybrid
- `rag_context(rerank=, mode=, format=)`

**Embedder:** `DemoEmbedder` (trigram hash) — NOT semantic. Agent supplies
`embedding` in payload; same model on write and query (S1 guard).

Hot test: `mcp_client` — **90/0** checks (v1.1.6).

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
