# Changelog — neural-sgdb

All notable changes to this project. Format based on
[Keep a Changelog](https://keepachangelog.com/), versions follow
[SemVer](https://semver.org/).

## [Unreleased]

## [0.4.0] — 2026-08-10

### Fixed (bughunt #11)
- **FileStorage oversized write = silent tail data loss**: `put` with a value
  > `MAX_VLEN` (or key > `MAX_KLEN`) was accepted, but `open()` recovery
  rejects it and **truncates the file** — every record written after it was
  silently destroyed. `append` now bounds-checks before writing (parity with
  `TickvFile`); oversized puts fail with `Err`.
- **`put(k, &[])` inconsistent with reopen** (both `FileStorage` and
  `TickvFile`): empty value writes a tombstone on disk (vlen `u32::MAX` /
  `0`) but kept `k → []` in the in-memory map — `get(k)` returned `Some([])`
  in-session and `None` after reopen. Empty value now behaves as delete at
  both read points.
- **`BqFlatIndex::insert_1024` broke the flat-index invariant**: it appended
  16 words unconditionally even when `words_per_vec` was already another
  value — `top_k` then read out-of-bounds (panic) or returned wrong results
  when mixed with narrower `insert_f32`/`insert`. It now truncates/pads to
  the established width (like `insert`).
- **`scan_volume` hid torn tails**: a partial header of 1..15 bytes at EOF
  was silently ignored (`truncated = false`), unlike `FileStorage` recovery.
  A clean pre-zeroed EOF region is still treated as clean EOF (not truncation).
- **`scan_volume` indexed the checkpoint record as memory**: `sys/tickv_ckpt`
  was surfaced as a live key (`vlen != 0`) in the backend map. Now skipped
  (parity with the OS `recover()`); exposed when `TickvFile` began writing
  checkpoints.
- **`encode_ckpt` count mismatch**: entries skipped in the body (key > 65535
  or `sys/tickv_ckpt`) still inflated the `n` field — an OS decoder reading
  `n` entries would desync. `n` now counts only the entries actually written
  (hash already covered only those).
- **`Hit.dist` scale contract**: the hamming fallback in `recall` returned a
  raw distance in `0..64` while `Hit.dist` documents `1−cos` on `0..1`; it is
  now normalized by `words_per_vec × 64`.

### Added
- **`Sgdb::recall_oversampled(query, k, oversample)`** (upstream BQ/Qdrant): the
  coarse Hamming filter now fetches `oversample·k` candidates before the FP32
  rescore. With low-dim embeddings the BQ filter collides on bits and the exact
  match escapes a small top-k (stress measured exact@1 ≈ 42% at 100k × 16-dim);
  raising the oversample recovers it **without any format change**. `recall()`
  delegates with oversample=4 (unchanged behavior); `rag_context_oversampled`
  added. Test: exact match recovers at 64× on low dims.
- **`TickvFile::checkpoint()` + fast-mount TKCK** (OS TickvLite parity, roadmap
  v0.1 gap): `checkpoint()` writes the `sys/tickv_ckpt` record (TKCK,
  byte-identical to the OS) as the LAST record; `open()` now tries
  `try_mount_from_ckpt` (header-only scan, FNV-1a index verification, per-entry
  CRC + `TKL V` stale check, ckpt-must-be-last guard) and falls back to the
  full `scan_volume` on any anomaly (torn/stale ckpt, post-ckpt appends).
  `ScanResult` gains `offsets` + `append_off`. Bench (churn, 35k recs / 5k
  live): fast-mount 14.8ms vs full-scan 43.2ms (**~2.9x**); in an all-live
  volume both read the same bytes so it's parity — the win is not re-processing
  tombstones/dead records. Torn ckpt degrades to the previous-mount semantics.
- **`TickvFile::compact()`** (GC, roadmap v0.2, OS `maybe_gc` parity): rewrites
  the live set as fresh TKLV records + a final TKCK checkpoint, atomic rename.
  Removes tombstones/obsolete versions and leaves the volume fast-mountable.
- **ART shrink on delete** (Leis paper / artful parity): `delete` now removes
  leaves and shrinks nodes 256→48→16→4 when `n` drops below threshold instead
  of leaving dead leaves — memory is reclaimed under churn. `delete_rec` now
  returns `Option<Box<Node>>` (None = empty subtree); the `dead` leaf tombstone
  is gone. Tests: 200-key→1-key shrink, 100k-op churn, re-insert after empty.
- **Fault-injection for fast-mount**: deterministic fuzz (in-memory) truncating
  at every offset + corrupting every byte of a valid TKCK volume → never
  panics and falls back to full scan; plus file-level torn/corrupt ckpt tests.
- **CI (GitHub Actions)**: test (default + `p2p`), `no_std` gate
  (`x86_64-unknown-none`), examples build, stress and bench smoke on every push.
- **Honest BQ benchmark**: bench recall@5 now uses correlated cluster data
  (not pure noise, which measured a meaningless 0%) and reports the oversample
  curve — 22% (1×) → 35% (16×) on dense 1024-dim clusters, documenting that
  sign-BQ separates the cluster but not the exact member (FP32 rescore then
  re-ranks the candidates).

### Performance
- **Hamming dispatch hot path**: `ensure_selected()` used a `SELECTED.swap(true)`
  (locked RMW) on **every** `hamming()` call — it dominated short-vector scans.
  Now `load`+`store` (benign double-select race, `set_cpu_caps` still rearms).
  Measured: BQ top-5 (10k vec × 1024 dim) 213µs → **160µs** (~25%).
- **CRC32 table (256)**: `const fn` table, zero-dep, no_std-safe, same bytes
  (golden tests pin). 1 op/byte instead of 8. Plus `crc32_parts` computes
  CRC over key‖val without concatenating (1 fewer allocation per record in
  FileStorage append/recovery/compact). 1MiB bench is serial/bandwidth-bound
  on this host (no change there); the win is per-record write/recovery.
- **FileStorage append with a persistent lazy handle**: `put`/`delete` no
  longer open+close the file on every write (one `CreateFile`+`CloseHandle`
  syscall pair per op). Measured (stress 100k, release): `Storage::put` raw
  185µs → **4.8µs** (~38x), `remember_semantic` 422µs → **22µs** (~19x),
  `remember_exchange` 201µs → 8.4µs (~24x). The handle opens on first append
  and is closed before `compact()`'s atomic rename (reopened lazily) — writes
  after compaction always target the new file. `open()` stays O(file) without
  extra syscalls (no regression on open/close stress).

### Planned (v0.3+)
- CRDT per-layer merge policy (roadmap): LWW for config/state, multi-value
  register for episodic, causal merge via VectorClock (`docs/api.md`)
- `TickvFile` GC/compaction + TKCK checkpoint in the backend
- L6 Associative/Metacognitive + Memory Graph (Doc 01/03)

## [0.3.0] — 2026-08-10

### Added (maturation sprint)
- **VectorClock semantics**: semantic `PartialEq` (map node→counter, order-
  independent), `happens_before` (causal), `concurrent` (excludes equality),
  `merge` (element-wise max + saturation), `counter_of`; 8 tests
- **CRDT conflict preservation**: `MergeVerdict` (SelfPacket/Stale/Duplicate/
  Applied/Conflict), `conflicts` (concurrent versions never LWW-discarded),
  `own_writes` (concurrency base — peer causal successor converges), self-packet
  ignored; `MemoryVersion`/`MemoryDelta`/`MemorySnapshot` abstractions; tests
- **Deterministic retrieval**: recall dedupes by storage key (best score) +
  tie-break by key — same DB+query+k ⇒ same ordered results; tests
- **Bounded BQ top-k**: max-heap (O(N·D/64 + N log k)) instead of full sort;
  k==0/k>=len/empty handled; deterministic (dist,id); bench heap(k=5)=320µs vs
  full-sort(k=N)=592µs; 6 tests
- **Durability semantics**: `Durability` (Buffered/Flushed/Durable),
  `Storage::durability()` + `sync_durable()` (fsync real no FileStorage,
  read+write handle p/ Windows); InMemory = Buffered; test
- **FileStorage compaction**: `compact()` — live set rewritten to temp +
  atomic rename; removes tombstones/obsolete; crash-safe; empty value = TOMBSTONE
  (aligned com append); 3 tests
- **Index rebuild público**: `Sgdb::rebuild_indices()` + `open_with_node_id`/
  `node_id()`; teste write→close→reopen→rebuild→recall
- **MemoryState model**: Active/Superseded/Archived/Invalidated — NÃO serializado
  no NMD1 (contrato byte-idêntico com o OS intacto); side-table `sys/state/`
  via Storage cru; `Sgdb::get_state/set_state/supersede`; `MemoryLayer::from_u8`
  = ponto único de validação; 2 testes
- **Adversarial tests**: fuzz determinístico LCG para MemoryDoc decode/view,
  TKLV scan_volume, CRDT apply_remote_version — nunca panics em malformed input

### Fixed
- **Baseline**: `cargo test --no-default-features` quebrava (30 erros) —
  imports alloc nos testes, gates `#[cfg(feature="file-storage")]`, exemplo
  mcp_server com backend por feature
- **FileStorage recovery**: bounds sanitizados (klen≤4KiB/vlen≤1MiB, checked_add),
  le32() sem unwrap, truncação determinística da cauda; CRÍTICO tombstone
  (vlen=u32::MAX) era tratado como length absurdo e chave deletada RESSUSCITAVA;
  HIGH tombstone truncado panicava (slice sem bounds); tombstone agora com CRC
- **CRDT**: has_other_state usava local_version (adotado de peers) — sucessor
  causal do mesmo peer virava Conflict para sempre; fix own_writes
- **Parsing safety**: rd_u32/rd_u64/le32 checados (sem `try_into().unwrap()`)
  em MemoryDoc decode/view, tickv scan, CRDT recv
- **compact**: valor vazio gravava vlen=0 vs append TOMBSTONE — mesma chave
  mudaria de significado pós-compactação
- **set_state(Active)**: deletava sys/state/ incondicionalmente (log crescia);
  só deleta se existir
- **recall**: overwrite em L4 re-inseria no BQ — mesma memória voltava 2x;
  dedupe por storage key + tie-break determinístico

### Changed
- Top-k BQ: full sort → bounded heap (resultado idêntico, mais rápido)
- `Sgdb::open` propaga erro de rebuild (P1); `recovered_records()` expõe contagem
- Bench honesto: baseline recall@5 agora é cosseno FP32 real (dados sintéticos
  pseudo-aleatórios → 0%, documentando o trade-off do sign-BQ em ruído)

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
