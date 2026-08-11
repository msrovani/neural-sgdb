# neural-sgdb — API Contract

> Contract document for the extraction of the SGDB core from neural-os-core.
> Status: **v0.1 implemented** — this document is the current public contract;
> v0.2+ items are explicitly marked as roadmap. The internal API lives in
> `crates/k_ai/src/sgdb/` of the parent OS; this doc defines the public surface
> the community crate exposes (and already ships).

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

    // ---- facts (L3, ART by timestamp) ----
    pub fn remember_fact(&mut self, fact: &str, now: u64) -> Result<(), SgdbError>;

    // ---- key index (ART, O(k)) ----
    pub fn scan_prefix(&mut self, prefix: &str) -> Result<Vec<(String, u64)>, SgdbError>;

    // ---- lifecycle ----
    pub fn checkpoint(&mut self) -> Result<(), SgdbError>;
    pub fn prune_working_ram(&mut self) -> Result<usize, SgdbError>;
    pub fn backend(&self) -> &'static str;
    pub fn ready(&self) -> bool;
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
}
```

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
- crate → OS: `TickvFile` writes 512-aligned TKLV records byte-exact; the OS
  mounts by full scan (`recover()` fallback, no checkpoint in v0.1).
- A true bidirectional test running the OS's own reader on host is deferred
  until the OS publishes its TickvLite reader as a crate.

**Format changelog:** initial public release v0.1 — NMD1 and TKLV/TKCK as
extracted from the OS (ADR-0063). Any layout change MUST bump a version marker
and update the golden tests in the same commit.

## Feature matrix (P2)

| Feature | Gates | Default |
|---------|-------|---------|
| `std` | base std support (`sgdb_log!` eprintln, `SgdbError: std::error::Error`) | ✅ |
| `file-storage` | `FileStorage` + `TickvFile` backends | ✅ |
| `simd-runtime` | `cpu_caps()` auto-detect via `std::arch::is_x86_feature_detected!` | ✅ |
| `p2p` | CRDT sync (`CrdtMemorySync`, `Transport`, `UdpTransport`) | ❌ opt-in |

`mcp_server` and `bench` are examples (dev-dep `serde_json`), not features.

## CRDT per-layer merge policy (P3 — roadmap)

v0.1 uses symmetric LWW (last-write-wins) for all layers — correct for
configuration/state records, but LWW can *erase* conflicting episodic memories
that should coexist as perspectives or versions. Roadmap v0.2 design:

- LWW only for config/state records (L7 identity, `sys/*`)
- multi-value register for episodic memories (L2/L3) — coexist on conflict
- causal merge with the existing `VectorClock` (8 nodes)
- per-layer reconciliation: L1 replaces, L2/L3 accumulate, L4 reindexes,
  L7 requires HITL/trust

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
      `recover()`) + InMemory parity. Note: `TickvFile` does not write
      checkpoints (v0.1) — the OS mounts by full scan; GC/compaction is v0.2
- [x] Zero `k_nano` / kernel dependency in the crate code

## Note — relationship with the OS (Mode 1)

Separate repo, independent evolution. neural-os-core **keeps** its internal
`k_ai::sgdb` (AGPL) — there is no wiring (path dep or version) at this point.
The two compatibility points are: **NMD1 document format** (byte-identical) and
**TKLV/TKCK storage format** (`src/tickv.rs`, byte-exact). If the OS ever
consumes the repo's product, it will be via a published crates.io version, not
filesystem coupling.
