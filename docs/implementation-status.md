# neural-sgdb — Implementation Status

> **Current snapshot (2026-08-21, v1.1.13).** Capability matrix vs the shipped
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
| Default tests | `cargo test` | **243+ lib + 1 doc-test** |
| P2P tests | `cargo test --features p2p` | **289 lib + 1 doc-test** |
| no_std tests | `cargo test --no-default-features` | **195 lib + 1 doc-test** |
| no_std target | `cargo check --no-default-features --target x86_64-unknown-none` | **ok** |
| Hot test (MCP) | `cargo run --release --example mcp_client` | **95/0 exit 0** |
| Machine protocol | `cargo run --release --example two_ai_protocol` | **16/16 exit 0** |
| Agent protocol | `cargo run --release --example agent_protocol` | **23/23 exit 0** |
| Clippy / doc gates | `-D warnings` | green |

## Capability matrix

| Capability | State | Evidence |
|---|---|---|
| MemoryDoc NMD1 | IMPLEMENTED | `src/memory_doc.rs`, golden tests |
| MDM1 v6 side-table meta | IMPLEMENTED | scope, entities, content_type, version_id, … |
| L0–L7 layers | IMPLEMENTED | `MemoryLayer`, layer-aware merge policy |
| L6 associative memory | IMPLEMENTED | `sys/rel/`, `associate`/`related_to`/… |
| MemoryState lifecycle | IMPLEMENTED | `sys/state/`, active-only recall default |
| Temporal validity | IMPLEMENTED | `sys/validity/`, `recall_temporal`, `expire_old` |
| Dynamic VectorClock | IMPLEMENTED | 8-node + overflow; NMD1 72B unchanged |
| Provenance / identity | IMPLEMENTED | `memory_id`, `Hit.provenance`, MDM1 |
| ART index | IMPLEMENTED | prefix guard, rebuild, delete reclaim |
| BQ + FP32 recall | IMPLEMENTED | oversample, heap, MihIndex, era guard |
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
| Automatic lifecycle scheduler | PARTIAL | core explicit `tick()` only; host `host_scheduler.rs` (v1.1.11) automates expire/decay/consolidate/audit |
| State-driven retention GC | REMAINING | compaction reclaims tombstones only |

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
`health(view=tensions)`. Hot test 84/0.

### Host connectors
`connectors/` is host-side (not crate SemVer). Hermes `MemoryProvider` is
executable against `mcp_server` 1.1.9 (lexical, scoped, lockfile). OpenClaw
adapter is a documented skeleton pending Node MCP transport in the host tree.
Contract: `python -m unittest discover -s connectors/tests -v`.

## Compatibility constraints

1. **NMD1 / TKLV / TKCK** — byte contracts; golden tests pin layout.
2. **no_std core** — verified on `x86_64-unknown-none`.
3. **Zero lib dependencies** — only `alloc`/`std`.
4. **Additive API** — v1.1.x features do not break v1.0 signatures.

## How this document is maintained

Update when a capability moves from REMAINING → IMPLEMENTED (code + tests
required). Historical Phase-0 audit content (v0.5 baseline) was superseded
by this snapshot on 2026-08-20.
