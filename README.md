# neural-sgdb

**Persistent, transferable memory database for AI agents.**

> Memories, not data.

`neural-sgdb` is a memory substrate for AI systems: what it stores, syncs and
transfers are **memories** — with cognitive layer, vector clock and identity —
not generic data packets.

Born inside [neural-os-core](https://github.com/msrovani/neural-os-core), a
bare-metal OS with AI from boot, this project is the independent extraction of
its memory management system (SGDB) for community use.

## What it does

- **8 memory layers (L0–L7):** Sensory → Working → Short/Long-term Episodic →
  Semantic → Procedural → Identity
- **Semantic `remember` / `recall`:** binary-quantized vector search (BQ) with
  SIMD dispatch (AVX-512 / AVX2 / scalar), no external dependencies (no FAISS,
  no HNSW)
- **Memory transfer between nodes:** CRDT synchronization (last-write-wins) —
  memories travel between agents/instances with versioning, not packets
- **Power-loss safe persistence:** append-log with CRC; memory survives
  crash/restart (checkpoint/restore)
- **O(k) key/fact lookup:** ART (Adaptive Radix Tree) index Node4→16→48→256,
  no rebalancing
- **`no_std` + `std`:** runs on bare-metal and host applications — one core

## Why memories?

AI agents today have ephemeral context. `neural-sgdb` gives them a persistent
brain: memory layers with real semantics, microsecond semantic recall and the
ability to **transfer memories between instances** — no SQL, no traditional
filesystem, no external runtime.

## Status

**v0.1 extracted** ✅ — the portable core lives in this repo as the
`neural-sgdb` crate, dual-mode (`no_std` + `std`, zero dependencies):

- `cargo test` on host: **30 tests + doc-test passing**
- `cargo check --no-default-features --target x86_64-unknown-none`: **clean**
- Ported: ART (Node4/16/48/256 + SSE), MemoryDoc L0–L7 (NMD1 format
  byte-identical to the parent OS), BQ + Hamming SIMD (AVX-512/AVX2/scalar),
  instance-based engine
- New: `Storage` trait + `InMemory` + `FileStorage` (CRC32 append-log,
  crash-safe) + `TickvFile` (TKLV byte-exact OS format) + `Sgdb` facade
  (`remember_exchange`, `remember_semantic`, `recall`, `rag_context`,
  `remember_fact`, `scan_prefix`, `checkpoint`)
- Full API contract in [`docs/api.md`](docs/api.md)

The reference implementation runs on bare-metal in the parent OS
(`k_ai::sgdb`, AGPL); this repo evolves separately (MIT OR Apache-2.0).

## Quick start

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

## MCP (AI agents)

`cargo run --release --example mcp_server` exposes `remember` / `recall` /
`rag_context` as MCP tools (JSON-RPC 2.0 over stdio, `2025-11-25` handshake) —
connectable to Claude Code, Cursor and OpenCode:

```bash
# Claude Code
claude mcp add neural-sgdb -- cargo run --release --example mcp_server
```

⚠️ The MCP recall embedding is a **demo** (character-trigram hash); for real
semantic recall, provide your own embeddings via `remember_semantic` / `recall`.

## Benchmarks

`cargo run --release --example bench` — local environment numbers (AVX2):
ART get P50≈200ns, ART insert P50≈800ns, BQ top-5 ≈310µs over 10k×1024 dims,
recall@5 BQ vs FP32-exact = 100% (measured 1-bit quantization trade-off).

## License

Licensed under **MIT** **or** **Apache-2.0** (dual license), your choice.

## Roadmap

- [x] Portable core extraction (ART, MemoryDoc L0–L7, BQ + Hamming SIMD)
- [x] Pluggable Storage trait (InMemory + FileStorage) and injectable clock/CPUID
- [x] CRDT memory sync as optional `p2p` feature (`CrdtMemorySync` +
      `Transport` trait + std `UdpTransport`; symmetric LWW merge)
- [x] Published benchmarks (`cargo run --release --example bench` — ART
      P50/P99, BQ top-k, recall BQ vs FP32)
- [x] MCP server layer (`cargo run --release --example mcp_server` — exposes
      `remember`/`recall`/`rag_context` to AI agents via MCP over stdio;
      trigram demo embedding)
- [x] **Byte-exact TKLV/TKCK storage interop with the OS** (`src/tickv.rs`:
      byte-exact codec + OS-readable `TickvFile` backend; golden test +
      `scan_volume` re-parse; NMD1 and TKLV interoperable)

## Interop with neural-os-core

- **NMD1 (document):** `MemoryDoc` encode/decode byte-identical to the OS
- **TKLV/TKCK (storage):** `tickv::encode_record`/`scan_volume` replicate the
  TickvLite format (`crates/k_nano/src/storage/tickv.rs`) — a volume written on
  either side is read by the other. `TickvFile` writes 512-aligned records with
  IEEE CRC32 over key‖val; tombstone `TKL\0`/`vlen=0`; EOF all-0x00/0xFF.
  Note: no checkpoint in v0.1 (the OS mounts by full scan); GC in v0.2.
