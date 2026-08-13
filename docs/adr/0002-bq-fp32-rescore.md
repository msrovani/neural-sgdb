# ADR-0002 — BQ + FP32 rescore instead of FAISS/HNSW

Status: Accepted (retrospective — core extraction, 2026)

## Context

The OS needs semantic recall on bare-metal: no heap-heavy index, no
external library. FAISS/HNSW are std-only, allocate heavily and are
dependency poison for a `no_std` crate (ADR-0001). The memory DB is a
cognitive substrate, not a general vector engine — recall volumes are
agent-scale (hundreds to low-hundreds of thousands of docs), not web-scale.

## Decision

- **Coarse stage**: binary quantization (sign-BQ flat bitvecs) + hamming via
  SIMD dispatch (scalar / AVX2 / AVX-512), bounded top-k heap, auto-oversample
  by dimensionality, optional `MihIndex` (multi-index hashing) for sub-linear
  candidates.
- **Fine stage**: FP32 cosine rescore over the oversampled candidate pool —
  ranking is always done on the original floats, never on the quantized bits
  (the benchmark baseline is true FP32 cosine; anything else is tautological).
- Indexes are DERIVED state: rebuilt from storage on open (`rebuild_indices`).
- `recall_weighted` adds recency/importance signals; `recall_lexical`/
  `recall_hybrid` (BM25) covers the lexical dual-path.

## Consequences

- Positive: zero deps, `no_std`-clean, deterministic and testable, honest
  benchmark numbers (`BENCHMARKS.md`).
- Negative: recall is brute-force-in-BQ (or MIH) — no ANN asymptotic
  guarantees beyond that; a *real* million-scale embedding workload would
  outgrow it.
- Contract impact: API (`recall`, `recall_oversampled`, `recall_weighted`,
  `recall_lexical`, `recall_hybrid`) is stable. Revisiting this decision
  requires a written ADR + benchmark evidence.