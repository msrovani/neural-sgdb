# neural-sgdb — Implementation Status

> **Current snapshot (2026-09-18, v1.1.20).** Capability matrix vs the shipped
> codebase. For the public contract see [`docs/api.md`](api.md); for architecture
> narrative see [`docs/architecture/README.md`](architecture/README.md).

## Status labels

| Label | Meaning |
|---|---|
| **IMPLEMENTED** | Shipped, exercised by tests, in the public API. |
| **PARTIAL** | Core exists; a stated requirement remains open. |
| **EXPERIMENTAL** | Works but demo-grade / not production-safe. |
| **REMAINING** | Honest gap on the roadmap — not a bug. |

## Verification matrix (current)

| Check | Command | Result |
|---|---|---|
| Default tests | `cargo test --lib` | **292** |
| P2P tests | `cargo test --features p2p --lib` | **338** |
| no_std tests | `cargo test --no-default-features --lib` | **244** |
| no_std target | `cargo check --no-default-features --target x86_64-unknown-none` | **ok** |
| Hot test (MCP) | `cargo run --release --example mcp_client` | **100/0 exit 0** |
| AI-user sim | `cargo run --release --example agent_sim` | loop real, scope isolado |
| Machine protocol | `cargo run --release --example two_ai_protocol` | **16/16 exit 0** |
| Agent protocol | `cargo run --release --example agent_protocol` | **23/23 exit 0** |
| Clippy / doc gates | `-D warnings` | green |

## Capability matrix

| Capability | State | Evidence |
|---|---|---|
| MemoryDoc NMD1 | IMPLEMENTED | `src/memory_doc.rs`, golden tests |
| MDM1 v6 side-table meta | IMPLEMENTED | scope, entities, content_type, version_id, … |
| MDM1 v7 scope_dims+model_id (v1.1.14/15) | IMPLEMENTED | `ScopeDims{user,agent,app,run}`, `model_id`, `mixed_models` verdict |
| Harness commit_run (ADR-0010, v1.1.19) | IMPLEMENTED | `src/harness.rs`: `commit_run`/`deprecate_run`/`consolidate_recurrences_scoped`/`remember_episodic_scoped`; MCP `curate` ops; `mom/anti-pattern` |
| Null-scoping ScopeDims (v1.1.20) | IMPLEMENTED | `allows_scope_filter`; `recall_*_dims` pool; consolidate herda dims; 14 harness tests |
| Hybrid RRF + rerank seam (v1.1.14) | IMPLEMENTED | `recall_hybrid_rrf`, `Reranker`, `recall_reranked` |
| Local embedder crate (v1.1.14) | IMPLEMENTED | `nsgdb-embed` 384d, `LOCAL_MODEL_ID` |
| TTL per-key + GC (v1.1.15) | IMPLEMENTED | `sys/ttl/`, `expire_ttl`, `GcConfig/collect_garbage` |
| Temporal event timeline (v1.1.15) | IMPLEMENTED | `sys/event/`, `set_event/close_event/recall_timeline` |
| ANN IVF-Flat + HNSW-lite (v1.1.15) | IMPLEMENTED | `src/ann.rs`, `recall_ann_ivf`, no_std zero-dep |
| Snapshot/WASM seam (v1.1.15) | IMPLEMENTED | `SnapshotStorage`, `src/wasm_storage.rs`, `examples/wasm_backend.rs` |
| L0–L7 layers | IMPLEMENTED | `MemoryLayer`, layer-aware merge policy |
| L6 associative memory | IMPLEMENTED | `sys/rel/`, `associate`/`related_to`/… |
| MemoryState lifecycle | IMPLEMENTED | `sys/state/`, active-only recall default |
| Temporal validity | IMPLEMENTED | `sys/validity/`, `recall_temporal`, `expire_old` |
| Dynamic VectorClock | IMPLEMENTED | 8-node + overflow; NMD1 72B unchanged |
| Provenance / identity | IMPLEMENTED | `memory_id`, `Hit.provenance`, MDM1 |
| ART index | IMPLEMENTED | prefix guard, rebuild, delete reclaim |
| BQ + FP32 recall | IMPLEMENTED | oversample, heap, MihIndex, era guard |
| ADC-lite dual-path (v1.1.17) | IMPLEMENTED | `corpus_mean`, `bq_top_k_f32_dual`, `sign(q−mean)`; state-first ties |
| Lexical BM25 | IMPLEMENTED | L2/L3, matched_terms, scoped |
| Entity 1-hop recall | IMPLEMENTED | `entity_index`, exact string match |
| Typed hits (v1.1.6) | IMPLEMENTED | `ContentType`, `RecallPath`, MCP json |
| Scoping multi-agent | IMPLEMENTED | MDM1 v4, scoped recall variants |
| Storage trait + backends | IMPLEMENTED | InMemory, FileStorage, TickvFile |
| Durability levels | IMPLEMENTED | `Durability` enum, `sync_durable` |
| CRDT + record replication | IMPLEMENTED | MDR1, MDLT, anti-entropy (p2p) |
| Conflict model | IMPLEMENTED | CFL1, resolve/dismiss |
| MemoryLifecycle tick | IMPLEMENTED | promote/decay/archive deterministic |
| Cognitive API | IMPLEMENTED | reinforce, supersede, explain, merge, … |
| Ebbinghaus decay (v1.1.10) | IMPLEMENTED | `decay_importance`, state `Decayed`, idempotent per `now` |
| Recurrence consolidation (v1.1.10) | IMPLEMENTED | `consolidate_recurrences`, deterministic L3, lineage |
| Derived-index oracle (v1.1.21) | IMPLEMENTED | ADR-0011: `index_fingerprint` (`fp(open)==fp(rebuild)`), `health(view=index)`; ids/órfãos/floats fora |
| `corpus_mean` invariant (v1.1.21) | IMPLEMENTED | `validate` §5: counts exatos + somas com tolerância relativa `1e-9` |
| Open cost metrics (v1.1.21) | IMPLEMENTED | ADR-0009 §4: `open_rebuild_ms_last`/`_max`/`opens` no core, health e bench |
| MCP alias surface (v1.1.21) | IMPLEMENTED | `ALIAS_SURFACE` (34) + `did_you_mean` no erro de tool desconhecida |
| Score breakdown (v1.1.10) | IMPLEMENTED | `recall_weighted_full`, `Hit.score_breakdown`, trust weights |
| Audit hash-chain (v1.1.10) | IMPLEMENTED | `sys/audit/` (AUD1), `audit_verify`, `rollback_to` |
| Write-path hardening (v1.1.10) | IMPLEMENTED | `validate_written` on all write seams |
| Host scheduler (v1.1.11) | IMPLEMENTED | `examples/host_scheduler.rs` (expire/decay/consolidate/audit) |
| Backfill helper (v1.1.11) | IMPLEMENTED | `examples/backfill_helper.rs` (L3→L4 re-embed + rebuild) |
| Storage batch (v1.1.11) | IMPLEMENTED | `Storage::put_many` + `FileStorage::put_batch` (1 write por remember_exchange) |
| Lexical fast (v1.1.11) | IMPLEMENTED | `LexicalIndex::search_fast` (dedup, sem matched_terms) |
| Recall heap (v1.1.11) | IMPLEMENTED | `recall_weighted_full` select_nth_unstable |
| Arbitration policy seam | IMPLEMENTED | `ArbitrationPolicy`, no LLM in core |
| Embedder seam | IMPLEMENTED | trait + DemoEmbedder + HTTP example |
| MCP server | IMPLEMENTED | 4 tools (+ aliases), lexical-first (ADR-0008), `nsgdb://session` |
| Host connectors (claw) | PARTIAL | `connectors/`: Hermes provider + MCP client + 4/4 contract tests; OpenClaw TS skeleton (wire into host checkout next) |
| Signed transport seam | IMPLEMENTED | `SignedEnvelope`, `signed_peer` example |
| UdpTransport | EXPERIMENTAL | unauthenticated demo |
| Overlay mesh routing | REMAINING | edge-directed pull today |
| Production crypto transport | REMAINING | seam only (ADR-0006) |
| Residual BQ / sharding | REMAINING | benchmark-driven, not scheduled |
| Relation inference | REMAINING | deliberate non-goal |
| Automatic lifecycle scheduler | PARTIAL | core explicit `tick()` only; host `host_scheduler.rs` (v1.1.11) automates expire/decay/consolidate/audit; v1.1.15 adds `collect_garbage` (TTL+GC) |
| State-driven retention GC | IMPLEMENTED | v1.1.15 `GcConfig/collect_garbage` (Decayed/Archived + TTL); compaction reclaims tombstones |

## Subsystem notes

### Memory model
NMD1 byte-identical to neural-os-core. All v0.6–v1.1.10 metadata in side-tables
(ADR-0003). Pre-v0.6 records decode with safe defaults (`scope=""`, empty
entities, `content_type=None`).

### Retrieval
Three paths (semantic / lexical / entities) + hybrid + temporal + weighted +
scoped. Hits typed for machine consumers; prose projection only Text/Json/Code.
BQ append-only with orphan reclaim on delete.

### Storage
CRC append-log, crash recovery, TickvFile TKCK fast-mount, compaction. Indexes
derived — rebuild on open.

### CRDT (feature `p2p`)
Full `MemoryRecord` replication, merge policy per layer, conflict preservation,
mesh harness tests. `node_versions` gossip may not converge in directed
topologies; **content** does.

### MCP
4 tools (`remember`/`recall`/`health`/`curate`); 23 legacy names as
`tools/call` aliases. Default recall **lexical** (ADR-0008). Unset
`NEURAL_SGDB_EMBEDDER` = none (`=demo` explicit only). `remember(text=)`
without vector → L3. Resources `nsgdb://doctrine` + `nsgdb://session`.
`health(view=tensions|staleness|era)`. Harness ADR-0010:
`curate(op=commit_run|deprecate_run)`. Hot test **100/0**. MCP contract
**1.1.20**.

### Host connectors
`connectors/` is host-side (not crate SemVer). Hermes `MemoryProvider` is
executable against current `mcp_server` (4 tools, lexical, scoped, lockfile).
OpenClaw adapter is a documented skeleton pending Node MCP transport.
Contract: `python -m unittest discover -s connectors/tests -v`. Keep adapter
notes in sync with `MCP_CONTRACT_VERSION` (not frozen at 1.1.9).

## Compatibility constraints

1. **NMD1 / TKLV / TKCK** — byte contracts; golden tests pin layout.
2. **no_std core** — verified on `x86_64-unknown-none`.
3. **Zero lib dependencies** — only `alloc`/`std`.
4. **Additive API** — v1.1.x features do not break v1.0 signatures.

## How this document is maintained

Update when a capability moves from REMAINING → IMPLEMENTED (code + tests
required). Historical Phase-0 audit content (v0.5 baseline) was superseded
by this snapshot on 2026-08-20.
