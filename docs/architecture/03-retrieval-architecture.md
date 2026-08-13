# 03 — Retrieval Architecture

> Status: **v0.2 design target**. Today (v0.2.0): ART (symbolic) + BQ flat
> scan + FP32 rescore exist and are tested. This doc defines the retrieval
> architecture v0.2: three retrieval mechanisms, honest performance framing,
> and the bounded top-k / hierarchical path. All English per repo policy.

## 1. Three retrieval mechanisms

Cognitive memory needs **two** (and v0.2 adds a third) retrieval modes:

```text
                    MEMORY RETRIEVAL
                          │
           ┌──────────────┼──────────────┐
           ▼              ▼              ▼
      SYMBOLIC        SEMANTIC        COGNITIVE
      exact/key       similarity      relations
           │              │              │
          ART             BQ          Graph (L6, v0.2)
           │              │              │
      scan_prefix     top-k +       relation lookup
                      FP32 rescore
```

| Mechanism | Index | Query | Use case |
|-----------|-------|-------|----------|
| Symbolic | ART (O(k)) | `scan_prefix("md/L3/")`, exact get | facts, timestamps, keys |
| Semantic | BQ (O(N·D/64)) | `recall(&emb, k)` | similarity over L4/L5 |
| Cognitive | Graph (L6, v0.2) | `related(a)`, `causes(a)` | associations, contradictions |

They are **complementary, not competing**: ART answers "what did I store
under this key", BQ answers "what is most similar to this", Graph answers
"what is connected to this".

## 2. Symbolic retrieval (ART) — exists, frozen

- `ArtIndex` Node4/16/48/256, prefix scan, tombstone delete, O(k) lookup.
- Keys are hierarchical (`md/L1/last_user`, `md/L3/ts/…`) → `scan_prefix`
  becomes a layer/namespace query. This is correct and **should not change**
  (review P0: don't touch ART).
- Inherited limitation: prefix keys unsupported (fixed-width suffixes).
  **Hardened (P1-7)**: `ArtIndex::has_prefix_conflict` + guards in
  `engine::put` / `engine::associate` reject a key that is a prefix of (or
  whose prefix is) an existing key with `SgdbError::Invalid` BEFORE writing —
  the silent corruption (short key becomes unreachable) is now a loud error.

## 3. Semantic retrieval (BQ) — honest framing

### 3.1 Current pipeline (exists)

```text
embedding FP32
      ↓ sign quantization (x > 0 → bit 1)
binary vector (u64 words)
      ↓ Hamming distance (SIMD AVX-512/AVX2/scalar)
top-k candidates (flat scan + full sort)
      ↓ FP32 rescore (1−cos over original payload)
final ranking
```

- `BqFlatIndex`: `ids[]`, `flat[]`, `words_per_vec` — contiguous, cache-friendly.
- Complexity: **O(N·D/64) scan + O(N log N) sort** per query.

### 3.2 Honest positioning

| Vectors | Verdict |
|---------|---------|
| 10k | excellent |
| 100k | viable (SIMD + FP32 rescore) |
| 1M+ | inadequate as-is |

**Do not market this as a "vector database".** It is a *cognitive memory
database with compact semantic retrieval* — the accurate, and more
interesting, framing.

### 3.3 v0.2: bounded top-k (review P1)

Replace full sort with a bounded top-k (binary heap of size k):

```text
for each vector: d = hamming(query, vec)
    if heap.len() < k: push
    else if d < heap.max(): replace-max
→ O(N·D/64 + N log k)   (k << N)
```

Isolated change in `BqFlatIndex::top_k`; no API change. Falls back to sort for
small N (heap overhead not worth it under ~256 vectors).

### 3.4 v0.2: hierarchical retrieval (review P3, staged)

```text
coarse binary index (all vectors, cheap)
      ↓ candidate set (e.g. top 4k by Hamming)
compact residual / scalar representation (design)
      ↓
final FP32 ranking on candidates
```

Enables millions of memories without full FP32 scans. **Residual
representation is v0.3+** (needs a compact residual encoder; BitNet-affine:
low-bit residual + integer ops). v0.2 ships only the bounded heap.

### 3.5 Semantic sharding (review P2, v0.3)

Shard BQ by content locality (e.g. coarse hash of the first 2 words) and query
only relevant shards. Deferred — no measurable need below ~1M vectors.

## 4. Cognitive retrieval — Memory Graph (v0.2)

Relations are L6 MemoryDocs (Doc 01 §5): `payload = (a, rel, b)`, key
`md/L6/rel/a→b`. Query surface:

```rust
// v0.2 (design)
db.related_to(a)      -> Vec<MemoryDoc>   // any rel touching a
db.rel_lookup(a, rel) -> Vec<MemoryDoc>   // "what causes a", "what a supports"
db.contradicts(a)     -> Vec<MemoryDoc>   // conflict surface for CRDT
```

- **No inference in v0.2** — relations are asserted (agent/LLM/HITL) and
  stored. Inference needs BitNet (v0.3+).
- Bounded fan-out: relations are edges, not re-embeddings — cheap to scan via
  ART prefix.

## 5. RAG assembly

Current: `rag_context(query, k)` — recall → fetch companion L2 text → format
`[SGDB-RAG top-k]` block. v0.2 additions:

- **Provenance in the block**: `(layer, source, confidence, state)` per hit —
  lets the prompt say "this memory is superseded" instead of silently using it.
- **Layer-aware assembly**: `rag_context` can restrict to `active` memories
  (skip superseded/decayed) via the lifecycle state (Doc 02).

## 6. Scope guard (v0.2)

| Item | Scope |
|------|-------|
| Bounded top-k heap in `BqFlatIndex` | ✅ core (P1) |
| `related_to`/`rel_lookup`/`contradicts` (Graph over L6) | ✅ core |
| Provenance-aware `rag_context` | ✅ core |
| Residual representation + reranking | ❌ v0.3 |
| Semantic sharding | ❌ v0.3 |
| Relation inference | ❌ v0.3 (needs BitNet) |

## 7. Open questions

- [ ] Heap vs sort threshold (empirical; keep both paths, bench in
      `examples/bench.rs`)
- [ ] Relation direction encoding in ART key (a→b vs b→a both indexable?)
- [ ] Does `rag_context` filter by `state=active` by default or opt-in?

## 8. Relationship to other docs

- Doc 01 — Memory Model: L6 relations, NMD1 impact
- Doc 02 — Lifecycle: state filters for retrieval
- Doc 04 — Distributed: contradictions = CRDT conflict surface
- Doc 05 — Storage: indexes rebuild from storage on open
- Doc 06 — Cognitive API: `recall`, `rag_context`, `associate`
