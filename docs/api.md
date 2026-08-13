# neural-sgdb — API Contract

> Contract document for the extraction of the SGDB core from neural-os-core.
> Status: **current public contract (crate v1.0.0)** — this document is the
> current public contract; roadmap items are explicitly marked as such. The
> internal API lives in `crates/k_ai/src/sgdb/` of the parent OS; this doc
> defines the public surface the community crate exposes (and already ships).

## Principles

1. **Memories, not data.** The API speaks `remember` / `recall`, L0–L7 layers
   and memory transfer (CRDT) — not generic `put` / `get`.
2. **Instance, not global.** The OS uses a global static (`ENGINE`); the
   community crate exposes `Sgdb::open(backend)` — the developer can open as
   many databases as they want.
3. **Storage by trait.** No kernel dependency: implement 4 methods and you're
   integrated. We ship `InMemory` and `FileStorage` ready to use.
4. **Everything injectable.** Clock, SIMD detection and logging are seams, not
   dependencies.
5. **`no_std` + `std`.** The same core runs on bare-metal and on host.

## Memory layers (L0–L7)

| Layer | Name | Typical use |
|-------|------|-------------|
| L0 | Sensory | raw input (sensors, network) |
| L1 | Working | current turn, immediate context |
| L2 | Short-term episodic | recent timestamped turns |
| L3 | Long-term episodic | persistent facts and episodes |
| L4 | Semantic | BQ embeddings + vector recall |
| L5 | Procedural | skills / procedures |
| L6 | (reserved) | — |
| L7 | Identity | persona, preferences, global state |

## `Storage` trait (the central contract)

```rust
pub trait Storage {
    fn put(&mut self, key: &[u8], val: &[u8]) -> Result<(), SgdbError>;
    fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, SgdbError>;
    fn scan_prefix(&mut self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, SgdbError>;
    fn delete(&mut self, key: &[u8]) -> Result<(), SgdbError>;
}
```

- **Semantics:** append-log, power-loss safe. `put` is idempotent; `delete`
  writes a tombstone. The crate guarantees CRC + crash recovery over any impl
  that follows this semantics.
- **Impls shipped in v0.1:**
  - `InMemory` — RAM, for tests and prototyping.
  - `FileStorage` — file append-log, for std/desktop applications.
  - `TickvFile` — byte-exact TKLV records, OS-readable (see Interop).
- **Embedded:** implement over your flash (SPI/NOR/NVMe). The pattern follows
  the embedded-storage ecosystem: implement the trait = integrated.

## Target public API

```rust
pub struct Sgdb { /* engine + art + bq + storage */ }

impl Sgdb {
    pub fn open(backend: impl Storage) -> Result<Self, SgdbError>;

    // ---- turn memory (L1/L2) ----
    pub fn remember_exchange(&mut self, user: &str, response: &str) -> Result<(), SgdbError>;
    pub fn remember_exchange_full(
        &mut self, user: &str, response: &str,
        emb_u: &[f32], emb_a: &[f32], now: u64,
    ) -> Result<(), SgdbError>;

    // ---- semantic (L4, BQ + FP32 rescore) ----
    /// Embedding input policy (P1-1): `emb` must be non-empty, all-finite, and
    /// `emb.len() <= MAX_EMBEDDING_DIM` (4096). NaN/±Inf/empty/oversized →
    /// `SgdbError::Invalid`. A non-finite vector would silently corrupt
    /// ranking (NaN → `x > 0.0` false → bit 0, and FP32 rescore NaN → score 0).
    /// The same policy applies to every `query: &[f32]` parameter on recall*:
    /// non-finite/oversized → `Invalid`; **empty query stays a no-op** (returns
    /// `Ok(Vec::new())`, not an error). `recall_weighted` ranks with `total_cmp`
    /// (total order — NaN weights are legal, sorted deterministically).
    ///
    /// **Centralized limits (P1-3)**: the storage ceilings (`MAX_KLEN` 4096,
    /// `MAX_VLEN` 1 MiB) and the embedding ceiling (`MAX_EMBEDDING_DIM` 4096)
    /// live in one module `neural_sgdb::limits` (re-exported at crate root and
    /// via `tickv::*` / `bq::*` for API compat). Every reader of external data
    /// (FileStorage recovery, `scan_volume`, fast-mount, wire decode) validates
    /// a length field against its ceiling BEFORE allocating — an oversized
    /// field is treated as corrupted tail, never a huge allocation.
    ///
    /// **Property tests (P1-4)**: deterministic LCG harnesses (zero deps —
    /// see src/art.rs, src/conflict.rs, src/memory_doc.rs, src/crdt.rs
    /// `mod prop_tests`) pin: decode∘encode roundtrips (NMD1 doc/record/meta,
    /// CFL1, MDLT/MSNP/SignedEnvelope/CrdtState), ART-vs-BTreeMap differential
    /// (fixed-width keys — no prefix relationship), and the LWW semilattice
    /// laws of `VectorClock::merge` (associative, commutative, idempotent,
    /// monotonic).
    pub fn remember_semantic(&mut self, key: &str, text: &str, emb: &[f32]) -> Result<(), SgdbError>;

    /// L4 recall: coarse BQ top-k -> FP32 rescore -> fine top-k.
    /// Auto-oversample by dimensionality (1 word→16, 2-4→8, else 4).
    pub fn recall(&mut self, query: &[f32], k: usize) -> Result<Vec<Hit>, SgdbError>;

    /// Explicit candidate pool: `oversample·k` Hamming candidates before rescore
    /// (raise the pool when low-dim BQ collides; don't lower `k`).
    pub fn recall_oversampled(&mut self, query: &[f32], k: usize, oversample: usize)
        -> Result<Vec<Hit>, SgdbError>;

    /// Weighted scoring: `w_sem·dist + w_rec·recency(/ts/<hex>) + w_imp·importance(layer)`.
    pub fn recall_weighted(&mut self, query: &[f32], k: usize, w_sem: f32, w_rec: f32,
        w_imp: f32, now: u64) -> Result<Vec<Hit>, SgdbError>;

    /// Lexical BM25 path over L2/L3 texts (dual-path, complements BQ).
    pub fn recall_lexical(&mut self, query_text: &str, k: usize) -> Result<Vec<Hit>, SgdbError>;
    pub fn recall_hybrid(&mut self, query_emb: &[f32], query_text: &str, k: usize)
        -> Result<Vec<Hit>, SgdbError>;

    /// Temporal validity window (`sys/validity/`): invalidate-not-delete.
    pub fn set_validity(&mut self, key: &str, from: u64, until: u64) -> Result<(), SgdbError>;
    pub fn validity_at(&mut self, key: &str, now: u64) -> Result<bool, SgdbError>;
    pub fn invalidate(&mut self, key: &str, now: u64) -> Result<(), SgdbError>;
    pub fn recall_at(&mut self, query: &[f32], k: usize, now: u64) -> Result<Vec<Hit>, SgdbError>;

    /// Read-only access to the BQ index (e.g. `MihIndex::build(&db.bq(), 4)`).
    pub fn bq(&self) -> &BqFlatIndex;

    // ---- RAG: recall + text fetch + formatted string ready for the prompt. ----
    pub fn rag_context(&mut self, query: &[f32], k: usize) -> Result<String, SgdbError>;
    /// Tetos: `rag_context_limited(q, k, oversample, max_bytes)` — contexto
    /// acumulado nunca excede `max_bytes` (0 = sem teto). Defaults:
    /// `rag_context` usa `MAX_RAG_CONTEXT_BYTES` (8192); oversample 0 = padrão.
    pub fn rag_context_limited(
        &mut self,
        query: &[f32],
        k: usize,
        oversample: usize,
        max_bytes: usize,
    ) -> Result<String, SgdbError>;

    // ---- facts (L3, ART by timestamp) ----
    pub fn remember_fact(&mut self, fact: &str, now: u64) -> Result<(), SgdbError>;

    // ---- key index (ART, O(k)) ----
    pub fn scan_prefix(&mut self, prefix: &str) -> Result<Vec<(String, u64)>, SgdbError>;
    /// Página lexicográfica determinística (P1-6): `scan_prefix_page(prefix,
    /// offset, limit)` — offset crescente (0, 100, …) sem materializar tudo.
    /// Ordem garantida entre chamadas; `scan_prefix` legado NÃO garante
    /// lexicográfica (ordem de travessia da ART).
    pub fn scan_prefix_page(
        &mut self,
        prefix: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<(String, u64)>, SgdbError>;

    // ---- lifecycle ----
    pub fn checkpoint(&mut self) -> Result<(), SgdbError>;
    pub fn prune_working_ram(&mut self) -> Result<usize, SgdbError>;
    pub fn backend(&self) -> &'static str;
    pub fn ready(&self) -> bool;

    // ---- memory identity + provenance (v0.6) ----
    /// Stable identity of the memory at `key` (32-hex). `None` if no doc or
    /// pre-v0.6 record without meta yet (a re-put / `set_importance` assigns).
    pub fn memory_id(&mut self, key: &str) -> Result<Option<String>, SgdbError>;
    /// Full metadata: memory_id, source, confidence, importance,
    /// created_tick, parent_ids, clock_overflow.
    pub fn meta(&mut self, key: &str) -> Result<Option<MemoryMeta>, SgdbError>;
    /// Importance [0..1] (normalized; out-of-range clamped, non-finite
    /// rejected). Persistent, queryable, modifiable (Phase 1.3).
    pub fn set_importance(&mut self, key: &str, importance: f32) -> Result<(), SgdbError>;
    /// Confidence [0..1] (same contract as importance).
    pub fn set_confidence(&mut self, key: &str, confidence: f32) -> Result<(), SgdbError>;

    // ---- physical deletion (distinct from logical state) ----
    /// Tombstone + removal from derived indexes (ART/lexical/id→sk) and
    /// side-tables (`sys/state/`, `sys/validity/`, `sys/meta/`). The BQ is a
    /// derived, append-only index: orphan candidates are skipped at recall,
    /// never resurrected. Idempotent (second call = `false`).
    pub fn delete(&mut self, key: &str) -> Result<bool, SgdbError>;
}

// ---- recall-time index extras ----
pub struct MihIndex { /* multi-index hashing over existing bitvecs */ }
impl MihIndex {
    pub fn build(src: &BqFlatIndex, blocks: usize) -> Self;
    pub fn candidates(&self, query: &[u64], probes: usize) -> Vec<usize>;
    pub fn top_k(&self, src: &BqFlatIndex, query: &[u64], k: usize, probes: usize) -> Vec<(u64, u32)>;
    pub fn top_k_f32(&self, src: &BqFlatIndex, query: &[f32], k: usize, probes: usize) -> Vec<(u64, u32)>;
}
pub struct LexicalIndex { /* inverted BM25 over L2/L3 texts (no_std, alloc-only) */ }
impl LexicalIndex {
    pub fn add(&mut self, key: &str, text: &str);
    pub fn remove(&mut self, key: &str);
    pub fn search(&self, query: &str, k: usize) -> Vec<(String, f32)>;
}

pub struct Hit {
    pub key: String,
    pub text: String,
    pub dist: f32,   // 1-cos distance (0 = identical)
    /// Provenance (v0.6): `None` for pre-v0.6 records without meta yet.
    pub provenance: Option<HitProvenance>,
}

pub struct HitProvenance {
    pub memory_id: String,
    pub layer: MemoryLayer,
    pub state: MemoryState,
    pub source: u8,
    pub confidence: f32,
    pub importance: f32,
    pub created_tick: u64,
    pub parent_ids: Vec<String>,
}

pub struct MemoryMeta { /* memory_id, source, confidence, importance,
    created_tick, parent_ids, clock_overflow — persisted in `sys/meta/` */ }

/// Deterministic memory id: FNV-1a 128 over (node_id, created_tick, layer,
/// key) → 32 hex chars. Assigned once at creation, never re-derived.
pub fn generate_memory_id(node_id: u8, created_tick: u64, layer: MemoryLayer,
    key: &str) -> String;
```

## Format decision (v0.6)

**NMD1 stays v1, byte-identical with `neural-os-core`.** Identity/provenance
metadata (`MemoryMeta`, incl. VectorClock overflow beyond 8 nodes) is NOT
serialized into the NMD1 record; it lives in the `sys/meta/<sk>` side-table
(the same pattern already proven by `sys/state/` and `sys/validity/`), is
attached by the engine on `get`, and travels with the doc on replication
(`Sgdb::put` preserves `doc.meta`). No format version bump was needed;
migration of pre-v0.6 records is lazy (identity is assigned on next put or
via `set_importance`/`set_confidence`).

## Injectable seams

| Seam | Today (kernel) | Becomes |
|------|---------------|---------|
| Clock | `k_nano::interrupts::TIMER_TICKS` | `now: u64` parameter on timestamped methods |
| CPU/SIMD | `k_nano::platform_probe::hw_info()` | `std::arch::is_x86_feature_detected!` on host; injectable `cpu_caps()` in no_std |
| Logging | `k_nano::slog_kai!` (serial) | internal macro + optional `log` hook |
| Storage | `k_nano::storage` (NVMe/RAM) | `Storage` trait (above) |

## On-disk format (OS interop)

- **Records:** `TKLV` (klen/vlen/crc32, tombstone V=0) and `TKCK` (checkpoint).
- **MemoryDoc:** `NMD1` (layer, key, 8-node VectorClock, payload, bitvec).
- **Contract:** a volume written by neural-os-core **is read** by neural-sgdb
  and vice versa. Formats are self-describing in the source code; this
  compatibility is an extraction acceptance requirement.

## Format versioning (P2)

Binary layouts are contracts — documented by byte offsets in the source
(`src/memory_doc.rs` for NMD1, `src/tickv.rs` for TKLV/TKCK) and pinned by
**golden byte tests**:

| Format | Golden test | Covers |
|--------|-------------|--------|
| NMD1 | `golden_nmd1_bytes` | magic, layer, klen/key, VectorClock 72B, plen/payload, bitflag |
| TKLV | `golden_record_bytes` | magic, klen/vlen u32le, CRC over key‖val, body pad 16, record pad 512 |
| FNV-1a 64 | `fnv1a64_known_vector` | offset basis + prime (vector `"a"` = `0xaf63dc4c8601ec8c`) |

**Cross-direction tests (P2):**
- OS → crate: golden bytes are derived from the OS spec (`crates/k_nano/src/
  storage/tickv.rs`); `scan_volume` re-parse proves the crate reads OS-written
  streams (incl. tombstones `TKL\0`/`vlen=0` and corrupt-hunt).
- crate → OS: `TickvFile` writes 512-aligned TKLV records byte-exact, with
  TKCK checkpoint fast-mount (`try_mount_from_ckpt`) and full `scan_volume`
  fallback; the OS mounts by full scan (`recover()`).
- A true bidirectional test running the OS's own reader on host is deferred
  until the OS publishes its TickvLite reader as a crate.

**Format changelog:** initial public release v0.1 — NMD1 and TKLV/TKCK as
extracted from the OS (ADR-0063). v0.7 — **MDM1 v1 → v2** (side-table meta
codec): adds `version_id` (per-version identity, Phase 3); v1 records decode
with `version_id = memory_id` (explicit migration, never silent). NMD1 and
TKLV/TKCK unchanged. Any layout change MUST bump a version marker and update
the golden tests in the same commit.

## Feature matrix (P2)

| Feature | Gates | Default |
|---------|-------|---------|
| `std` | base std support (`sgdb_log!` eprintln, `SgdbError: std::error::Error`) | ✅ |
| `file-storage` | `FileStorage` + `TickvFile` backends | ✅ |
| `simd-runtime` | `cpu_caps()` auto-detect via `std::arch::is_x86_feature_detected!` | ✅ |
| `p2p` | CRDT sync (`CrdtMemorySync`, `Transport`, `UdpTransport`) | ❌ opt-in |

`mcp_server` and `bench`/`stress` are examples (dev-dep `serde_json`);
`bench` and `stress` require `file-storage` (`required-features` in
Cargo.toml).

**CRDT transport security boundary:** the core is transport-agnostic
(`trait Transport`) and implements NO cryptography. `SignedEnvelope`
(payload + node identity + opaque `auth`) is the wire envelope for
*authenticated* transports (HMAC/ed25519/TLS, verified outside the core); the
shipped `UdpTransport` is an **unauthenticated development demo** that does
not use it — replace in production.

## CRDT per-layer merge policy (IMPLEMENTED v0.6)

`MergePolicy::for_layer(layer)` is the single point of truth (explicit table,
not one universal LWW rule):

| Layer | Policy | Semantics |
|---|---|---|
| L0 Sensory | `LocalOnly` | strictly local — remote never adopted (`Rejected`) |
| L1 Working | `LocalWorking` | local-only working memory |
| L2/L3 Episodic | `MultiValueRegister` | concurrent versions BOTH retained |
| L4 Semantic | `CausalLwwWithHistory` | causally-dominant wins; older superseded |
| L5 Procedural | `ControlledLww` | LWW with safeguards, history preserved |
| L6 Reserved | `Reserved` | no merge semantics defined |
| L7 Identity | `ControlledLww` | controlled identity state |

The policy is consulted at two levels: version sync
(`apply_remote_version_with_policy`; local layers return `MergeVerdict::Rejected`)
and record merge (`Sgdb::merge_remote` — Applied/Stale/Duplicate/Conflict/
Rejected). **Concurrent same-key memories are never silently overwritten** —
`merge_remote` returns `Conflict`, preserves the local value and leaves the
resolution to the cognitive layer (roadmap Phase 14/15).

## Causal DAG (v0.7 — Phase 3)

- **`MemoryMeta.version_id`** — per-version identity, distinct from
  `memory_id` (stable slot identity). Every local overwrite of a slot
  advances `version_id` and records the previous version in `parent_ids`
  (lineage). Replicated docs keep the creator's version.
- **`sys/version/<version_id>`** reverse index → (storage key + the version's
  OWN meta), so a parent resolves to the version it actually was, not the
  slot's current meta. Derived state: written on persist, rebuilt on
  `rebuild_indices`, removed on delete.
- **`Sgdb::version_of(key)` / `Sgdb::lineage(key)`** — current version id and
  the causal chain (version_id, memory_id, key, source, created_tick,
  parents per entry; cycle-guarded). `supersede` links the current version;
  `HitProvenance.version_id` exposes it in recall.

## Replication primitives (v0.6)

- **`MemoryRecord`** — one memory as a unit: doc NMD1 + `MemoryState` +
  validity window + `MemoryMeta` (identity/provenance). Wire `MDR1`,
  bounds-checked decode (never panics).
- **`Sgdb::export_record(key)` / `Sgdb::import_record(record)`** — export a
  record with its side-tables; import WITHOUT ticking the local clock (the
  receiver never becomes an author of someone else's memory). Pre-v0.6
  records get a deterministic identity derived from the clock's author.
- **`Sgdb::merge_remote(record)`** — policy-aware import (see table above).
- **`MemoryDelta` / `MemorySnapshot`** — wire codecs (`MDLT`/`MSNP`) that
  carry `Vec<MemoryRecord>`, replacing the pre-v0.6 stubs (`docs: Vec<u8>`).
  **Wire-safety (P1-2)**: every wire type (`ConflictRecord`, `MemoryDelta`,
  `MemorySnapshot`, `SignedEnvelope`, `CrdtState`) exposes `try_encode() ->
  Result<Vec<u8>, &'static str>` which validates every length/count field
  before casting — a field that does not fit the wire format returns `Err`
  instead of silently truncating (which would desynchronize the stream on
  decode). `encode()` remains as the infallible convenience (panics on
  overflow); production write paths use `try_encode()` and propagate the
  error. `decode()` stays bounds-checked (never panics).
- **`CrdtMemorySync::missing_after(peer_versions)`** — the causal range a
  peer lacks (what to request in a delta protocol).
- Version 0 packets are ignored (a relay node's heartbeat never creates
  phantom conflicts); `local_version` counts only own writes.

## Anti-entropy (v0.7 — Phase 6)

- **`CrdtMemorySync::announce()` / `known_clock()` / `node_id()`** — the
  full known clock (own version + relayed peer versions) is what each node
  publishes each round; versions and records cross intermediate nodes
  (gossip/relay).
- **`Sgdb::keys_for_clock(node, counter)`** — derived index `(node, counter)
  → storage keys`: the docs of a specific causal version, the basis of the
  directed pull. Rebuilt on `rebuild_indices`, removed on delete.
- **Directed pull of the missing causal range** — for each announced
  `(node, v)` the receiver pulls only `known+1..=v`, so a peer joining after
  several writes fetches the whole series and repeated sync is idempotent.
- **`CrdtState` (`state()` / `restore()`)** — durable replication metadata:
  node identity, local counters, known peer versions (wire `CRDT`,
  bounds-checked). `restore` refuses a foreign node_id. Persist via
  `Sgdb::read_side_bytes`/`write_side_bytes` (e.g. `sys/crdt/…`) so a
  restarted node does not regress its clock.
- **One logical write = one causal version** — `remember_semantic` writes its
  L2 text companion under the same counter as L4 (`put_companion`), keeping
  the CRDT version aligned with the per-doc clock (without this the directed
  pull loses companion docs).

## L6 associative memory (v0.8 — Phase 8)

- **`RelationKind`** — `RelatedTo`, `Causes`, `Supports`, `Contradicts`,
  `DerivedFrom`, `Supersedes`. Memory-native relations: persisted in
  side-table `sys/rel/<kind>/<a>#<b>` (storage = source of truth) and
  indexed in the ART (forward `rel/…` + reverse `rev/…`, derived, rebuilt
  on open, pruned on delete — a deleted memory never keeps topology).
- **`Sgdb::associate(a, rel, b)`** — asserts `a --rel--> b` (idempotent;
  rejects keys containing the reserved `#`). **No inference**: the upper
  layer (agent/LLM) asserts, SGDB stores.
- **`Sgdb::related_to(key)`** — both directions, all kinds, deterministic by
  `(kind, target)`. **`causes` / `supports` / `contradicts` /
  `derived_from(key)`** — outgoing targets per kind.
- Relations survive reopen (reindexed from storage); no doc is required for
  either endpoint.

## Provenance-aware recall (v0.8 — Phase 9)

- **`recall(query, k)` defaults to ACTIVE memories only** — `Superseded` /
  `Archived` / `Decayed` / `Invalidated` are filtered *before* ranking (they
  never consume top-k slots). A superseded memory is never silently
  presented as active.
- **`recall_historical(query, k)`** — includes inactive memories with
  `Hit.provenance.state` exposed (explicit historical query). Same for
  `recall_lexical` vs `recall_lexical_historical`.
- `recall_at` (validity) composes on top; `recall_weighted` and
  `rag_context` inherit the active-only default.

## Lifecycle engine (v0.8 — Phase 15/16)

- **`MemoryLifecycle::new(config)` / `tick(&mut db, now)`** — deterministic
  (`now` is explicit; no hidden wall clock, no background thread; the caller
  schedules). Returns a structured `LifecycleReport` (tick, committed,
  promoted, semanticized, archived, decayed).
- **`LifecycleConfig`** — `l1_commit_after_ticks`, `l2_to_l3_importance` /
  `l2_to_l3_min_age_ticks`, `l3_to_l4_importance` /
  `l3_to_l4_min_age_ticks`, `decay_per_tick` (0.0 = off),
  `decayed_below`, `archive_superseded_after_ticks` (None = off).
- Transitions per tick: **L1→L2 commit** (origin Archived), **L2→L3
  promotion** (importance + age), **L3→L4 heuristic semanticization** (the
  L4 doc is created WITHOUT a bitvec — embeddings are the upper layer's job;
  the core never generates semantic representations), **decay**
  (importance decays; below threshold → `Decayed`, never deleted),
  **archive** (aged superseded → Archived). **L4→L5 is never automatic**
  (proceduralization requires explicit upper-layer/HITL decision).
- Every promotion wires `parent_ids += [source version]` and the L6
  relation `new --derived_from--> old` (causal DAG + topology).
  Idempotent: a source is only promoted while `Active`.
- **`Sgdb::add_parents(key, ids)`** — appends lineage parents to a memory's
  meta (also the base of v0.9's `merge_memories`).

## Conflict model (v0.9 — Phase 14/15)

- **`ConflictRecord`** — first-class conflict: deterministic `conflict_id`
  (FNV-1a 128 over subject + sorted candidates), `subject` (storage key),
  `candidates` (version IDs), `nodes` (source nodes), `created_tick`,
  `status` (Open/Resolved), `resolved_winner`, and `records: Vec<Vec<u8>>`
  (MDR1-encoded `MemoryRecord` per candidate, parallel to `candidates`) —
  evidence is self-contained: resolving does not require re-fetching the
  remote node. Wire `CFL1 v1`, bounds-checked decode.
- **`Sgdb::merge_remote`** — on CONCURRENT verdict, upserts a
  `ConflictRecord` in `sys/conflict/<id>` with both candidates' evidence;
  re-delivery of the same concurrent pair upserts (never duplicates).
- **`Sgdb::conflicts()` / `conflict(id)` / `dismiss_conflict(id)`** —
  enumerate open/resolved conflicts, inspect, or clean up after resolution.
  **Convergence note (P2-1)**: a `ConflictRecord` is LOCAL evidence, created
  where the concurrent merge happened — it is NOT the replication unit
  (`MemoryRecord` MDR1), so conflict records do not necessarily converge
  across a mesh. What converges is the causal content (byte-identical
  `MemoryRecord` per key in all nodes) plus no-lost-version (each author
  keeps its own value). Convergence is verified by random-topology mesh
  tests (LCG-generated directed graphs, partitions/rejoins).
- **`Sgdb::resolve_conflict(conflict_id, winner_vid)`** — explicit
  upper-layer decision: imports the winner's `MemoryRecord` via evidence,
  sets it as the current version of the slot (`version_id = winner_vid`),
  appends all losers to `parent_ids` (causal lineage), marks the conflict
  `Resolved`. Idempotent (already Resolved = `Ok`). Winner validation:
  must be a candidate or `Err`.
- **`engine.put_conflict` / `get_conflict` / `list_conflicts` /
  `delete_conflict`** — raw side-table helpers (`sys/conflict/<id>`).
- The core never decides semantic truth; it detects, preserves, and executes
  explicit decisions.

## Cognitive API (v0.9 — Phase 12/17/23)

- **`reinforce(key, delta)`** — `importance += delta` (clamped [0, 1]);
  `last_reinforced = own_counter` (MDM1 v3). Does NOT tick the clock —
  reinforcement is local cognitive metadata. Persisted in `sys/meta/`.
  Decay (v0.8 lifecycle) uses importance, so reinforced memories decay
  slower.
- **`forget(key)`** — archives a memory (`MemoryState::Archived`). History
  is preserved (accessible via `recall_historical`, `lineage`); the memory
  is removed from default active recall. For physical removal, use
  `delete`.
- **`explain(key)` → `MemoryExplanation`** — structured, machine-readable
  explanation of a memory's current state: `key`, `layer`, `state`,
  `memory_id`, `version_id`, `source`, `confidence`, `importance`,
  `created_tick`, `last_reinforced`, `parents`, `validity`, `children`
  (versions that list this as parent — derived from the `sys/version/`
  index). No human-readable narrative: the upper layer formats.
- **`transfer_to(key, target_layer)`** — moves a memory to another layer
  with full lineage: the new doc gets `parent_ids += [source version_id]`
  and the L6 relation `new --derived_from--> old` is asserted; the source
  is archived (never deleted). Idempotent (same layer = no-op). Generalizes
  lifecycle promotion for arbitrary layer moves.
- **`merge_memories(a, b, target)`** — creates a new memory C at `target`
  (or auto-generated key) with `parent_ids = [A.version_id, B.version_id]`,
  payload = A payload + 0x1F separator + B payload, `importance = max(A, B)`,
  `confidence = max(A, B)`. A and B remain intact (history preserved).
  Roadmap Phase 16 — no semantic merging; concatenation + lineage.
- **`engine.add_parents(sk, parents)`** — low-level lineage append
  (also the base of `merge_memories` and lifecycle promotion).
- **`engine.scan_versions()`** — iterates `sys/version/` index;
  returns `(version_id, storage_key, MemoryMeta)` tuples (used by `explain`
  for children enumeration).
- **`engine.own_counter()`** — current own-clock watermark (used by
  `reinforce`).
- **`MemoryExplanation`** struct — `key`, `layer`, `state`, `memory_id`,
  `version_id`, `source`, `confidence`, `importance`, `created_tick`,
  `last_reinforced`, `parents`, `validity`, `children`.

## What does NOT go public

- **OS namespaces:** `hanr/`, `pkg/`, `audit/`, `sys/`, `hw/` are AIOS-specific
  and stay internal to the kernel. The community crate exposes only the memory
  model (`md/L0`–`md/L7`).
- **CRDT sync (network):** becomes the optional `p2p` feature — degrades to
  local-only when off.
- **OS residuals:** 10M/100k benchmark, HW kill-9, AVX-512 CI — are parent-OS
  goals; the crate publishes its own benchmarks when they exist.

## Example (README showcase)

```rust
use neural_sgdb::{Sgdb, FileStorage};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = Sgdb::open(FileStorage::open("agent_memory.db")?)?;

    db.remember_exchange("how's the weather?", "sunny, 24 degrees")?;
    db.remember_semantic("turn:1", "sunny weather in sao paulo", &emb)?;

    let hits = db.recall(&query_emb, 5)?;
    let ctx = db.rag_context(&query_emb, 3)?;
    println!("{ctx}");
    Ok(())
}
```

## Migration map (internal → public)

| Internal (`k_ai::sgdb`) | Public (`neural_sgdb`) | Change |
|--------------------------|--------------------------|--------|
| `init_global(1)` / `ensure_ready()` | `Sgdb::open(backend)` | global static → instance |
| `remember_exchange(u, r)` | `Sgdb::remember_exchange(u, r)` | wrapper |
| `remember_semantic(k, t, emb)` | `Sgdb::remember_semantic(k, t, emb)` | wrapper |
| `recall_semantic(q, k) -> (Vec<(String,u32)>, &'static str)` | `recall(q, k) -> Vec<Hit>` | return type |
| `rag_context(q, k) -> String` | `rag_context(q, k) -> Result<String>` | error |
| `remember_fact(f)` (uses TIMER_TICKS) | `remember_fact(f, now)` | injected clock |
| `put_kv` / `get_kv` | via `Storage` trait | backend |
| `slog_kai!` | internal `log` macro | seam |
| `store::ns::{hanr,pkg,...}` | removed | OS-only |

## Extraction acceptance criteria

- [x] `cargo test` in the neural-sgdb repo passes (host) with `InMemory` + `FileStorage` + `TickvFile`
- [x] `cargo check --no-default-features --target x86_64-unknown-none` passes (no_std, alloc-only, zero deps)
- [x] `FileStorage` roundtrip: put → reopen → get, survives a simulated crash
- [x] **Document interop (v0.1):** `MemoryDoc` (NMD1) — encode/decode
      byte-identical to the OS (`crates/k_ai/src/sgdb/memory_doc.rs`); a
      document written by one is read by the other
- [x] **Storage interop (v0.1.1):** byte-exact TKLV/TKCK codec of the OS
      TickvLite in `src/tickv.rs` (`encode_record`/`scan_volume`/`encode_ckpt`/
      `fnv1a64`) + `TickvFile` backend (512-aligned records, IEEE CRC32 over
      key‖val, tombstone `TKL\0`/`vlen=0`, EOF all-0x00/0xFF). Verified by a
      golden bytes test + `scan_volume` re-parse (same semantics as the OS
      `recover()`) + InMemory parity. Note: `TickvFile` writes TKCK
      checkpoints (v0.7+) with fast-mount (`try_mount_from_ckpt`, FNV-1a index
      check, per-entry CRC + stale check, ckpt-must-be-last) and a full
      `scan_volume` fallback; GC/compaction rewrites live set + ckpt + atomic
      rename.
- [x] Zero `k_nano` / kernel dependency in the crate code

## Note — relationship with the OS (Mode 1)

Separate repo, independent evolution. neural-os-core **keeps** its internal
`k_ai::sgdb` (AGPL) — there is no wiring (path dep or version) at this point.
The two compatibility points are: **NMD1 document format** (byte-identical) and
**TKLV/TKCK storage format** (`src/tickv.rs`, byte-exact). If the OS ever
consumes the repo's product, it will be via a published crates.io version, not
filesystem coupling.
