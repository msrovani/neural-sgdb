# BENCHMARKS

Measured, reproducible numbers for `neural-sgdb`. **This file is the honest
source of truth for performance claims** — README/docs must not assert numbers
that aren't reproduced here.

## How to reproduce

```bash
cargo run --release --example bench
cargo run --release --example era_migration_bench   # ADR-0007 era migration
```

- All inputs are **deterministic** (LCG-seeded pseudo-random; same run on the
  same machine → same data). No wall-clock variance from input generation.
- The bench is a **smoke-grade benchmark** (single binary, no multi-sample
  statistics beyond P50/P99 on per-op timings). It answers "is this fast
  enough?" — not "what is the asymptotic constant?". For rigorous numbers use
  a real profiler (`perf` / VTune) on your workload.
- `recall@k` measures the BQ coarse filter vs the **true FP32 cosine** over the
  original f32 vectors (never hamming vs the same quantized bits — that would
  be tautological). Data is **correlated clusters** (center + small noise), not
  pure noise (pure noise measures a meaningless 0% — sign-BQ cannot separate
  uniform noise).
- Numbers are **wall-clock**, single-threaded, `--release` (opt-level 3),
  with the SIMD kernel auto-selected (`avx2_xor` on AVX2 hosts).

## Environment (measured 2026-08-13)

- CPU: AMD Ryzen 7 PRO 5750G (8C/16T, 3.8 GHz max), AVX2 (no AVX-512)
- OS: Windows 11 Pro (build 26200), PowerShell host
- Rust: nightly-2026-07-05 (`rustc 1.98.0-nightly`), `x86_64-pc-windows-gnu`
- SIMD kernel: `avx2_xor`

**Numbers are tied to this environment.** Expect different (usually faster)
results on newer silicon, and slower on laptops/older CPUs. Do not report a
µs number from one machine as if it were universal.

## Results (3 runs, same binary, no code change between runs)

### ART — 100k fixed-width keys

| metric | run 1 | run 2 | run 3 |
|---|---|---|---|
| insert P50 | 600 ns | 600 ns | 600 ns |
| insert P99 | 14.5 µs | 12.7 µs | 8.0 µs |
| get P50 | 200 ns | 200 ns | 200 ns |
| get P99 | 800 ns | 1.1 µs | 800 ns |

P99 insert is dominated by node growth/reallocation (Node4→16→48→256 and
path splits) on the fixed-width key pattern; P50 is the steady-state cost.

### BQ top-k — 10k vectors × 1024 dims, 100 queries

| metric | run 1 | run 2 | run 3 |
|---|---|---|---|
| top-5 avg/query | 97.1 µs | 102.8 µs | 96.0 µs |
| top-k heap (k=5) | 102.4 µs | 101.7 µs | 97.7 µs |
| top-k full-sort (k=N) | 282.4 µs | 295.1 µs | 281.4 µs |

The bounded max-heap (k=5) is ~2.8–2.9× cheaper than full-sort (k=N) on the
same index. Hamming is SIMD (AVX2 `_mm_xor_si128` + popcount).

### recall@5 — BQ coarse vs true FP32 cosine (correlated clusters)

| oversample | recall@5 |
|---|---|
| 1× | 22% |
| 2× | 22% |
| 4× | 24% |
| 8× | 30% |
| 16× | 35% |

Honest framing (see `src/art.rs`/`examples/bench.rs` comments): sign-BQ
separates the **cluster**, not the exact member — dense clusters produce
hamming ties, so the id tie-break decides. Oversampling raises the candidate
pool so the FP32 rescore can re-rank more true neighbors. This is the coarse-
filter→rescore design working as intended: **BQ is a filter, not the ranking.**
For exact k-NN use the FP32 path (full-sort column above) or raise oversample.

### CRC32 (storage integrity) — 1 MiB × 100

| run 1 | run 2 | run 3 |
|---|---|---|
| 1.94 ms avg (515 MiB/s) | 1.94 ms (515 MiB/s) | 1.94 ms (515 MiB/s) |

Table-driven (256-entry const fn), ~1 op/byte.

### TickvFile open — churn: 35k records, 5k live

| run 1 | run 2 | run 3 |
|---|---|---|
| fast-mount(ckpt) 17.6 ms vs full-scan 21.0 ms (**1.2×**) | 14.8 ms vs 16.6 ms (**1.1×**) | 16.5 ms vs 20.5 ms (**1.2×**) |

**Correction to earlier claims**: historical docs claimed fast-mount is
"~2–3× faster under churn". On this environment the checkpoint fast-mount
measured only **~1.1–1.2×** over full-scan. Fast-mount pays off because it
skips re-parsing tombstones (15k of 35k here), but the CRC+parse cost of the
live set dominates. It also wins only under churn — on an all-live volume the
two paths are parity (both must read every record). Any future claim must be
re-measured here.

### End-to-end Sgdb — 1k exchanges (InMemory)

| run 1 | run 2 | run 3 |
|---|---|---|
| 106.0 ms total | 101.8 ms | 105.1 ms |

Includes L1+L2 doc creation, ART insert, and (in release) no checkpoint
(InMemory). Not a microbenchmark — a sanity end-to-end number.

### Era migration (ADR-0007) — N=2000 docs, FileStorage, 3 runs

Methodology: build an era-OLD corpus (DemoEmbedder 256-dim), migrate to
era-NEW (EraEmbedder 384-dim, simulated model) by re-embedding the preserved
L2 text and rewriting payload+bitvec, then `rebuild_indices()` (BQ width
reset). The re-embed cost measured here is the **crate-side floor** (trait
invocation + allocation); a real model's inference/network cost is external
(`Embedder` seam, `examples/embedder_http.rs`). All correctness invariants
asserted: width-lock trap reproduced, `memory_id` stable across overwrite,
40/40 resurrected recalls (dist ≈ 0), era-OLD queries fail **loudly** (S1),
lexical still recovers old-era text, `validate()` clean.

| phase | run 1 | run 2 | run 3 |
|---|---|---|---|
| write era OLD total (2000 docs) | 82.9 ms | 86.0 ms | 86.3 ms |
| write per doc | 41.5 µs | 43.0 µs | 43.2 µs |
| scan `md/L4/` (2000 keys) | 960.7 µs | 659.7 µs | 716.2 µs |
| read L2 companions | 1.22 µs/doc | 1.11 µs/doc | 1.07 µs/doc |
| re-embed (sim, crate-side) P50 | 900 ns | 1 µs | 900 ns |
| rewrite payload+bitvec P50 | 71.6 µs | 73.0 µs | 73.2 µs |
| rewrite payload+bitvec P99 | 145.1 µs | 195.2 µs | 173.8 µs |
| `rebuild_indices()` (4000 docs) | 51.7 ms | 52.3 ms | 49.2 ms |

Migration cost is dominated by the **payload rewrite** (~72 µs/doc: FileStorage
append + meta + index) and the **index rebuild** (~50 ms flat); scanning and
text reads are negligible. Wall-clock scaling ≈ `O(N · rewrite) + O(rebuild)`.
The append-only FileStorage keeps the old-era blobs until `compact()`.

## What is deliberately NOT benchmarked

- **Network/CRDT sync latency** — transport is a demo (`UdpTransport`,
  unauthenticated). Latency numbers would be meaningless without a real
  deployment target.
- **MCP server throughput** — stdio JSON-RPC, demo embedding (trigram).
- **Power-loss / crash durability** — not measurable by a bench; covered by
  the recovery tests and sweep tests, not by timing.
- **Microbenchmarks of individual memory operations** (`remember_semantic`,
  `recall_weighted`) — the earlier "µs/put" figures in the changelog were
  measured on a different toolchain/OS and are not reproduced here; treat
  them as historical only.