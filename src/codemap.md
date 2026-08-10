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
| `memory_doc.rs` | `MemoryDoc` (NMD1 format), zero-copy `MemoryDocView`, `VectorClock`, `MemoryLayer` L0–L7 | Binary contract byte-identical to the OS |
| `art.rs` | `ArtIndex` — Adaptive Radix Tree Node4/16/48/256, prefix scan, tombstone delete | Radix Tree (Leis 2013) |
| `bq.rs` | `BqFlatIndex` — binary quantization (1-bit) + top-k by Hamming | Quantized flat scan |
| `hamming_dispatch.rs` | SIMD dispatch scalar/AVX2/AVX-512 (`#[target_feature]`), seam `cpu_caps()`/`set_cpu_caps()` | Runtime strategy |
| `engine.rs` | `AiosDatabaseEngine` — RAM L0/L1 + Storage L2–L7, ART/BQ indexing, rebuild | Persistence engine |
| `sgdb.rs` | `Sgdb` — public facade: remember/recall/rag_context/checkpoint; `Hit` | Facade (port of layers.rs) |
| `storage.rs` | `Storage` trait (4 methods) + `InMemory` + `FileStorage` (CRC32 append-log crash-safe) + `SgdbError` | Pluggable trait / Strategy |
| `tickv.rs` | Byte-exact TKLV/TKCK codec of the OS TickvLite + `TickvFile` backend | Format interop |
| `crdt.rs` | `CrdtMemorySync` (LWW) + `Transport` trait + `UdpTransport` (`p2p` feature) | CRDT / Observer |

## Flow
1. `Sgdb::open(backend: impl Storage)` → creates `AiosDatabaseEngine` +
   `rebuild_indices_from_storage` (scan `md/`, re-index ART/BQ)
2. `remember_*` → `MemoryDoc::encode` (NMD1) → RAM L0/L1 or `Storage::put`
   (L2–L7) → `index_doc` (ART for keys, BQ for L4/L5 with bitvec)
3. `recall(query: &[f32], k)` → `BqFlatIndex::top_k_f32` (coarse BQ) → FP32
   rescore (1−cos distance) → fine top-k → `Hit { key, text, dist }`
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
