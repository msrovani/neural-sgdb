# AGENTS.md — neural-sgdb

Guide for AI agents (OpenCode, Cursor, Windsurf, Claude Code) working in this
repo. **Read `codemap.md` (atlas) and `docs/api.md` (contract) before editing
code.**

## Repository Map

A full codemap is available at `codemap.md` in the project root.

Before working on any task, read `codemap.md` to understand:
- Project architecture and entry points
- Directory responsibilities and design patterns
- Data flow and integration points between modules

For deep work on a specific folder, also read that folder's `codemap.md`
(`src/codemap.md`, `examples/codemap.md`).

## What it is

Memory database for AI agents (**memories, not data**): documents with a
cognitive layer L0–L7, semantic recall BQ + FP32 rescore (no FAISS/HNSW), O(k)
ART index, pluggable storage, optional CRDT sync, MCP server. Extracted from
neural-os-core as a community project: **zero deps, `no_std` + `std`, MIT OR
Apache-2.0**. OS interop via byte-identical NMD1 and TKLV formats.

## Development rules

1. **Zero dependencies in the lib** — only `alloc` (no_std) / `std`. Examples
   may use dev-deps (`serde_json`). Do not add deps to `[dependencies]`.
2. **`no_std` is a contract** — `cargo check --no-default-features --target
   x86_64-unknown-none` must ALWAYS pass. `deny(warnings)` in no_std elevates
   dead-code to error: use explicit `#[allow(dead_code)]` on port-parity.
3. **`f32::sqrt` does NOT exist in core** for that target — use `sqrt_f32`
   (Newton, in `sgdb.rs`) or `libm` (not in this crate).
4. **ART does not support prefix keys** — keys where one is a prefix of another
   break silently; use fixed-width suffixes.
5. **Formats are contracts** — NMD1 (`memory_doc.rs`) and TKLV (`tickv.rs`) are
   byte-identical to the OS; do NOT change encode/decode/layout without
   updating the OS.
6. **Seams, not globals** — clock via `now: u64`, SIMD via `cpu_caps()`/
   `set_cpu_caps()`, log via `sgdb_log!`. No global engine statics.
7. **Verification** — `cargo test` (31+1 default, 35+1 `--features p2p`) and
   `cargo check` (std + no_std) before committing.

## Quick API

```rust
use neural_sgdb::{Sgdb, InMemory, FileStorage, Storage, SgdbError};
use neural_sgdb::{MemoryLayer, MemoryDoc, ArtIndex, BqFlatIndex};

// open (InMemory for tests; FileStorage to persist; TickvFile for OS format)
let mut db = Sgdb::open(FileStorage::open("mem.db")?)?;

// memories
db.remember_exchange("user", "response")?;             // L1 + L2
db.remember_semantic("k", "text", &emb)?;              // L4 BQ (emb: &[f32])
db.remember_fact("fact", now)?;                        // L3 timestamped
db.checkpoint()?; db.prune_working_ram()?;             // flush L0/L1 RAM

// recall (requires caller-supplied embeddings)
let hits: Vec<Hit> = db.recall(&query_emb, 5)?;        // coarse BQ + FP32 rescore
let ctx = db.rag_context(&query_emb, 3)?;              // string ready for the prompt
let facts = db.scan_prefix("md/L3/")?;                 // ART prefix scan
```

## Testing the examples

```bash
cargo run --release --example bench        # benchmarks (ART/BQ/recall vs FP32)
cargo run --release --example mcp_server   # MCP server for AI agents
```

## Running tests

```bash
cargo test                                 # 31+1 tests (InMemory/FileStorage/TickvFile)
cargo test --features p2p                  # 35+1 (includes CRDT sync)
cargo check --no-default-features --target x86_64-unknown-none   # no_std gate
```

## Repo-specific gotchas

- **MCP server** (`examples/mcp_server.rs`): stdout JSON-RPC ONLY (logs →
  stderr), one message per `\n` line, `2025-11-25` handshake, do not gate tools
  on `notifications/initialized` (Claude Code sends tools/list first), echo the
  id verbatim, `-32601` for unknown methods (modern-client fallback). The
  `demo_embed` embedding is a trigram hash — NOT a real semantic model.
- **TickvFile** (`src/tickv.rs`): does not write checkpoints (v0.1) — the OS
  mounts by full scan; GC/compaction is v0.2. 512-aligned records, tombstone
  `vlen=0` or `TKL\0`. **`scan_volume` MUST skip in-place tombstones
  (`hdr[3]==0`) before CRC** — otherwise OS-written deletes resurrect (bughunt
  #1 CRÍTICO, fixed).
- **CRDT** (`src/crdt.rs`): rate-limit uses `Option<u64>` (the 0 sentinel fails
  on first sync at now=0); `UdpTransport` is an unauthenticated demo — use a
  signed transport in production. `set_cpu_caps` must rearm `SELECTED`
  (bughunt #9).
- **Storage CRC** (`src/storage.rs`): FileStorage CRC covers **key‖val**, not
  just the key — bit rot in values must be detected (bughunt #2).
- **Recall sort** (`src/sgdb.rs`): sort by the raw u32 score (FP32 0..10000 vs
  ham 0..64 share the OS ordering space); `sk.replacen("/L4/", "/L2/", 1)` for
  the companion-text lookup — a key containing `/L4/` must not be corrupted
  (bughunt #3/#6).
- **clamp** (`src/sgdb.rs`): truncate at a char boundary — `&s[..max]` panics
  mid multi-byte char (bughunt #7).
- **Bench baseline** (`examples/bench.rs`): recall@k must compare against true
  FP32 cosine over the original f32 vectors, never hamming over the same
  quantized bits (tautological, bughunt #4).
- **Features** (Cargo.toml): `std`, `file-storage`, `simd-runtime`, `p2p`
  (opt-in). Default = `["std","file-storage","simd-runtime"]`. no_std gate:
  `cargo check --no-default-features --target x86_64-unknown-none`.
- **Format contracts**: NMD1/TKLV byte-identical to the OS — golden tests
  (`golden_nmd1_bytes`, `golden_record_bytes`, `fnv1a64_known_vector`) pin the
  layout; change them in the same commit as any format change.
