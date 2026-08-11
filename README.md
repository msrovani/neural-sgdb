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

**v0.5** ✅ — dual-mode (`no_std` + `std`, zero dependencies):

- `cargo test` on host: **92 tests + doc-test** (102 + doc-test with `p2p`)
- `cargo check --no-default-features --target x86_64-unknown-none`: **clean**
- **Recall stack**: BQ coarse → FP32 rescore, SIMD hamming (AVX-512/AVX2/
  scalar), auto-oversample by dimensionality, `recall_oversampled`,
  `recall_weighted` (recency·importance·semantic), `recall_lexical` +
  `recall_hybrid` (BM25 dual-path), `MihIndex` (multi-index hashing,
  sub-linear candidates)
- **Storage**: `Storage` trait + `InMemory` + `FileStorage` (CRC32 append-log,
  persistent lazy handle ~38x, durability levels, atomic compaction) +
  `TickvFile` (byte-exact TKLV **with TKCK checkpoint fast-mount + GC/compact**
  + in-place `TKL\0` invalidation)
- **Memory semantics**: 8 layers L0–L7, `MemoryState` lifecycle, temporal
  validity window (`sys/validity/` — invalidate-not-delete), CRDT sync with
  conflict preservation + delta sending (`p2p`), vector clock
- **Interfaces**: MCP server with `memory://{layer}/{key}` resources +
  `nextCursor` pagination + tool annotations; `cargo run --release
  --example stress` (100k-op stress) and `--example bench`
- Full API contract in [`docs/api.md`](docs/api.md)

The reference implementation runs on bare-metal in the parent OS
(`k_ai::sgdb`, AGPL); this repo evolves separately (MIT OR Apache-2.0).

## Quick start

```rust
use neural_sgdb::{Sgdb, FileStorage};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = Sgdb::open(FileStorage::open("agent_memory.db")?)?;

    // L1 + L2 (RAM; persiste com checkpoint)
    db.remember_exchange("how's the weather?", "sunny, 24 degrees")?;
    db.checkpoint()?;

    // L4 semântico — embeddings fornecidos pelo caller
    let emb = vec![1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
    db.remember_semantic("turn:1", "sunny weather in sao paulo", &emb)?;

    // recall: BQ + FP32 rescore, auto-oversample; variantes ponderada/híbrida
    let hits = db.recall(&emb, 5)?;
    let recent = db.recall_weighted(&emb, 3, 1.0, 1.0, 0.5, 1000)?;
    let lex = db.recall_lexical("sunny weather", 3)?;

    // L3 fato temporal + janela de validade (invalidar-não-deletar)
    db.remember_fact("user prefers dark mode", 42)?;
    db.set_validity("md/L3/ts/000000000000002a", 0, 1000)?;

    let ctx = db.rag_context(&emb, 3)?;
    println!("{ctx}");
    Ok(())
}
```

More: `cargo run --release --example bench` (benchmarks), `--example stress`
(100k-op stress), `--example mcp_server` (MCP), and **telepathy** —
`cargo run --release --example p2p_telepathy --features p2p` exchanges
memories between two `Sgdb` instances via CRDT version sync + doc pull
(two instances converge with no central server). The crate doc
(`cargo doc --open`) is a runnable tour of the whole API.

**How the memory sync really works** — [docs/telepathy.md](docs/telepathy.md)
(EN) / [docs/telepathy-pt.md](docs/telepathy-pt.md) (PT-BR): the CRDT model,
the two-instance convergence flow, the honest cost (eventual consistency, no
global order, conflict preservation) and how an AI at the root of the process
arbitrates the preserved conflicts.

## MCP (AI agents)

`cargo run --release --example mcp_server` exposes `remember` / `recall` /
`rag_context` as MCP tools (JSON-RPC 2.0 over stdio, `2025-11-25` handshake),
memories as **resources** (`memory://{layer}/{key}`), recall with opaque
`nextCursor` pagination, and tool annotations (`readOnlyHint`/
`destructiveHint`/`idempotentHint`) — connectable to Claude Code, Cursor and
OpenCode:

```bash
# Claude Code
claude mcp add neural-sgdb -- cargo run --release --example mcp_server
```

⚠️ The MCP recall embedding is a **demo** (character-trigram hash); for real
semantic recall, provide your own embeddings via `remember_semantic` / `recall`.

## Benchmarks

`cargo run --release --example bench` — local environment numbers (AVX2):
ART insert P50≈800ns / get P50≈200ns, BQ top-5 ≈160–175µs over 10k×1024 dims,
CRC32 ≈2.6ms/1MiB, TickvFile fast-mount ≈2–3x under churn, recall@5 BQ coarse
22%→35% as oversample 1×→16× (sign-BQ separates the cluster, not the exact
member — the FP32 rescore re-ranks the candidates).

## Docs

- **API contract** — [`docs/api.md`](docs/api.md) (Storage trait, seams,
  migration map, format versioning, feature matrix, CRDT policy)
- **Architecture v0.2 (design)** — [`docs/architecture/`](docs/architecture/)
  — Memory Model (01), Lifecycle (02), Retrieval (03), Distributed (04),
  Storage (05), Cognitive API (06) — formalizes the *cognitive memory system*
  step on top of the current *memory substrate*
- **AI agent guides** — `AGENTS.md`, `CLAUDE.md`, `codemap.md` (atlas)

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
