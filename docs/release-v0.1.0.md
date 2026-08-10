# neural-sgdb v0.1.0

**Persistent, transferable memory database for AI agents — memories, not data.**

Core extracted from [neural-os-core](https://github.com/msrovani/neural-os-core)
(`k_ai::sgdb`, ADR-0063) as an independent community project: **zero deps,
dual-mode `no_std` + `std`, MIT OR Apache-2.0**.

## What it delivers

- **8 memory layers L0–L7** (Sensory → Working → Episodic → Semantic →
  Procedural → Identity) with `MemoryDoc` (NMD1) format byte-identical to the OS
- **Semantic `remember` / `recall`** — BQ (binary quantization) + FP32 rescore,
  SIMD dispatch AVX-512 / AVX2 / scalar
- **ART index** (Adaptive Radix Tree) O(k), Node4→16→48→256
- **Pluggable storage** via `Storage` trait — `InMemory`, `FileStorage`
  (CRC32 crash-safe append-log) and **`TickvFile` (byte-exact TKLV format of
  the OS TickvLite — volume interop)**
- **CRDT memory sync** (`p2p` feature) — `CrdtMemorySync` + `Transport` +
  `UdpTransport`, symmetric LWW merge
- **Benchmarks** — ART P50/P99, BQ top-k, recall BQ vs FP32
- **MCP server** — `remember`/`recall`/`rag_context` for AI agents
  (Claude Code, Cursor, OpenCode) via MCP over stdio

## Verification

- `cargo test` — 30+1 tests (default), 34+1 with `p2p`
- `cargo check --no-default-features --target x86_64-unknown-none` — clean
- Interop: NMD1 and TKLV/TKCK verified by golden test + re-parse
  (`scan_volume`, OS `recover()` semantics)

## Roadmap

6/6 complete. Next (v0.2): `TickvFile` GC/compaction, TKCK checkpoint in the
backend, GitHub Actions CI.
