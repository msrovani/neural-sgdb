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

## RRF hybrid fused by memory (1.5.10) — the fix is right, the number got WORSE

- **What changed:** the hybrid fused streams by *storage key*, so a semantic
  memory (`md/L4/<id>`) and its text companion (`md/L2/<id>`, the one the
  lexical index holds) were counted as two different hits: the same memory
  occupied two of the top-k slots and its two votes never met. 1.5.10 fused by
  **memory identity** (`Hit.rel ?? Hit.key`).
- **Artifact:** `examples/longmemeval.rs --mode hybrid` (new mode; the
  `component`/`pipeline` modes are lexical and never touch the RRF, so the
  published benchmark could not measure this at all before).
  LongMemEval-S, 500 questions, k=5, same dataset/sha256 — only the fusion code
  differs.
- **Result:** hit@5 **0.932 → 0.840**, recall@5 0.847 → 0.705,
  recall-all@5 0.752 → 0.566.
- **Why (from `--dump`):** before, the top-5 mixed `md/L2` and `md/L4` of the
  same session — the right evidence appeared because duplicates were *double
  counted*. After the fix the ranking is coherent, and with the `demo` embedder
  (256-dim trigram, noisy) **two mediocre votes outrank one strong lexical
  vote**, pushing the evidence out of the top-5.
- **Decision:** keep the fix (returning the same datum twice and never summing
  its votes is a bug regardless of the score), and publish the honest number.
  **No weight or `k_rrf` tuning** was done to recover the metric — that would be
  fitting the benchmark with the benchmark itself.
- **Follow-up (1.5.12) — measured with a REAL embedder:** the same harness,
  same dataset/sha256, ran `hybrid` with `all-MiniLM-L6-v2` (384d, local HTTP
  endpoint via `scripts/embed_server.py`). hit@5 **0.840 -> 0.974** (lexical
  baseline 0.950), recall@5 0.705 -> 0.928, recall-all 0.566 -> 0.860. So the
  fusion DOES help when the semantic signal is real: the 1.5.10 verdict was
  about the `demo` embedder, not the fusion. No weights or `k_rrf` were tuned
  (`k_rrf=60` unchanged). One slice regressed (single-session-user 0.971 ->
  0.957). Artifacts: `evals/runs/1.5.12-hybrid-real.json` and
  `1.5.12-lexical-pipeline.json`; see `BENCHMARKS.md`.
- **Lesson:** a correctness fix can lower a metric whose previous value came
  from the bug. Measure the *mechanism* too (`--dump` showed duplicates), not
  only the aggregate — and never quote a fused-retrieval number whose harness
  never exercises the fusion.
