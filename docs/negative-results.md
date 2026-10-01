# Negative results — what we measured and REJECTED

Honest record of ideas implemented, benchmarked, and **not** adopted. A
negative result is an asset: it stops the next agent from re-deriving it, and
it is the main reason to trust the positive numbers. Each entry names the
artifact that reproduces it.

## RaBitQ (1-bit randomized quantization) — REJECTED

- **Benchmark:** `examples/bench_rabitq_ab.rs` (A/B, same harness).
- **Result:** on dense correlated clusters, RaBitQ was **−13 .. −17 pp**
  recall@5 versus the shipped **ADC-lite dual-path** at *every* oversample,
  and **~25–45× more expensive** (~1.3 ms vs 27–60 µs/query).
- **Decision:** ADC-lite (`quantize_f32_minus_mean` + `bq_top_k_f32_dual`) is
  the official path. `src/rabitq.rs` is kept only as a reproducible record.
- **Lesson:** a fancier quantizer loses to a query-side re-expression when the
  stored bitvecs are already fixed by the era contract (ADR-0007).

## BQ candidate delta (incremental top-k) — REJECTED

- **Where:** session note (post-v1.2.1 batch). Maintaining a delta of the BQ
  flat did not beat a full Hamming scan with the bounded heap at the corpus
  sizes measured.
- **Lesson:** measure before optimizing; the real win that session was
  `delete` being O(N) (fixed to O(1), ~6×).

## Post-RRF scoring (frequency + decay / cross-encoder) — no gain

- **Source:** vstash-style experiment (see `docs/memory-landscape.md`
  references): post-RRF frequency+decay and off-the-shelf cross-encoder
  reranking **degraded** NDCG on the tested corpora while adding latency.
- **Where in nsgdb:** `rag_context_reranked` uses lexical **anchoring** (cheap,
  deterministic) rather than a learned cross-encoder, which the core cannot
  host (ADR-0001). The `Reranker` seam (v1.1.14) lets a host inject one — we
  do not ship one.

## BQ coarse on pure-noise vectors — meaningless 0%

- **Not a bug, a methodology trap:** sign-BQ separates the **cluster**, not the
  exact member; on uniform noise it measures nothing. The honest bench uses
  correlated clusters (`examples/bench.rs`). Recorded so nobody "fixes" the
  benchmark back to noise and reports 0% as a regression.

## Frozen / parked (not rejected, just not scheduled)

- **Full HNSW / IVF-PQ** beyond the shipped `ann.rs` (IVF-Flat + HNSW-lite):
  benchmark-driven, gated on larger corpora.
- **Overlay mesh routing / production crypto transport:** seam only (ADR-0006).
- **Relation inference / multi-hop graph:** deliberate non-goal (ADR + roadmap).
