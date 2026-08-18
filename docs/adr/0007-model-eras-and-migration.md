# ADR-0007 — Embedding model = era invariant; era migration is the switch path

Status: Accepted

## Context

Semantic recall runs on caller-supplied embeddings (`remember_semantic`/`recall`
take `&[f32]`, ADR-0002, the `Embedder` trait in `src/embedder.rs`). The S1
guard (v1.1.3) rejects queries whose dimensionality matches none of the
`indexed_dims`. But the model is only a convention, not a mechanism: the guard
checks DIMENSIONS, not model identity. Two real hazards when a user/agent swaps
its embedding model:

- **Same dim, different model** → the guard is silent, old and new vectors do
  not cross-match, and semantic recall of old memories silently degrades.
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
  (width lock ⇒ silent truncation). Era migration must rewrite ALL primary
  docs first, then `Sgdb::rebuild_indices()` (which clears the BQ, resets
  `words_per_vec` and reindexes every doc at the new width —
  `src/engine.rs:rebuild_indices_from_storage`).
- **Era migration procedure** (implemented as a benchmark/prototype in
  `examples/era_migration_bench.rs`, zero lib changes — the public API
  `scan_prefix`/`get`/`remember_semantic`/`rebuild_indices` suffices):
  1. scan `md/L4/` (ids) — 2. read each companion `md/L2/<id>` text —
  3. re-embed the preserved text with the new model — 4. rewrite
     `remember_semantic(id, text, new_emb)` (identity is stable per key:
     memory_id/source/created survive the overwrite, `src/engine.rs:put`) —
  5. `rebuild_indices()` to reset the BQ width.
- **Embedding-free paths are the permanent recovery net**: after any era
  switch, the old era is always reachable lexically and by entities; the text
  is never lost, so semantics can always be rebuilt later.
- **Optional future**: record `model_id` in `MemoryMeta` (MDM1 v6) so era
  membership is detectable and the migration tool can filter by era instead of
  "migrate everything".

## Consequences

- Positive: model swaps are safe, documented and measurable (the benchmark
  reports per-phase cost: scan / text read / re-embed / rewrite / rebuild);
  past data is never destroyed — worst case it degrades to
  lexical/entity-only and is re-promotable by re-embedding.
- Negative / cost: era migration is an O(N) write pass (rewrites every L4/L5
  payload + companion) plus a full index rebuild; it bumps each doc's causal
  version (overwrite = new version of the same identity), which churns CRDT
  deltas. Same-dim model swaps stay undetectable until `model_id` exists.
- Contract impact: none — no format, API, or feature change. Revisiting the
  BQ width lock (e.g. per-dim buckets) or adding `model_id` needs its own ADR.