# neural-sgdb — Implementation Status

> Phase 0 audit of the Master Implementation Roadmap. This document reflects
> **code and tests in this repository at the time of writing**, not the
> architecture docs (which describe targets). Last updated: 2026-08-12.
> Baseline: `v0.5.0` (`main`, `84aa5e4`).

## 0. Status labels

| Label | Meaning |
|---|---|
| **IMPLEMENTED** | Shipped, exercised by tests, in the public API. |
| **PARTIAL** | A real, tested core exists but a stated requirement is missing. |
| **EXPERIMENTAL** | Works but is explicitly demo-grade / not production-safe. |
| **DESIGN** | Specified in `docs/architecture/*` only; no code. |
| **FUTURE** | Listed on the roadmap; no design or code yet. |

## 1. Baseline (recorded before any Phase 1 work)

| Check | Command | Result |
|---|---|---|
| Default tests | `cargo test` | **113 lib + 1 doc-test ok** |
| P2P tests | `cargo test --features p2p` | **125 lib + 1 doc-test ok** |
| no_std tests | `cargo test --no-default-features` | **74 lib + 1 doc-test ok** |
| no_std target | `cargo check --no-default-features --target x86_64-unknown-none` | **ok** |
| Examples | `cargo test --examples` | **ok** |
| Bench | `cargo run --release --example bench` | BQ top-k heap(k=5) = 98.9 µs vs full-sort = 279.5 µs (**2.8×**) |
| Stress | `STRESS_N=1500 STRESS_REOPEN=300 cargo run --release --example stress` | **ok** (delete integrity verified) |

Repo hygiene note: the maturation-sprint v0.2 changes (Sgdb::delete, SignedEnvelope,
index-rebuild, docs) are **staged but uncommitted** — the git identity
(`user.name`/`user.email`) is not configured in this checkout. See §8.

## 2. Capability matrix vs roadmap

| Capability | Current state | Evidence | Target |
|---|---|---|---|
| MemoryDoc | PARTIAL→**improved (v0.6)** — layer/key/clock/payload/bitvec + in-memory `meta` (id, source, confidence, importance, parents, clock overflow); NMD1 unchanged | `src/memory_doc.rs` | richer memory model (validity in-record, associations) |
| L0–L7 | IMPLEMENTED as storage classes + validation | `MemoryLayer` in `memory_doc.rs`, `from_u8` single validation point | real lifecycle semantics per layer |
| MemoryState | PARTIAL→**improved (v0.6)** — Active/Superseded/Archived/Invalidated/**Decayed** via side-table; supersede wires `parent_ids` | `MemoryState` + `sys/state/` in `engine.rs`; `Sgdb::supersede` | lifecycle-driven transitions |
| Temporal validity | IMPLEMENTED (side-table) — `set_validity`/`invalidate`/`recall_at` | `sys/validity/` in `engine.rs`, `Sgdb` | validity as first-class metadata |
| ART index | IMPLEMENTED | `src/art.rs` (Node4→16→48→256, prefix scan) | preserve/stabilize |
| BQ retrieval | IMPLEMENTED — bounded top-k heap, MIH, FP32 rerank, oversample | `src/bq.rs`, `src/sgdb.rs`; bench above | incremental scalability only (benchmark-driven) |
| Persistence | IMPLEMENTED — Storage trait, FileStorage (CRC append-log), TickvFile, recovery, compaction | `src/storage.rs`, `src/tickv.rs` | durable cognitive lifecycle |
| CRDT version sync | IMPLEMENTED (v0.6) — `missing_after(peer)` + delta/snapshot abstractions; version 0 ignored; `local_version` = own writes only | `src/crdt.rs` | causal distributed memory |
| Full memory replication | PARTIAL→**improved (v0.6)** — `MemoryRecord` (doc+state+validity+meta) travels as one unit; `MemoryDelta`/`MemorySnapshot` carry records with bounds-checked codecs; example pulls via export+merge_remote | `src/memory_doc.rs`, `src/engine.rs`, `examples/p2p_telepathy.rs` | anti-entropy protocol (v0.8) |
| Conflict preservation | PARTIAL→**improved (v0.6)** — per-layer `MergePolicy` table (L2/L3 multi-value, L4 causal-LWW-with-history, L5/L7 controlled, L0/L1 local-only → `Rejected`); `Sgdb::merge_remote` never LWW-overwrites concurrent same-key memories | `MergePolicy`, `MergeVerdict` in crdt.rs; `merge_remote` in sgdb.rs | per-layer conflict model + resolution API |
| Dynamic VectorClock | **IMPLEMENTED (v0.6)** — 8-node fast path + overflow registry (bounded), dynamic `set_counter`, overflow-aware compare/merge; NMD1 stays 72B | `VectorClock` in `src/memory_doc.rs` + tests | causal DAG on top |
| Causal DAG | PARTIAL→**implemented core (v0.7)** — per-version identity (`MemoryMeta.version_id`, MDM1 v2, v1-decodable), `sys/version/` reverse index, `Sgdb::version_of`/`lineage`, `supersede` links versions; merge-branch exploration via `parent_ids` | `src/memory_doc.rs`, `src/engine.rs`, `src/sgdb.rs` | full DAG queries (children/descendants) |
| Provenance | PARTIAL→**implemented core (v0.6)** — `MemoryMeta` (source, confidence, importance, created_tick, parents) in `sys/meta/`; exposed in `Hit.provenance`; pre-v0.6 records lazily migrated | `src/memory_doc.rs`, `src/engine.rs`, `src/sgdb.rs` | provenance-aware recall modes (active vs historical) |
| L6 associations | DESIGN — no `associate`/`related_to` API | `docs/architecture/01` §5, `06-cognitive-api.md` | relation index on ART |
| Lifecycle engine | PARTIAL — primitives only (state, validity, supersede); no tick/decay/consolidation | engine/sgdb + `docs/architecture/02` | `MemoryLifecycle::tick(db, now)` |
| Semantic consolidation | DESIGN | `docs/architecture/02` | deterministic heuristic (no LLM) |
| Cognitive API | PARTIAL — `remember`/`recall`/`rag_context`/`supersede`/`delete` + `memory_id`/`meta`/`set_importance`/`set_confidence` (v0.6); no associate/reinforce/explain | `src/sgdb.rs` | progressive verb surface |
| AI arbitration | NOT core — correctly absent | `docs/architecture/06` | separate upper layer |

## 3. Per-subsystem detail

### 3.1 Memory model (`src/memory_doc.rs`) — IMPROVED (v0.6)

IMPLEMENTED: NMD1 binary format (layer + key + VectorClock + payload + bitvec),
byte-identical to `neural-os-core` (golden test); `MemoryLayer` L0–L7 with
centralized `from_u8` validation; `MemoryState` enum incl. `Decayed`
(side-table, NMD1 untouched); `VectorClock` **dynamic** — 8-node fast path +
bounded overflow registry (`set_counter` = dynamic node registration),
overflow-aware `happens_before`/`concurrent`/`merge`/equality, saturating
counters; `MemoryMeta` (memory_id, source, confidence, importance,
created_tick, parent_ids, clock_overflow) in `sys/meta/`; deterministic
`generate_memory_id`; bounds-checked decoding everywhere (no panics on
malformed input, fuzz-tested).

NOT implemented: `valid_from`/`valid_to` in the record, associations,
per-version identity (the causal DAG — Phase 3). Format decision (v0.6):
NMD1 stays v1 byte-identical; metadata lives in `sys/meta/` side-tables (no
version bump needed). Pre-v0.6 records return `meta: None` until re-put —
never silent reinterpretation of old bytes (§5.6, §35).

### 3.2 Retrieval (`src/sgdb.rs`, `src/bq.rs`, `src/lexical.rs`) — IMPLEMENTED

Stages are explicit: BQ Hamming candidate generation → FP32 cosine rerank
(oversampled) → optional validity filter (`recall_at`) → deterministic
tie-break (storage-key dedupe + stable secondary order). `recall_weighted`
adds recency/importance signals as explainable weighted components.
`recall_lexical`/`recall_hybrid` (BM25 dual-path). Bounded top-k heap
(`O(N·D/64 + N log k)`) with regression tests and benchmark.

Gap: `recall` still does **not** filter by `MemoryState` (superseded
memories appear) — but hits now expose `provenance` (v0.6) so the caller can
distinguish active vs superseded/decayed. Phase 9 adds explicit recall modes
(`recall_active` vs historical).

### 3.3 Storage (`src/storage.rs`, `src/tickv.rs`) — IMPLEMENTED

`Storage` trait (4 methods) + `InMemory` + `FileStorage` (CRC32 append-log,
tombstones, deterministic recovery incl. truncated/corrupt tail, atomic
compaction, lazy handle) + `Durability {Buffered, Flushed, Durable}` +
`sync_durable` + TickvFile/TKLV/TKCK byte-exact with `neural-os-core`.
Recovery fault-injection tests exist.

### 3.4 CRDT (`src/crdt.rs`, feature `p2p`) — PARTIAL / EXPERIMENTAL

IMPLEMENTED (v0.6): `CrdtMemorySync` version sync with merge verdicts
(SelfPacket/Stale/Duplicate/Applied/Conflict/**Rejected**); concurrent
versions preserved in `conflicts` (never blind LWW); **layer-aware
`MergePolicy`** (L0/L1 local-only → `Rejected`, L2/L3 multi-value, L4
causal-LWW-with-history, L5/L7 controlled, L6 reserved); **`missing_after`**
(causal range a peer lacks); **`MemoryDelta`/`MemorySnapshot` carry real
`MemoryRecord`s** (doc + state + validity + meta) with bounds-checked
codecs; version-0 packets ignored (no phantom conflicts from relay nodes);
`local_version` counts only own writes (a fresh node never re-broadcasts a
peer's version as its own); delta-queue (`pending`, `send_delta`);
`SignedEnvelope` (payload + node_id + opaque auth) as the authentication
seam; `Transport` trait; `UdpTransport` **explicitly unauthenticated demo**.

Honest gap (roadmap §3): this is still **version sync + document transfer**,
not a full anti-entropy protocol — versions and records propagate only along
direct edges in the mesh harness (no relay through intermediates), and
replication state is not yet durable (identity/clock reset on restart). The
protocol abstractions (`MemoryDelta`, `missing_after`) are in place; the
anti-entropy cycle is the v0.8 milestone.

### 3.5 Indexes (`src/art.rs`, `src/hamming_dispatch.rs`) — IMPLEMENTED

ART exact/prefix with compression + SIMD; runtime SIMD dispatch (scalar/
AVX2/AVX-512) as an injectable seam; indexes are **derived state** —
`Sgdb::rebuild_indices` reconstructs ART/BQ/lexical from storage (tested
write→close→reopen→rebuild→recall).

### 3.6 MCP (`examples/mcp_server.rs`) — PARTIAL (integration layer)

`memory://{layer}/{key}` resources; embedding generation is a trigram demo,
clearly labeled. Does not yet expose cognitive verbs (associate, reinforce,
explain, supersede) or provenance/state fields in responses.

## 4. Compatibility constraints

1. **NMD1 byte-format** is the interop contract with `neural-os-core` — the
   maturation sprint deliberately kept it byte-identical and used side-tables
   (`sys/state/`, `sys/validity/`) for new metadata. Any in-record field
   addition requires an explicit version bump + golden fixtures + migration
   path (§35).
2. **TKLV/TKCK** storage format stays byte-exact (fast-mount, FNV-1a).
3. **no_std core**: memory model, clock, CRDT, storage traits, indexes must
   remain `no_std` (currently verified via `x86_64-unknown-none`). Host
   concerns (fs, UDP, MCP) live behind feature gates.
4. **Public API**: `Sgdb::open`, `remember*`, `recall*`, `rag_context`,
   `supersede`, `delete`, `set_validity` are the stable surface. New
   capabilities must be **additive**.
5. **Optional features** must stay isolated: `std`, `file-storage`,
   `simd-runtime`, `p2p`.

## 5. Known limitations (honest)

- VectorClock is **dynamic** (v0.6) with a bounded overflow registry (248
  extra nodes); the 256-node u8 space is the hard limit. Per-version
  identity (causal DAG) is **implemented (v0.7)**: `version_id` + `lineage`
  walk (parents resolve via the `sys/version/` index); `memory_id` still
  identifies the (layer+key) slot and is stable across overwrites. DAG
  queries (children/descendants) and per-layer conflict-resolution on top of
  it remain future work.
- CRDT is version sync + record transfer, not full anti-entropy: versions
  and records travel only along direct edges (no relay through
  intermediates); no durable replication state (identity/clock reset on
  restart is acceptable today only because the demo is ephemeral).
- Layer policy is explicit and enforced at the version and record level
  (v0.6), but there is no **resolution** API yet — conflicts are detected
  and preserved, a higher layer decides (roadmap Phase 14/15, v0.9).
- Same-key concurrent writes are never silently overwritten (`Conflict`),
  but both values are not co-located in one store: each stays on its author
  node until a higher layer resolves.
- No provenance/confidence/importance filtering in `recall` (exposed via
  `Hit.provenance`); `recall_weighted` uses the layer as a coarse proxy.
- L6 associations absent; lifecycle engine absent (only state/validity/
  supersede primitives); no decay/reinforcement/consolidation.
- `UdpTransport` unauthenticated (documented, demo-only).

## 6. Architecture contradictions found in the code

1. **Fixed-node VectorClock vs CRDT node_id** — **RESOLVED (v0.6)**: the
   clock is dynamic (8-node fast path + bounded overflow registry).
2. **State lives outside the record** — **RESOLVED (v0.6)**: `MemoryRecord`
   transports doc + state + validity + meta as one unit (`export_record` /
   `import_record` / `merge_remote`); the example no longer drops lifecycle
   metadata across nodes.
3. **`conflicts` exposure vs layer policy** — **RESOLVED (v0.6)** at the
   version level: `MergePolicy` table + `MergeVerdict::Rejected` + doc-level
   `Sgdb::merge_remote`. Open (v0.9): resolution API and a persisted conflict
   object (the roadmap's first-class conflict model).
4. **Recency is wall-clock in the key** (`/ts/<hex>`): used by
   `recall_weighted` as "recency". This is a *timestamp*, not causal time —
   fine as a read-time signal, but it must never be presented as causal
   ordering (roadmap §43).

## 7. Tests covering each subsystem

| Subsystem | Where |
|---|---|
| MemoryDoc / VectorClock | `memory_doc.rs` (equality, happens-before, concurrent, merge, saturation, adversarial decode fuzz) |
| ART | `art.rs` (insert/delete/prefix/scan, memory reclamation) |
| BQ / top-k / rerank | `bq.rs`, `sgdb.rs` (stages test, determinism) |
| Storage / recovery | `storage.rs` (fault injection, parity InMemory↔FileStorage↔TickvFile) |
| Tickv | `tickv.rs` (byte-exact codec, GC, crash) |
| Sgdb API / lifecycle | `sgdb.rs` (delete, supersede, validity, rebuild, reopen) |
| CRDT | `crdt.rs` (self/stale/newer/concurrent/duplicate, envelope malformed-input) |
| p2p / telepathy | `examples/p2p_telepathy.rs` (two-node convergence) |
| Stress / bench | `examples/stress.rs`, `examples/bench.rs` |

Missing per roadmap §29–31: three-node and partition/rejoin tests; property
tests (merge commutativity/associativity/idempotence) exist only partially.

## 7.5 v0.6 + v0.7 (M1) delivered (this session)

**v0.7 — M1 (Phase 3):** per-version identity (`version_id`), MDM1 v2
(v1-decodable), `sys/version/` reverse index (key + the version's own meta),
`Sgdb::version_of`/`lineage`, `supersede` links current versions,
`HitProvenance.version_id`. Next in v0.7: anti-entropy (M1b), durable
replication metadata.

**v0.6 —** the whole block ships in the **v0.6.x** line (no version jump):

**Block 1 (P0-1..P0-4, P0-9 partial):** dynamic VectorClock, memory_id,
provenance side-table + Hit exposure, format decision (NMD1 stays v1
byte-identical; metadata in `sys/meta/`), `Hit.provenance`.

**Block 2 (P0-5..P0-7):** replication side-metadata closed — `MemoryRecord`
carries state + validity + meta; `export_record`/`import_record`/`merge_remote`;
`MemoryDelta`/`MemorySnapshot` with real records + codecs; `missing_after`;
layer-aware `MergePolicy` table + `MergeVerdict::Rejected`; 3-node triangle
convergence, partition/rejoin preserving concurrent writes, duplicate/stale/
out-of-order idempotence, fresh-node catch-up, merge-associativity property.
Contradictions #1–#3 are resolved (see §6).
Pending: P0-8 (lifecycle tick), P0-10 (L6 relations), plus anti-entropy
protocol (relay of versions/docs through intermediate nodes) as v0.8.

## 8. First concrete tasks (Phase 1 onward — v0.6.x/v0.8)

> P0-1..P0-7 are **done (v0.6)** — see §7.5. Remaining backlog:

1. **P0-8 (lifecycle): deterministic `MemoryLifecycle::tick(db, now)`**
   driving the existing state/validity primitives (L1→L2 commit,
   L2→L3 promotion signals) with no hidden wall clock.
2. **P0-9b (retrieval): provenance-aware recall mode** — `recall_active`
   default filtering superseded/invalidated/decayed, historical mode opting
   in; expose provenance fields in `rag_context`.
3. **P0-10 (L6 foundation): relation side-index on ART** —
   `associate(a, rel, b)` + `related_to`/`contradicts`/`causes`/`supports`,
   memory-native, no inference.
4. **P0-11 (durable replication metadata):** persist CRDT clock/identity
   so a restarted node does not regress its clock (Phase 22).
5. **Anti-entropy (v0.8):** relay of versions and records through
   intermediate nodes (today versions/docs propagate only along direct
   edges; a full anti-entropy cycle with clock exchange is the next
   milestone — roadmap Phase 6).

Suggested versioning (respecting §37, per maintainer decision): P0-1..P0-7
land together in the **v0.6.x** line (identity + provenance + dynamic clock
foundation + delta replication + layer-aware CRDT); P0-8..P0-10 land in
**v0.8** (lifecycle + L6). Nothing below ships without tests and doc updates.

## 9. How this document is maintained

Update this file at the end of every phase: re-run §1, move rows from
DESIGN/PARTIAL to IMPLEMENTED **only when code + tests prove it**, and
record the phase status (implemented / tests / performance / compatibility /
known limitations / next phase) per roadmap §42.
