# 03 — Retrieval Architecture

> Status: **current (v1.1.26)** — ART + BQ + lexical + entities + typed hits
> ship in production code. MCP default retrieval is **lexical** (ADR-0008).
> **implemented** = code + tests; **remaining** =
> honest gap. All English per repo policy.

## 1. Retrieval mechanisms

```text
                    MEMORY RETRIEVAL
                          │
           ┌──────────────┼──────────────┐
           ▼              ▼              ▼
      SYMBOLIC        SEMANTIC        COGNITIVE
      exact/key       similarity      entities + relations
           │              │              │
          ART             BQ          entity_index + L6 rels
           │              │              │
      scan_prefix     recall*        recall_entities
      get             hybrid         related_to / contradicts
                      temporal
                      lexical (BM25)
```

| Mechanism | Index | Entry points | Use case |
|-----------|-------|--------------|----------|
| Symbolic | ART O(k) | `scan_prefix`, `get` | facts, keys, timestamps |
| Semantic | BQ + FP32 | `recall`, `recall_weighted`, `recall_temporal` | similarity L4/L5 |
| Lexical | inverted BM25 | `recall_lexical`, `recall_hybrid` | L2/L3 text, no embedding |
| Entities | entity_index | `recall_entities` | 1-hop overlap (exact strings) |
| Relations | `sys/rel/` + ART | `related_to`, `contradicts`, … | graph adjacency |

Modes are **complementary**: ART = "what's under this prefix", BQ = "what's
similar", lexical = "which words match", entities = "which declared entities overlap".

## 2. Symbolic retrieval (ART) — implemented

- `ArtIndex` Node4/16/48/256, prefix scan, node reclamation on delete.
- **Prefix-key guard (P1-7):** `has_prefix_conflict` rejects keys where one
  is a prefix of another **before** write — loud `Invalid`, not silent loss.
- `scan_prefix_page` — deterministic lexicographic pagination (P1-6).

## 3. Semantic retrieval (BQ) — implemented

### Pipeline

```text
query FP32
      ↓ sign quantization (x > 0 → bit 1)
binary vector (u64 words)
      ↓ Hamming distance (SIMD AVX-512/AVX2/scalar)
top-k candidates (bounded heap + auto-oversample)
      ↓ FP32 rescore (1−cosine over payload)
final ranking → Hit { path=Semantic, content_type, payload_type, … }
```

- **ADD-only contract (v1.1.4):** BQ flat is append-only; new facts accumulate;
  conflict resolution is retrieval-time (`supersede`, `recall_weighted`), not
  silent overwrite.
- **Orphan reclamation:** `BqFlatIndex::retain` + `reclaim_bq_orphans` on
  delete (threshold 64 by default).
- **Era guard (ADR-0007):** S1 on read (dim mismatch → `Invalid`); write-side
  guard on `remember_semantic`; `era_report()` for migration planning.
- **MihIndex:** multi-index hashing for sub-linear candidate generation
  (study/advanced API).

### Honest positioning

| Corpus size | Verdict |
|-------------|---------|
| ~10k vectors | excellent (see BENCHMARKS.md) |
| ~100k | viable with SIMD + oversample |
| 1M+ | not a vector DB — revisit only on benchmark evidence |

## 4. Lexical retrieval (BM25) — implemented

- Inverted index over L2/L3 tokenized text (`src/lexical.rs`).
- `search` returns `(key, score, matched_terms)` — grounding for typed hits.
- Scoped: `recall_lexical_scoped` honors same scope filter as semantic recall.
- Companion `/L2/` scope comes from primary via `Engine::effective_scope`.
- **MCP default (ADR-0008):** `recall`/`rag_context` use lexical when no
  `embedding=` is supplied. Semantic/hybrid are opt-in.

## 5. Hybrid, temporal, weighted — implemented

| API | Behavior |
|-----|----------|
| `recall_hybrid` | semantic ∪ lexical, deduplicated |
| `recall_temporal` | semantic pool re-ranked by proximity to `at` |
| `recall_weighted` | `w_sem·dist + w_rec·recency + w_imp·(1−importance)` |
| `recall_scoped` | scope filter inside candidate pool (null-scoping) |
| `rag_context_reranked` | oversampled pool + lexical anchor rerank (`anchors=N`) |
| `recall_adaptive` | escalates `1→4→8→16` while the top-k boundary is ambiguous (ADR-0012) |

Default recall: **active memories only**. Historical variants opt in.

### How much effort a recall deserves (v1.1.22 — implemented, ADR-0012)

Every other recall API here takes a FIXED candidate budget: `k · oversample`,
chosen by the caller or by dimensionality. The cost is linear in that budget and
the benefit is not, so the default overpays on the easy majority.
`recall_adaptive` makes the budget a consequence of the **ambiguity of the top-k
boundary**: each ladder step asks for `k+1` hits and stops when the RAW u32 gap
between the last kept hit and the first dropped one exceeds `SCORE_TIE_MARGIN`
(the margin the state-first ranking already uses to call two scores "same
content, different version"). Two details are load-bearing:

- The comparison is on the **raw score**, never on the `score/(MARGIN+1)` bucket
  the ranking sorts by — buckets have arbitrary edges, and two documents 0.0026
  cosine apart can land in different buckets and look "decided".
- `RecallProbe { considered, survivors, budget }` separates "the store ran out"
  from "the pool ran out before the valid docs" (`saturated()`). Without it, a
  scope whose documents sit deep in the BQ order would get a silently truncated
  top-k, because the scope filter runs INSIDE the candidate loop.

**Measured honestly:** with `SCORE_TIE_MARGIN` as the threshold, escalating is
more often the case than the exception (see `BENCHMARKS.md` §Recall adaptativo).
The API is therefore opt-in and ships as an **instrument**
(`oversample_used` / `escalations` / `boundary_decisive` / `probe`) so a host can
pick its own ceiling from data — the same posture as ADR-0009 §4.

## 6. Typed hits (v1.1.6 — implemented)

Machine consumers parse structured hits, not lossy prose:

```text
Hit {
  key, text, dist, score, path, content_type, payload_type,
  matched_terms, validity, rel, provenance
}
```

- Prose projection only for Text/Json/Code.
- Embedding/Binary → empty `text`; consumer reads `content_type`/`payload_type`.
- **`payload_type` honors the LAYER (v1.1.25):** `Embedding(dim)` only for
  L4/L5 — the layers the BQ indexes. Before, the `index_doc` arithmetic
  (`len % 4 == 0`) labelled **any prose whose byte length was a multiple of 4**
  as a vector (a quarter of all texts), so a machine consumer could try to
  reuse a vector that never existed.
- MCP `format=json` — stable string labels.
- `rel` links companion `/L2/` → primary `/L4|L5|L3/`.

## 7. Relations and entities — implemented

- **Entities:** declared in `MemoryMeta.entities` (MDM1 v5); index rebuilt
  from `sys/meta/`; 1-hop recall by exact string overlap.
- **Relations:** `sys/rel/` + ART; no embedding, no inference.

## 8. RAG assembly — implemented

- `rag_context` / `rag_context_limited` — recall + companion text + byte cap.
- `rag_context_reranked` — lexical anchor gate before truncation (P1/P5).
- MCP supports `mode`, `rerank`, `format=json`.

## 9. Remaining gaps

- **Residual / hierarchical BQ** — coarse→fine beyond heap+oversample (v1.x
  medium-term, benchmark-driven).
- **Semantic sharding** — deferred until measurable need (~1M+ vectors).
- **Relation-aware retrieval fusion** — relations exist; recall does not yet
  re-rank semantic hits by graph distance (upper layer can compose).

## 10. Relationship to other docs

- Doc 01 — Memory Model: layers, content types
- Doc 02 — Lifecycle: active-only recall
- Doc 04 — Distributed: replication does not change recall local semantics
- Doc 06 — Cognitive API: MCP recall modes + `format=json`
