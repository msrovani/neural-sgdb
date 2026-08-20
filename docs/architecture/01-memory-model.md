# 01 — Memory Model

> Status: **current (v1.1.6)** — describes the shipped model. **implemented**
> = code + tests; **remaining** = honest gap. All English per repo policy.

## 1. What is a memory?

A memory is **not** a `key → value` record. It is a `MemoryDoc`: a cognitive
entity carrying layer semantics, identity, provenance and (optionally) a
semantic vector.

### 1.1 NMD1 document (implemented)

```text
MemoryDoc (on disk: NMD1 — byte-identical to neural-os-core)
 ├── layer      L0..L7 (cognitive storage class)
 ├── key        opaque string (e.g. "md/L4/turn-1")
 ├── clock      VectorClock (72B fixed in NMD1)
 ├── payload    opaque bytes (text, fact, procedure, embedding)
 └── bitvec     optional binary-quantized vector (L4/L5)
```

Golden test `golden_nmd1_bytes` pins the layout. **The NMD1 blob does not
change** when metadata evolves — new fields live in side-tables (ADR-0003).

### 1.2 Side-table metadata (implemented — MDM1 v6)

Semantic state and provenance travel in `sys/meta/` (MDM1), not inside NMD1:

```text
MemoryMeta (MDM1 v6, side-table sys/meta/)
 ├── memory_id          stable 32-hex identity
 ├── version_id         per-version identity (v2+)
 ├── source             creating node_id
 ├── confidence         [0..1]
 ├── importance         [0..1]
 ├── created_tick       creation counter
 ├── parent_ids         causal parents (merge DAG)
 ├── last_reinforced    (v3+, from reinforce())
 ├── scope              (v4+, multi-agent scoping)
 ├── entities           (v5+, declared entity strings)
 └── content_type       (v6+, declared stable label)
```

Additional side-tables (same pattern — NMD1 untouched):

| Side-table | Purpose |
|------------|---------|
| `sys/state/` | `MemoryState`: Active / Superseded / Archived / Invalidated / Decayed |
| `sys/validity/` | bi-temporal window `[from, until)` — invalidate-not-delete |
| `sys/rel/<kind>/` | L6 associative edges (forward + reverse ART index) |
| `sys/version/` | reverse index: `(node, counter) → storage keys` |
| `sys/conflict/` | first-class conflict records (CFL1) |

**Replication unit:** `MemoryRecord` (MDR1 wire) = doc + state + validity +
meta — one import/export/merge unit (v0.6+).

## 2. Layers L0–L7

| Layer | Name | Storage | Index | Typical content |
|-------|------|---------|-------|-----------------|
| L0 | Sensory | RAM | — | raw input (volatile) |
| L1 | Working | RAM | ART | current turn |
| L2 | Short-term episodic | persistent | ART + lexical | timestamped turns, verbatim companions |
| L3 | Long-term episodic | persistent | ART + lexical | consolidated facts/episodes |
| L4 | Semantic | persistent | BQ + ART | embeddings + generalized facts |
| L5 | Procedural | persistent | BQ + ART | skills, procedures |
| L6 | Associative | side-table `sys/rel/` | ART forward/rev | causes/supports/contradicts/derived_from |
| L7 | Identity | persistent | ART | persona, preferences |

Layers are **storage/index classes**. Promotion between layers is explicit
(`MemoryLifecycle::tick`, `transfer_to`) — a doc at L2 stays L2 until moved.

## 3. Layer semantics (implemented)

- **L0/L1** — volatile RAM; `checkpoint()` flushes to storage.
- **L2/L3** — episodic, timestamped (`sortable_ts_key`), lexical-indexed.
  `remember_episodic` stores raw user/response pairs verbatim (v1.1.4).
- **L4/L5** — semantic/procedural: BQ bitvec + companion `/L2/` text for
  lexical retrieval and RAG. **Write-side era guard** (ADR-0007): embedding
  dim outside `indexed_dims` on a live corpus → `Invalid`.
- **L6** — relations asserted by the upper layer (`associate`); **no
  inference in core**. Stored in `sys/rel/`, indexed in ART, pruned on delete.
- **L7** — identity; LWW-appropriate for CRDT; HITL for mutations.

## 4. VectorClock (implemented)

- **NMD1:** fixed 72B (8 nodes × u64 counters) — interop contract unchanged.
- **Runtime:** dynamic node identity — 8-node fast path + bounded overflow
  registry (248 extra nodes). Overflow persists in `MemoryMeta.clock_overflow`.
- Compare/merge/happens_before use `iter_nodes` (fixed + overflow).

**Remaining:** variable-length clock inside NMD1 would require OS negotiation
and a format version bump — not planned while side-table overflow works.

## 5. Associations (implemented)

Relations are **not** L6 MemoryDocs in the current implementation — they live
in `sys/rel/<kind>/<a>#<b>` with derived ART indices:

```text
RelationKind: related_to | causes | supports | contradicts | derived_from
API: associate / related_to / causes / supports / contradicts / derived_from
     associate_checked (validates both endpoints exist)
```

Lookup is O(k) via ART prefix/reverse index. **No graph inference** — strings
and keys must match exactly (same contract as `entities` and `Embedder`).

## 6. Memory states (implemented)

```text
active ──► superseded ──► archived
   │            │
   └── decayed ─┘
         └── invalidated (validity window closed)
```

- `supersede(old, new)` — history preserved; loser marked Superseded.
- `forget` — archives (never silent delete by default lifecycle).
- `delete` — **physical** tombstone + index removal (distinct from logical state).
- Default recall filters **active only**; `recall_historical*` opts in.

## 7. Content typing (v1.1.6 — implemented)

Hits expose typed datums for machine consumers (`src/ctype.rs`):

| Type | Meaning |
|------|---------|
| Text / Json / Code | prosa projection in `Hit.text` |
| Embedding(dim) | raw f32 payload — never `from_utf8_lossy` |
| Binary | non-UTF8 bytes |

Write seam: `set_content_type` (MDM1 v6) — **declared wins** over read-time
detector. Propagates to L4/L5 companion `/L2/`.

## 8. Remaining gaps

- Multi-value register encoding **inside NMD1** for L2/L3 (today: conflict
  preservation via CRDT + side metadata, not co-located value lists in one key).
- Automatic entity/relation **inference** from text (deliberate non-goal —
  upper layer provides strings).
- In-record `valid_from/valid_to` (today: `sys/validity/` side-table).

## 9. Relationship to other docs

- Doc 02 — Lifecycle: transitions, `MemoryLifecycle::tick`
- Doc 03 — Retrieval: ART/BQ/lexical/entities, typed `Hit`
- Doc 04 — Distributed: clock, replication, merge policy
- Doc 05 — Storage: persistence, compaction
- Doc 06 — Cognitive API: public verbs + MCP
