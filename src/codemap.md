# src/ — neural-sgdb core

## Responsibility
Cognitive memory database (SGDB) layer, dual-mode `no_std` + `std`, zero
external dependencies (only `alloc`). Domain model: **memories, not data** —
documents with L0–L7 layer, vector clock, semantic recall via BQ + FP32
rescore, O(k) ART index, pluggable storage.

## Design
Layered architecture with injectable seams (replacing the origin kernel):

| Module | Responsibility | Pattern |
|---|---|---|
| `lib.rs` | Crate root, `cfg_attr(not(feature="std"), no_std)`, public re-exports | Facade |
| `memory_doc.rs` | `MemoryDoc` (NMD1 format), zero-copy `MemoryDocView`, `VectorClock`, `MemoryLayer` L0–L7, `MemoryMeta`/`MemoryRecord` (MDM1 **v6**: version_id, last_reinforced, scope, entities, content_type) | Binary contract byte-identical to the OS |
| `art.rs` | `ArtIndex` — Adaptive Radix Tree Node4/16/48/256, prefix scan, tombstone delete | Radix Tree (Leis 2013) |
| `bq.rs` | `BqFlatIndex` — binary quantization (1-bit) + top-k by Hamming, `MihIndex` | Quantized flat scan |
| `hamming_dispatch.rs` | SIMD dispatch scalar/AVX2/AVX-512 (`#[target_feature]`), seam `cpu_caps()`/`set_cpu_caps()` | Runtime strategy |
| `ctype.rs` | **v1.1.6**: `ContentType` (Text/Json/Code/Embedding(dim)/Binary), `RecallPath`, `detect_content_type`, `stable_label`/`parse_stable_label`/`renders_prose` — tipagem do datum p/ consumidor máquina | HINT derivation (no_std-safe) |
| `engine.rs` | `AiosDatabaseEngine` — RAM L0/L1 + Storage L2–L7, ART/BQ indexing, rebuild, side-tables, relations, `entity_index` | Persistence engine |
| `sgdb.rs` | `Sgdb` — public facade; `remember_text_with` (v1.1.9 lexical write); `remember_semantic_with`/`RememberOutcome`/`recall_empty_hint` (v1.1.7); `Hit` tipado v1.1.6; **v1.1.10**: `decay_importance`/`consolidate_recurrences`/`recall_weighted_full` (`Hit.score_breakdown`)/`audit_checkpoint`/`audit_verify`/`rollback_to` + `validate_written` | Facade |
| `audit.rs` | **v1.1.10**: ledger hash-chain `sys/audit/<seq:016x>` (wire `AUD1`), `AuditEntry`/`AuditSnapshotItem`, `audit_key`/`audit_seq_from_key` — base do rollback cognitivo | Append-only ledger |
| `doctrine.rs` | **v1.1.8**: agent doctrine (`DOCTRINE`, key/scope/entities) — source `docs/doctrine.md` | Compile-time include |
| `era.rs` | **v1.1.5**: `Sgdb::era_report()`/`era_report_lines()` — era do corpus, veredito, custo estimado de migração | Read-only diagnostic |
| `lexical.rs` | Inverted BM25 index over L2/L3 texts; `search` → `(key, score, matched_terms)` (v1.1.6) | Inverted index |
| `lifecycle.rs` | `MemoryLifecycle` — deterministic tick (commit/promote/semanticize/decay/archive) | Deterministic engine |
| `arbitration.rs` | `ArbitrationPolicy` trait + deterministic `Arbitrator` (no LLM in core) | Policy |
| `conflict.rs` | `ConflictRecord` (CFL1), conflict detection/preservation/resolution | CRDT adjunct |
| `limits.rs` | **P1-3**: centralized storage/embedding ceilings | Constants |
| `metrics.rs` | Runtime metrics counters | Observability |
| `storage.rs` | `Storage` trait (4 methods) + `InMemory` + `FileStorage` (CRC32 append-log crash-safe) + `SgdbError` | Pluggable trait / Strategy |
| `tickv.rs` | Byte-exact TKLV/TKCK codec of the OS TickvLite + `TickvFile` backend | Format interop |
| `trust.rs` | `SignedEnvelope`/`Signer`/`TrustStore` reference signed-transport flow (P2-3, p2p) | Auth seam (no crypto in core) |
| `crdt.rs` | `CrdtMemorySync` (LWW) + `Transport` trait + `UdpTransport` (`p2p` feature) | CRDT / Observer |
| `wire_fuzz.rs` | **P2-4**: single LCG fuzz harness over all 8 wire types | Fuzz gate |

## Flow
1. `Sgdb::open(backend: impl Storage)` → creates `AiosDatabaseEngine` +
   `rebuild_indices_from_storage` (scan `md/`, re-index ART/BQ/lexical/entities)
2. `remember_*` → `MemoryDoc::encode` (NMD1) → RAM L0/L1 or `Storage::put`
   (L2–L7) → `index_doc` (ART for keys, BQ for L4/L5 with bitvec, lexical for
   L2/L3 texts) → meta side-table `sys/meta/` (MDM1 v6)
3. `recall(query: &[f32], k)` → `BqFlatIndex::top_k_f32` (coarse BQ) → FP32
   rescore (1−cos distance) → fine top-k → `Hit { key, text, dist, path,
   content_type, payload_type, score, matched_terms, validity, rel }` (tipado
   v1.1.6 — `resolve_content_type`: declared wins sobre o detector)
4. `checkpoint()` → flush RAM L0/L1 → Storage; `prune_working_ram()` → drop RAM
5. `TickvFile` writes 512-aligned TKLV records; `scan_volume` = OS `recover()`
   semantics (hunt 512-aligned, EOF all-0x00/0xFF, last-wins)

## Integration
- Consumed by: `examples/` (bench, mcp_server), future host apps
- Depends on: only `alloc` (no_std) / `std` (FileStorage, UdpTransport, examples)
- Interop: `MemoryDoc` (NMD1) and `tickv` (TKLV/TKCK) byte-identical to neural-os-core
- Seams: clock `now: u64` (no internal clock), `cpu_caps()`, `sgdb_log!` (no-op
  no_std / eprintln std)

## Gotchas (port lessons)
- `f32::sqrt` does NOT exist in core for `x86_64-unknown-none` → `sqrt_f32`
  Newton in `sgdb.rs`
- ART does not support prefix keys (one key being a prefix of another) — use
  fixed-width keys
- CRDT rate-limit uses `Option<u64>` (the 0 sentinel fails on first sync at now=0)
- `deny(warnings)` in no_std elevates dead-code to error → explicit
  `#[allow(dead_code)]` on port-parity
