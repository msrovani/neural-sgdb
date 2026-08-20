# neural-sgdb — Implementation Status

> Phase 0 audit of the Master Implementation Roadmap. This document reflects
> **code and tests in this repository at the time of writing**, not the
> architecture docs (which describe targets). Last updated: 2026-08-12.
> Baseline: `v0.5.0` (`main`, `84aa5e4`).
>
> **UPDATE (2026-08-19, v1.1.6)**: this is a historical Phase-0 audit — the
> capability matrix below predates v0.6–v1.1.6. For the CURRENT contract and
> feature surface use `docs/api.md` (crate v1.1.0, features up to v1.1.6),
> `CHANGELOG.md`, and `codemap.md`. The test baseline evolved to **229+1
> default / 275+1 p2p / 181+1 no_std** with the v1.1.6 typed-hit release
> (hot test 90/0; clippy/no_std/doc gates green).

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
| Default tests | `cargo test` | **136 lib + 1 doc-test ok** |
| P2P tests | `cargo test --features p2p` | **163 lib + 1 doc-test ok** |
| no_std tests | `cargo test --no-default-features` | **95 lib + 1 doc-test ok** |
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
| Full memory replication | PARTIAL→**anti-entropy (v0.7)** — `MemoryRecord` (doc+state+validity+meta) travels as one unit; clock announcement + directed pull of the missing causal range; relay through intermediates; durable `CrdtState` | `src/memory_doc.rs`, `src/engine.rs`, `src/crdt.rs` | overlay routing/trust (v0.8+) |
| Conflict preservation | IMPLEMENTED **(v0.9)** — `ConflictRecord` (id determinístico, candidates+records MDR1 paralelos, status Open/Resolved, evidência preservada) em `sys/conflict/`; `merge_remote` cria conflito na branch CONCORRENTE; `resolve_conflict` importa vencedor via evidência + marca loser parent; `dismiss_conflict` limpa; `conflicts()` enumera. Nenhuma decisão semântica no core (roadmap §14/15). | `src/conflict.rs`, `src/engine.rs`, `src/sgdb.rs` | arbitração plugável (M4) |
| Dynamic VectorClock | **IMPLEMENTED (v0.6)** — 8-node fast path + overflow registry (bounded), dynamic `set_counter`, overflow-aware compare/merge; NMD1 stays 72B | `VectorClock` in `src/memory_doc.rs` + tests | causal DAG on top |
| Causal DAG | PARTIAL→**implemented core (v0.7)** — per-version identity (`MemoryMeta.version_id`, MDM1 v2, v1-decodable), `sys/version/` reverse index, `Sgdb::version_of`/`lineage`, `supersede` links versions; merge-branch exploration via `parent_ids` | `src/memory_doc.rs`, `src/engine.rs`, `src/sgdb.rs` | full DAG queries (children/descendants) |
| Provenance | PARTIAL→**implemented core (v0.6)** — `MemoryMeta` (source, confidence, importance, created_tick, parents) in `sys/meta/`; exposed in `Hit.provenance`; pre-v0.6 records lazily migrated. **Provenance-aware recall (v0.8)**: default recall = ACTIVE only; `recall_historical`/`recall_lexical_historical` include inactive with state exposed | `src/memory_doc.rs`, `src/engine.rs`, `src/sgdb.rs` | explain/arbitration on top |
| L6 associations | DESIGN→**IMPLEMENTED (v0.8)** — `associate`/`related_to`/`causes`/`supports`/`contradicts`/`derived_from`; side-table `sys/rel/` + ART forward/reverse index (derived, rebuilt on open, pruned on delete); no inference | `src/engine.rs`, `src/sgdb.rs`, `RelationKind` in `memory_doc.rs` | relation-aware retrieval fusion |
| Lifecycle engine | PARTIAL→**IMPLEMENTED (v0.8)** — `MemoryLifecycle::tick(db, now)` deterministic (no hidden wall clock), `LifecycleConfig`/`LifecycleReport`; L1→L2 commit, L2→L3 promotion, L3→L4 heuristic semanticization (no LLM/embedding in core), decay→Decayed (never delete), archive of aged superseded; lineage wired on every promotion | `src/lifecycle.rs` | consolidation/reinforcement (v0.9) |
| Semantic consolidation | PARTIAL→**heuristic foundation (v0.8)** — L3→L4 promotion by importance+age records `derived_from`; embedding backfill is the upper layer's job | `src/lifecycle.rs` | repetition/similarity-density signals |
| Cognitive API | **IMPLEMENTED core (v0.9)** — `remember`/`recall`/`rag_context`/`supersede`/`delete` + `memory_id`/`meta`/`set_importance`/`set_confidence` + `reinforce(key,delta)` + `forget` (Archived) + `explain` → `MemoryExplanation` + `transfer_to` (layer move + lineage) + `merge_memories(a,b,target)` + `associate`/`related_to`/`contradicts` + `conflicts`/`resolve_conflict`/`dismiss_conflict`. MCP exposes all 14 tools (v0.9). | `src/sgdb.rs`, `examples/mcp_server.rs` | arbitration/trust (M4) |
| AI arbitration | **POLICY-PLUGGABLE (v1.0)** — `ArbitrationPolicy` trait + deterministic `Arbitrator` (prefer/invalidate/merge/escalate by confidence/importance/recency). NO LLM inside the core — the policy is a consumer; evidence preserved, never deleted | `src/arbitration.rs` | plug real AI/BitNet policies outside the crate |

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

IMPLEMENTED (v0.7): a real **anti-entropy cycle** — each round announces
the full known clock (`CrdtMemorySync::announce`/`known_clock`: own +
relayed versions), every node pulls only the **missing causal range**
(`known+1..=v` per node) resolved through the derived `(node, counter) →
keys` index (`Sgdb::keys_for_clock`); versions and records cross
intermediate nodes (relay/gossip tested A→C→B with no direct A↔B edge);
duplicate/stale/out-of-order delivery idempotent; **durable replication
state** `CrdtState` (node_id + counters + known versions, wire "CRDT"
bounds-checked) via `state()`/`restore()` + `Sgdb::read_side_bytes`/
`write_side_bytes` — a restarted node does not regress its clock.

One logical write = one causal version: `remember_semantic` writes its L2
text companion under the same counter as L4 (`put_companion`), so the CRDT
version and the per-doc clock never diverge (previously each put ticked and
the directed pull lost docs).

Remaining gap (roadmap §3): pull is still edge-directed (each round
reconciles every announced version along the edge); there is no overlay
routing/partial-mesh spanning — that is acceptable for v0.7 and is the
v0.8 relay/anti-entropy refinement along with peer trust (§23).

### 3.5 Indexes (`src/art.rs`, `src/hamming_dispatch.rs`) — IMPLEMENTED

ART exact/prefix with compression + SIMD; runtime SIMD dispatch (scalar/
AVX2/AVX-512) as an injectable seam; indexes are **derived state** —
`Sgdb::rebuild_indices` reconstructs ART/BQ/lexical from storage (tested
write→close→reopen→rebuild→recall).

### 3.6 MCP (`examples/mcp_server.rs`) — PARTIAL (integration layer)

`memory://{layer}/{key}` resources; embedding generation is a trigram demo,  clearly labeled. v0.9 exposes 14 MCP tools (remember/recall/rag_context/
  explain/reinforce/forget/associate/related_to/contradicts/supersede/
  conflicts/resolve_conflict/merge_memories) + provenance (state/imp/conf/src)
  per hit in recall. ServerInfo v0.9.0.

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
  identifies the (layer+key) slot and is stable across overwrites.  DAG queries (children/descendants) implemented, `transfer_to` (layer move with lineage),
  `merge_memories` (fusão com parent_ids=[A,B]), `forget` (archival),
  `reinforce` (importance delta + last_reinforced), and MCP 14 tools (v0.9).
- Anti-entropy (v0.7) relays versions and records through intermediate
  nodes and persists replication state (`CrdtState`); the remaining
  limitation is that reconciliation is edge-directed (no overlay routing),
  and replication state durability is opt-in (`sys/crdt/…` side-table) —
  the p2p demo stays ephemeral.
- Layer policy is explicit and enforced at the version and record level
  (v0.6), but there is no **resolution** API yet — conflicts are detected
  and preserved, a higher layer decides — `resolve_conflict` (v0.9) imports winner
  via evidence + marks loser parent; `dismiss_conflict` cleans up (Phase 14/15, v0.9).
- Same-key concurrent writes are never silently overwritten (`Conflict`),
  but both values are not co-located in one store: each stays on its author
  node until a higher layer resolves.
- Recall (v0.8) filters to **active** by default (`recall_historical` opts
  into inactive with `provenance.state`); `recall_weighted` still uses the
  layer as a coarse importance proxy — importance-aware weighting is v0.9
  along with `reinforce` (v0.9).
- `reinforce` (v0.9) updates importance + `last_reinforced` (MDM1 v3); lifecycle
  decay (v0.8) uses importance, so reinforced memories decay slower.
- L4→L5 proceduralization is deliberately manual (HITL). L3→L4 promotion
  creates the L4 doc without a bitvec — semantic (BQ) retrieval of
  consolidated memories requires the upper layer to backfill embeddings.
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
| Sgdb API / lifecycle | `sgdb.rs` (delete, supersede, validity, rebuild, reopen, relations, active/historical recall) + `lifecycle.rs` (commit/promote/semanticize/decay/archive, determinism, idempotence) |
| L6 relations | `sgdb.rs` (directions, determinism, reopen persistence, delete topology cleanup, reserved-`#` rejection) |
| CRDT | `crdt.rs` (self/stale/newer/concurrent/duplicate, envelope malformed-input) |
| CRDT anti-entropy / durability | `crdt.rs` (3-node triangle, partition/rejoin, relay through intermediate node, version-range pull, `CrdtState` roundtrip + restart-no-regression, idempotence) |
| Recall modes | `sgdb.rs` (active default vs historical, semantic + lexical) |
| p2p / telepathy | `examples/p2p_telepathy.rs` (two-node convergence) |
| Stress / bench | `examples/stress.rs`, `examples/bench.rs` |

Roadmap §29–31 status: three-node + partition/rejoin + relay + restart
tests are in place (v0.7); property tests (merge commutativity/
associativity/idempotence) cover the VectorClock merge; remaining: a full
property suite over the mesh convergence (multiple random topologies) and
A↔B↔C concurrent-write scenarios at scale.

## 7.5 v0.6 + v0.7 + v0.8 (M1/M2) delivered (this session)

**v0.8 — M2 (Phase 8/9/15/16):** L6 associative memory (`associate`/
`related_to`/`causes`/`supports`/`contradicts`/`derived_from`, side-table
`sys/rel/` + ART forward/reverse, topology pruned on delete); provenance-
aware recall (default = active only; `recall_historical`/
`recall_lexical_historical`); deterministic `MemoryLifecycle::tick(db,
now)` with `LifecycleConfig`/`LifecycleReport` (L1→L2 commit, L2→L3
promotion, L3→L4 heuristic semanticization without LLM/embedding in core,
configurable decay → Decayed, aged-superseded archiving; every promotion
wires parent_ids + `derived_from`); `Sgdb::add_parents`.


**v0.7 — M1 (Phase 3 + 6):** per-version identity (`version_id`), MDM1 v2
(v1-decodable), `sys/version/` reverse index (key + the version's own meta),
`Sgdb::version_of`/`lineage`, `supersede` links current versions,
`HitProvenance.version_id`; **anti-entropy** — clock announce/gossip,
directed pull of the missing causal range (`keys_for_clock`), relay through
intermediate nodes, durable `CrdtState` (`state`/`restore` + `sys/crdt/…`
side-table), one-logical-write = one causal version (`put_companion`).

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
Contradictions #1–#3 are resolved (see §6); anti-entropy + durable
replication state (P0-7/P0-11) are done in v0.7; lifecycle tick (P0-8),
active/historical recall (P0-9b) and L6 relations (P0-10) are done in
v0.8.

## 8. First concrete tasks (v0.9 — M3, cognitive API)

> P0-1..P0-11 are **done (v0.6/v0.7/v0.8)** — see §7.5. Remaining backlog:

1. **Reinforcement/decay API** — `reinforce(key, delta)` + `last_reinforced`
   (Phase 17); decay already ticks in the lifecycle but needs the explicit
   counter-signal.
2. **First-class conflict model** — `Conflict` object (conflict_id, subject,
   candidates, concurrency, sources, confidence) persisted, not just the
   in-memory `conflicts` vec (Phase 14).
3. **Resolution API** — `resolve_conflict`/`supersede`/`invalidate`/
   `merge_memories` at the cognitive surface; CRDT detects/preserves, the
   layer decides (Phase 15).
4. **Cognitive verbs** — `forget`/`explain`/`transfer`/`merge` + MCP
   surface exposing provenance/state (Phase 18/19).
5. **Arbitration (v1.0)** — policy-pluggable arbitration WITHOUT LLM in the
   core, trust seam on the authenticated transport, observability counters.

Suggested versioning (respecting §37, per maintainer decision): P0-1..P0-7
in the **v0.6.x** line; anti-entropy + per-version identity in **v0.7**;
P0-8..P0-10 in **v0.8** (lifecycle + L6); cognitive API in **v0.9**;
arbitration/trust/observability in **v1.0**. Nothing ships without tests
and doc updates.

## 9. How this document is maintained

Update this file at the end of every phase: re-run §1, move rows from
DESIGN/PARTIAL to IMPLEMENTED **only when code + tests prove it**, and
record the phase status (implemented / tests / performance / compatibility /
known limitations / next phase) per roadmap §42.
