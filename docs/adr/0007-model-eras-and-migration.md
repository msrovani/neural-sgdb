# ADR-0007 — Embedding model = era invariant; era migration is the switch path

Status: Accepted *(updated 2026-09-16 — `model_id` shipped in MDM1 v7 / v1.1.14–15)*

## Context

Semantic recall runs on caller-supplied embeddings (`remember_semantic`/`recall`
take `&[f32]`, ADR-0002, the `Embedder` trait in `src/embedder.rs`). The S1
guard (v1.1.3) rejects queries whose dimensionality matches none of the
`indexed_dims`. But the model is only a convention, not a mechanism: the guard
checks DIMENSIONS, not model identity. Two real hazards when a user/agent swaps
its embedding model:

- **Same dim, different model** → without an explicit model label, the dim
  guard is silent, old and new vectors do not cross-match, and semantic recall
  of old memories silently degrades.
- **Different dim in the same live DB** → `BqFlatIndex` locks `words_per_vec`
  on the first insert (`src/bq.rs`) and silently truncates/pads every later
  vector to that width. Writing new-era docs into a live BQ corrupts even the
  NEW era's index (payload stays intact, ranking becomes garbage — no error).

A model swap is therefore an **era transition**: the embedding payloads frozen
in the NMD1 files (`md/L4/`, `md/L5/`) belong to era X and never re-embed
automatically (rebuild only re-reads payloads, `src/engine.rs`). The past
remains reachable through the embedding-free paths (`recall_lexical`,
`recall_entities`; `recall_temporal` still needs an era-matching embedding
because it re-ranks the semantic pool).

## Decision

- **The embedding model is an era invariant per corpus (DB file).** One
  model per DB; switching models means opening a new DB file for the new era,
  or running an explicit **era migration**.
- **Never write a different-dim embedding into a live, already-populated BQ**
  (width lock ⇒ silent truncation). Write-side era guard (v1.1.5): dim outside
  `indexed_dims` on a live corpus → `SgdbError::Invalid` (nothing written).
  Era migration must rewrite ALL primary docs first, then
  `Sgdb::rebuild_indices()` (clears the BQ, resets `words_per_vec`, reindexes
  at the new width — `src/engine.rs:rebuild_indices_from_storage`).
- **Era migration procedure** (benchmark/prototype in
  `examples/era_migration_bench.rs`; public API
  `scan_prefix`/`get`/`remember_semantic`/`rebuild_indices` suffices):
  1. scan `md/L4/` (ids) — 2. read each companion `md/L2/<id>` text —
  3. re-embed the preserved text with the new model — 4. rewrite
     `remember_semantic(id, text, new_emb)` (identity is stable per key:
     memory_id/source/created survive the overwrite, `src/engine.rs:put`) —
  5. `rebuild_indices()` to reset the BQ width.
- **Embedding-free paths are the permanent recovery net**: after any era
  switch, the old era is always reachable lexically and by entities; the text
  is never lost, so semantics can always be rebuilt later.
- **`model_id` is shipped (MDM1 v7, v1.1.14/15)** — persisted on
  `MemoryMeta` alongside `ScopeDims{user,agent,app,run}`. `era_report` /
  health can surface `mixed_models` when labels disagree. Hosts SHOULD set the
  same `model_id` on write and query for a live era (ADR-0008 local embedder /
  `nsgdb-embed` `LOCAL_MODEL_ID`). Dim guard alone is still insufficient for
  same-dim silent swaps if the host omits `model_id`.

## Consequences

- Positive: model swaps are safe, documented and measurable (the benchmark
  reports per-phase cost: scan / text read / re-embed / rewrite / rebuild);
  past data is never destroyed — worst case it degrades to
  lexical/entity-only and is re-promotable by re-embedding; `model_id` +
  `mixed_models` make same-dim cross-era detectable when the host labels
  writes.
- Negative / cost: era migration is an O(N) write pass (rewrites every L4/L5
  payload + companion) plus a full index rebuild; it bumps each doc's causal
  version (overwrite = new version of the same identity), which churns CRDT
  deltas. Hosts that leave `model_id` empty keep the old silent same-dim risk.
- Contract impact: MDM1 v7 is an additive side-table bump (ADR-0003); NMD1/
  TKLV unchanged. Revisiting the BQ width lock (e.g. per-dim buckets) needs
  its own ADR + benchmarks (ADR-0002). Index snapshot on open is ADR-0009
  (separate from era migration).
