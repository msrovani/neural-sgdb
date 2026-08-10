# neural-sgdb v0.2.0

**Persistent, transferable memory database for AI agents — memories, not data.**

Core extracted from [neural-os-core](https://github.com/msrovani/neural-os-core)
(`k_ai::sgdb`, ADR-0063) as an independent community project: **zero deps,
dual-mode `no_std` + `std`, MIT OR Apache-2.0**.

## What's new in v0.2.0

- **10 bug fixes from the bughunt** (oracle review), including:
  - **CRITICAL** — in-place tombstone (`TKL\0`) resurrection in `scan_volume`:
    OS-written deletes no longer come back as live data
  - `FileStorage` CRC now covers key‖val (value bit rot detected)
  - recall sorts by raw u32 score (OS parity); companion-text lookup via
    `replacen` (keys containing `/L4/` no longer corrupt)
  - `clamp` truncates at char boundaries (no multi-byte panic)
  - `set_cpu_caps` rearms SIMD kernel selection (no_std injection works)
  - MCP `remember` keys are collision-free; demo embedding handles short text
  - bench recall@k baseline is true FP32 cosine (was tautological)
- **P0–P3 review applied:** `Sgdb::open` propagates rebuild errors +
  `recovered_records()` (recovery observable); feature split
  (`std`/`file-storage`/`simd-runtime`/`p2p`); format versioning with golden
  byte tests; CRDT per-layer merge policy documented (roadmap v0.2)
- **Documentation 100% English** (README, api.md, AGENTS.md, CLAUDE.md,
  codemaps, release notes) + CHANGELOG.md

## What it delivers (v0.1 base)

- **8 memory layers L0–L7** with `MemoryDoc` (NMD1) byte-identical to the OS
- **Semantic `remember` / `recall`** — BQ + FP32 rescore, SIMD AVX-512/AVX2/scalar
- **ART index** O(k), Node4→16→48→256
- **Pluggable storage** — `InMemory`, `FileStorage`, `TickvFile` (byte-exact
  TKLV/TKCK interop with the OS)
- **CRDT memory sync** (`p2p` feature) — symmetric LWW
- **MCP server** for AI agents (Claude Code, Cursor, OpenCode)
- **Benchmarks** — ART P50/P99, BQ top-k, recall BQ vs FP32

## Verification

- `cargo test` — 31+1 tests (default), 35+1 with `p2p`
- `cargo check --no-default-features --target x86_64-unknown-none` — clean
- Interop: NMD1 + TKLV/TKCK golden byte tests + `scan_volume` re-parse
