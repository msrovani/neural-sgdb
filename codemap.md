# Repository Atlas: neural-sgdb

## Project Responsibility
Persistent, transferable memory database for AI agents — **memories, not data**.
Core extracted from neural-os-core (`k_ai::sgdb`, ADR-0063) as an independent
community project: zero dependencies, dual-mode `no_std` + `std`, MIT OR
Apache-2.0 license. 8 memory layers L0–L7, semantic recall BQ + FP32 rescore,
O(k) ART index, pluggable storage, CRDT sync (`p2p` feature), MCP server.

## System Entry Points
- `src/lib.rs`: crate root — `cfg_attr(not(feature="std"), no_std)`, public
  re-exports (`Sgdb`, `Storage`, `MemoryDoc`, `ArtIndex`, `BqFlatIndex`,
  `TickvFile`, …)
- `Cargo.toml`: features `default=["std","file-storage","simd-runtime"]`, `p2p=["std"]`; dev-dep `serde_json` (examples only)
- `docs/api.md`: API contract + migration map (Storage trait, seams, acceptance)
- `examples/bench.rs`: measured benchmarks (ART P50/P99, BQ, recall vs FP32) — numbers/methodology in `BENCHMARKS.md`
- `examples/mcp_server.rs`: MCP server for AI agents (JSON-RPC 2.0 stdio)

## Directory Map (Aggregated)
| Directory | Responsibility Summary | Detailed Map |
|-----------|------------------------|--------------|
| `src/` | Memory DB core: MemoryDoc NMD1 + MemoryMeta MDM1 v6, ART, BQ + Hamming SIMD, ctype (payload typing), engine, Sgdb facade, Embedder/era seams, Storage trait, TKLV codec, CRDT p2p | [View Map](src/codemap.md) |
| `examples/` | Showcase: bench (P50/P99 latency, recall vs FP32), MCP server (23 tools, typed hits), stress/audit, hot test client, agent/decision + memory-arena eval, machine→machine protocol, p2p mesh/signed | [View Map](examples/codemap.md) |
| `docs/` | API contract (`api.md`), architecture (v1.1.6), implementation status, ADRs, release notes | — |

## Format Contracts (interop with neural-os-core)
- **NMD1** (`src/memory_doc.rs`): memory document — magic `NMD1`, layer u8,
  klen u32 LE, key, VectorClock 72B, plen u32 LE, payload, bitvec flag
- **TKLV/TKCK** (`src/tickv.rs`): storage — 16B header (`TKLV` + klen/vlen/crc32
  u32 LE), IEEE CRC32 over key‖val, body pad 16, record pad 512, tombstone
  `TKL\0`/`vlen=0`, checkpoint `TKCK` + FNV-1a 64

## Seams (injectable — replace the origin kernel)
- Clock: methods take `now: u64` (no internal clock)
- SIMD: `cpu_caps()` (std auto-detect) / `set_cpu_caps()` (no_std)
- Log: `sgdb_log!` (no-op no_std, eprintln std)
- Storage: `Storage` trait (put/get/scan_prefix/delete) — `InMemory`, `FileStorage`, `TickvFile`

## Quick Start (for AI agents)
```rust
use neural_sgdb::{Sgdb, InMemory};
let mut db = Sgdb::open(InMemory::new())?;
db.remember_exchange("how's the weather?", "sunny, 24 degrees")?;
let hits = db.recall(&query_emb, 5)?; // query_emb: &[f32] supplied by caller
```
- `recall` requires caller-supplied embeddings (the crate does not generate
  them — MCP demo uses trigrams)
- Persistence: `FileStorage::open(path)` or `TickvFile::open(path)` (OS format)
