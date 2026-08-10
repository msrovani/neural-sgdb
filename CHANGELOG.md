# Changelog — neural-sgdb

All notable changes to this project. Format based on
[Keep a Changelog](https://keepachangelog.com/), versions follow
[SemVer](https://semver.org/).

## [Unreleased]

### Planned (v0.2+)
- CRDT per-layer merge policy design (roadmap v0.2): LWW for config/state,
  multi-value register for episodic, causal merge via VectorClock
  (`docs/api.md`)
- `TickvFile` GC/compaction + TKCK checkpoint in the backend

## [0.2.0] — 2026-08-10

### Fixed
- **CRITICAL — in-place tombstone resurrection** (`scan_volume`): OS-written
  deletes (`TKL\0`, magic[3]=0) were re-inserted as live data; now skipped
  before CRC, matching OS `recover()` (bughunt #1).
- **FileStorage CRC** now covers key‖val, not just the key — value bit rot is
  detected on open (bughunt #2).
- **Recall sort** uses the raw u32 score (OS parity: FP32 0..10000 vs ham
  0..64); companion-text lookup uses `replacen("/L4/", "/L2/", 1)` (bughunt #3/#6).
- **clamp** truncates at a char boundary — no panic on mid multi-byte cut (bughunt #7).
- **`set_cpu_caps`** rearms the SIMD kernel selection latch — no_std injection
  after first use now takes effect (bughunt #9).
- **MCP `remember`** keys are ms×1000+seq (no collision in the same ms);
  `demo_embed` falls back to individual bytes for text < 3 chars (bughunt #10).
- **Bench recall@k** baseline is true FP32 cosine over the original f32
  vectors — the previous "FP32-exact" comparison was tautological (bughunt #4).
- **`Sgdb::open`** propagates rebuild errors — an unreadable storage no longer
  opens as a silent "ready" (P1); `recovered_records()` exposes the reindexed
  count.

### Changed
- `scan_volume` bounds checks run before the record-size check, matching the OS
  (a mid-volume corrupt header no longer stops the scan — bughunt #5).
- `Sgdb::open` is now strict about storage scan errors (best-effort rebuild is
  gone; recovery is observable).
- Features split: file backends behind `file-storage`, SIMD auto-detect behind
  `simd-runtime` (defaults unchanged).
- Documentation 100% English: README, `docs/api.md`, AGENTS.md, CLAUDE.md,
  codemaps, release notes; all PT-BR doc-comments translated.

## [0.1.0] — 2026-08-10

### Added
- **Portable core extraction** from neural-os-core (`k_ai::sgdb`, ADR-0063):
  `ArtIndex` (Radix Tree Node4/16/48/256 + SSE2), `MemoryDoc`/`MemoryDocView`
  (NMD1 format, byte-identical to the OS), `BqFlatIndex` (1-bit quantization +
  Hamming top-k), `hamming_dispatch` (scalar/AVX2/AVX-512, `#[target_feature]`),
  instance-based `AiosDatabaseEngine` (RAM L0/L1 + Storage L2–L7, ART/BQ
  indexing, rebuild), `Sgdb` facade (remember_exchange/_full, remember_semantic,
  recall, rag_context, remember_fact, scan_prefix, checkpoint, prune_working_ram,
  get, recovered_records).
- **`Storage` trait** (put/get/scan_prefix/delete) + `SgdbError` with three
  shipped backends: `InMemory`, `FileStorage` (CRC32 append-log, crash-safe),
  `TickvFile` (byte-exact TKLV records, OS-readable).
- **TKLV/TKCK codec** (`src/tickv.rs`): `encode_record`, `scan_volume` (OS
  `recover()` semantics: 512-aligned corrupt hunt, EOF all-0x00/0xFF, in-place
  `TKL\0` tombstone skip, last-wins), `encode_ckpt`, `fnv1a64` — byte-exact
  storage interop with neural-os-core.
- **CRDT memory sync** (feature `p2p`, opt-in): `CrdtMemorySync` (symmetric
  LWW), `Transport` trait, `UdpTransport` (std, unauthenticated demo).
- **Examples**: `bench` (ART P50/P99, BQ top-k, recall@5 BQ vs FP32 cosine,
  zero-dep) and `mcp_server` (MCP over stdio, handshake `2025-11-25`, tools
  remember/recall/rag_context, trigram demo embedding).
- **Features**: `std`, `file-storage`, `simd-runtime` (default), `p2p`
  (opt-in). Dual-mode `no_std` + `std`, zero runtime dependencies.
- **Docs**: README, `docs/api.md` (contract, seams, migration map, format
  versioning, feature matrix, CRDT policy), `AGENTS.md`, `CLAUDE.md`, codemaps,
  release notes — all English.

### Fixed
- **CRITICAL — in-place tombstone resurrection** (`scan_volume`): OS-written
  deletes (`TKL\0`, magic[3]=0) were re-inserted as live data; now skipped
  before CRC, matching OS `recover()` (bughunt #1).
- **FileStorage CRC** now covers key‖val, not just the key — value bit rot is
  detected on open (bughunt #2).
- **Recall sort** uses the raw u32 score (OS parity: FP32 0..10000 vs ham
  0..64); companion-text lookup uses `replacen("/L4/", "/L2/", 1)` (bughunt #3/#6).
- **clamp** truncates at a char boundary — no panic on mid multi-byte cut (bughunt #7).
- **`set_cpu_caps`** rearms the SIMD kernel selection latch — no_std injection
  after first use now takes effect (bughunt #9).
- **MCP `remember`** keys are ms×1000+seq (no collision in the same ms);
  `demo_embed` falls back to individual bytes for text < 3 chars (bughunt #10).
- **Bench recall@k** baseline is true FP32 cosine over the original f32
  vectors — the previous "FP32-exact" comparison was tautological (bughunt #4).
- **`Sgdb::open`** propagates rebuild errors — an unreadable storage no longer
  opens as a silent "ready" (P1); `recovered_records()` exposes the reindexed
  count.

### Changed
- `scan_volume` bounds checks run before the record-size check, matching the OS
  (a mid-volume corrupt header no longer stops the scan — bughunt #5).
- `Sgdb::open` is now strict about storage scan errors (best-effort rebuild is
  gone; recovery is observable).
- Features split: file backends behind `file-storage`, SIMD auto-detect behind
  `simd-runtime` (defaults unchanged).

## [0.0.0] — 2026-08-09

### Added
- Initial scaffold: dual license (MIT OR Apache-2.0), README, `.gitignore`,
  `docs/api.md` (design target).
