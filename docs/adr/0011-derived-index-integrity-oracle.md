# ADR-0011 — Derived-index integrity oracle (`index_fingerprint`)

Status: **Implemented** (v1.1.21 — `src/fingerprint.rs`, `Sgdb::index_fingerprint`,
`health(view=index)`, `Sgdb::validate` §5)

## Context

The derived indexes (ART, BQ, lexical, `entity_index`, `indexed_dims`,
`corpus_sums`) are rebuilt from storage on every `Sgdb::open`. Until v1.1.21 there
was **no way to ask whether an in-RAM derived index equals what a rebuild would
produce** — the only proof available was behavioural (call a bunch of queries and
compare results), which is neither complete nor cheap.

Two concrete blockers followed from that:

1. **ADR-0009 §3 (persisted index snapshot) was unfalsifiable.** A fast-mount must
   `validate → else full rebuild`. "Validate" had no definition: there was no
   primitive stating "this mounted index is equivalent to the rebuild". The ADR
   was forced to leave the wire format TBD *because the integrity check did not
   exist yet*.
2. **Imperatively maintained derived state was unverifiable.** `corpus_mean` is
   accumulated incrementally (insert adds, delete/overwrite subtracts, rebuild
   recomputes) at three sites. Correct-looking bookkeeping is not proof; a missed
   subtract would degrade ADC-lite recall silently and forever.

Separately, the ADC-lite dual-path (v1.1.17) made a float question unavoidable:
`corpus_sums` accumulates `f32` values into `f64`. **Float addition is not
associative**, so recomputing the same sum from storage in a different order may
legitimately differ in the last bits.

## Decision

### 1. A canonical fingerprint of the DERIVED state

`Sgdb::index_fingerprint() -> u64` (FNV-1a over components in canonical order).
The central invariant:

```text
fp(open) == fp(rebuild_indices())
```

If two stores that are byte-identical produce different fingerprints, the derived
index carries state that does not come from storage — exactly what a snapshot must
detect before being mounted.

### 2. What is covered, and what is deliberately EXCLUDED

Covered: ART keys (sorted), BQ `words_per_vec` + live vectors keyed by resolved
storage key, `indexed_dims`, lexical postings, `entity_index`, `corpus_sums`
counts.

Excluded, each for a reason that is part of the contract:

- **ART/BQ ids.** `NEXT_ID` is a **process-global** counter (`engine.rs`): the same
  corpus opened twice yields *different* ids. Hashing ids would make the
  fingerprint unstable across processes, i.e. useless for a persisted snapshot.
  The fingerprint resolves `id → storage_key` (via `id_to_sk`) and hashes the
  **key**. This is the finding that shaped the design.
- **BQ orphans.** The flat index is append-only; `delete` leaves inert entries
  until reclamation. Hashing `bq.len()` (the first implementation) counted them
  and broke both deletion symmetry and reclamation stability — caught by
  `index_fingerprint_ignores_bq_orphans`. Only vectors that resolve to a live
  storage key enter the hash, so the fingerprint measures the index the recall
  actually sees.
- **Floats (`corpus_mean` sums).** Non-associativity (above). Counts are exact;
  sums are compared with a **relative tolerance** in `validate`.

### 3. Cost is opt-in, never on the default `health`

The hash is `O(n log n)` (ART keys are sorted before entering — its traversal
order is not lexicographic). `HealthReport` therefore does **not** carry it;
`Sgdb::index_fingerprint()` is explicit and `health(view=index)` is an opt-in view.

### 4. `validate` verifies the float invariant with a tolerance

`Engine::corpus_sums_drift()` recomputes the sums from storage **reusing the same
accumulation function** (`corpus_add`) as the incremental path — sharing the
function is what makes the comparison meaningful. Counts must match **exactly**
(a count mismatch is a hard bug: some delete/overwrite path failed to subtract);
sums are accepted within `1e-9` relative (with an absolute floor of `1.0` for
total cancellation). Without that tolerance the check would produce **false
positives in production**, since the rebuild accumulates in a different order.

Same discipline for reading payloads: `index_doc`'s "no bitvec" branch and the
recompute share `payload_floats_truncated`, so the two sides cannot drift apart by
construction.

## Consequences

- **Positive.** ADR-0009 §3 gains its integrity primitive ("fingerprint matches"
  *is* the snapshot validator). Rebuild-invariance, deletion symmetry, insertion
  order independence, and replicated-store equivalence became assertions instead
  of beliefs. `validate()` stopped being only about side-tables. The flaky-test
  hazard of the float invariant was identified and designed away *before* it could
  bite.
- **Negative / cost.** `O(n log n)` per call; a single 64-bit hash is a summary,
  not a diff (it says *that* something diverged, not *what*). Adding a component
  to the hash is a silent contract change if done carelessly — the tests
  (`covers_derived_indexes_only`, `is_stable_across_reopen`) are the guard.
- **Contract impact.** Additive public API + one opt-in MCP view; `MCP_CONTRACT_VERSION`
  → **1.1.21**. **No format change** (NMD1/TKLV untouched; nothing is persisted —
  the fingerprint is not a wire type and never goes to disk).

## Non-goals

- **Not** a persisted format. It is a runtime oracle; ADR-0009 §3 decides what, if
  anything, gets persisted.
- **Not** a storage equivalence check. It hashes the *derived* index; scope and
  other side-tables are deliberately outside (there is a test pinning that
  boundary).
- **Not** authorizing any reduction in rebuild correctness — the fingerprint
  proves rebuild-equivalence, it does not replace the rebuild fallback.

## References

- `src/fingerprint.rs` (rules + FNV-1a helpers), `engine::index_fingerprint`,
  `engine::recompute_corpus_sums`, `engine::corpus_sums_drift`.
- ADR-0009 (index snapshot — §3 now has its validator; §4 measurement shipped in
  the same release).
- ADR-0002 (BQ filter), ADR-0004 (byte contracts), ADR-0007 (eras).
- Tests: `index_fingerprint_rebuild_invariant`, `…_is_insertion_order_independent`,
  `…_ignores_bq_orphans`, `…_covers_derived_indexes_only`,
  `…_import_equivalence`, `…_is_stable_across_reopen`,
  `validate_corpus_mean_has_no_drift_through_lifecycle`,
  `validate_detects_corpus_mean_sum_drift`, `…_count_mismatch`,
  `validate_mean_tolerance_absorbs_accumulation_order`.
