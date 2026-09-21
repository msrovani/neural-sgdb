# ADR-0012 — Adaptive recall: pay for the top-k boundary only when it is ambiguous

Status: **Implemented** (v1.1.22 — `Sgdb::recall_adaptive` /
`recall_adaptive_scoped`, `AdaptiveRecall`, `RecallProbe`)

## Context

Every recall in the crate uses a **fixed** candidate budget: `k · oversample`,
either explicit (`recall_oversampled`) or automatic by dimensionality
(`recall`: 1 word → 16×, 2–4 → 8×, else 4×). The automatic ladder exists because
the coarse BQ filter degrades with few words per vector, and the crate's own
benchmark shows the whole cost curve:

| oversample | recall@5 coarse | recall@5 dual ADC-lite |
|---|---|---|
| 1× | 22% | 24% |
| 4× | 24% | 28% |
| 16× | 35% | 40% |

The cost is linear in the oversample and the benefit is not: most queries gain
nothing from a large pool, and a few need it badly. The fixed default overpays
on the easy majority and underpays on the hard minority — and no caller can tell
which case it is in without running the recall twice and comparing.

The crate already had the measurement that makes the distinction decidable:
`SCORE_TIE_MARGIN` (u32 50 ≈ 0.005 cosine), the margin the state-first ranking
(v1.1.17) uses to decide when two memories are "the same content, different
version". A top-k boundary whose `k`-th and `(k+1)`-th hits are within that
margin was **decided by version order, not by content** — so an unexamined
candidate could legitimately hold the slot.

## Decision

### 1. Ambiguity of the boundary is the escalation signal

`Sgdb::recall_adaptive(query, k, max_oversample) -> AdaptiveRecall` walks the
ladder `1 → 4 → 8 → 16` (clamped to `max_oversample`). At each step it asks for
`k+1` hits — the **boundary sentinel** — and stops when:

```text
|score(hits[k]) - score(hits[k-1])| > SCORE_TIE_MARGIN     (decisive)
```

or when the cap is reached. The gap is compared on the **raw u32 score**
(`dist·10000`), not on the `score/(MARGIN+1)` buckets the ranking sorts by:
buckets have arbitrary edges, so two documents 0.0026 cosine apart can fall in
different buckets and look "decided". The first implementation used the bucket
and the crate's own test caught it (`adaptive_escalates_and_recovers_best_the_
small_pool_missed`).

### 2. "Few survivors" is ambiguous unless the budget was not saturated

An unfilled top-k has two very different causes, and only the pool knows which:

```text
survivors <= k && !probe.saturated()  → the store has nothing else   → decisive
survivors <= k &&  probe.saturated()  → the filter ate the pool      → escalate
```

`RecallProbe { considered, survivors, budget }` is returned (and exposed on
`AdaptiveRecall`) so the caller sees the reason instead of a boolean. Without
this distinction the scope filter — which runs **inside** the candidate loop —
would silently return a truncated top-k for a scope whose docs sit deep in the
BQ order.

### 3. Opt-in, never the default

Escalating adds candidates, and more candidates can change the **order** of the
results. Anything that changes ranking must not be the default for callers that
consume hits by position (the MCP contract, the protocols, the connectors).
`recall` keeps its fixed automatic oversample; `recall_adaptive` is a separate
API, and `max_oversample = 1` degrades exactly into the classic behaviour.

## Consequences

### What the mechanism guarantees (unit-tested)

- **Correctness of escalation** — on a corpus where the best document is
  *outside* the cheap pool (degenerate hamming cluster where the coarse filter
  tie-breaks by id), the adaptive path escalates and recovers it, while a fixed
  1× recall does not.
- **No op when decisive** — a boundary decided by content stops at 1× and
  returns exactly the same hits as the equivalent fixed-budget recall.
- **Convergence** — escalating to the cap produces the same top-k as a single
  fixed pass at that cap.
- **Determinism** — same DB + query + k + cap ⇒ identical hits, oversample,
  escalation count and verdict.
- **Scope safety** — the ladder never leaks across scopes and never returns a
  truncated top-k because the filter starved the pool.

### What the benchmark says (and why the default threshold is the weak point)

Two regimes measured in `examples/bench.rs` (same policy, same data):

| corpus | recall@5 adaptive | ladder histogram (1/4/8/16) | candidates/query | reference |
|---|---|---|---|---|
| 8 dense clusters, 1024-dim (deliberately degenerate) | 44% | 40/40/40/40 | 338.6 | single 16× ≈ 160 |
| spread (uniform) | 58% | 40/28/25/25 | 212.1 | single 1× ≈ 12 |

The honest reading: **with `SCORE_TIE_MARGIN` as the ambiguity threshold,
"ambiguous" is the common case.** On dense clusters every query escalates, so
the ladder costs ~2× a single pass at the cap for +4 pp of recall; on spread
data 62% of queries still escalate to the cap. The margin was calibrated for a
different job (deciding version-vs-content ties inside the top-k), and it is
generous relative to the distance spacing of a large corpus.

Therefore this ADR ships the mechanism, not a claim that the default pays. The
knobs that would make it pay — a tighter ambiguity threshold (exposed per call)
and a ladder that starts above 1× — are deliberately left out of the API until
a host measures its own corpus. The crate's contribution here is the
**instrument**: `AdaptiveRecall { oversample_used, escalations,
boundary_decisive, probe }` reports exactly what the policy did, so the decision
becomes data instead of faith — the same posture as ADR-0009 §4 and ADR-0011.

### Cost

No format change (no wire type, no side-table, no MDM1 version), no change to
default recall, no new dependencies, `no_std` clean. The extra work is one
internal counter pair read out of the existing `recall_impl` (`RecallProbe`),
which is why `recall_impl` is now a thin wrapper over `recall_impl_probe`.
