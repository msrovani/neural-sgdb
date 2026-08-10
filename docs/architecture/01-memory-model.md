# 01 — Memory Model

> Status: **v0.2 design target** — builds on the v0.2.0 crate (MemoryDoc, L0–L7,
> ART, BQ, Storage trait). Sections marked "exists" describe current code;
> "design" describes the v0.2 target. All English per repo policy.

## 1. What is a memory?

A memory is **not** a `key → value` record. It is a `MemoryDoc`: a cognitive
entity carrying layer semantics, identity, provenance and (optionally) a
semantic vector.

### 1.1 Today (v0.2.0, exists)

```text
MemoryDoc
 ├── layer      L0..L7 (cognitive storage class)
 ├── key        opaque string (logical id, e.g. "md/L2/turn-42")
 ├── clock      VectorClock (8 nodes fixed — 72B on disk)
 ├── payload    opaque bytes (text, fact, procedure, embedding)
 └── bitvec     optional binary-quantized vector (L4/L5, 1-bit per dim)
```

Format: NMD1 — byte-identical to neural-os-core (interop contract, golden
test `golden_nmd1_bytes`).

### 1.2 What is missing (design)

The model has structure but no **semantic state**. A v0.2 memory needs:

```text
MemoryDoc (v0.2)
 ├── layer
 ├── key
 ├── memory_id      stable global id (replication-friendly)
 ├── parent_ids     causal parents (merge DAG)
 ├── clock          VectorClock — DYNAMIC node identity (design §4)
 ├── payload
 ├── bitvec
 ├── state          active | superseded | archived | decayed
 ├── source         node_id that created it
 ├── confidence     [0..1] — trust in the content
 ├── importance     [0..1] — retention/reinforcement weight
 ├── valid_from/valid_to   temporal validity window
 └── associations   related_to/causes/contradicts/supports (design §5)
```

**NMD1 impact:** adding fields changes the byte layout — must be negotiated
with neural-os-core (shared contract) and versioned, not silently extended.

## 2. Layers L0–L7

| Layer | Name | Storage class | Index | Typical content |
|-------|------|---------------|-------|-----------------|
| L0 | Sensory | raw input | — | sensor/network frames (volatile) |
| L1 | Working | RAM | ART | current turn, immediate context |
| L2 | Short-term episodic | persistent | ART (ts) | recent timestamped turns |
| L3 | Long-term episodic | persistent | ART (ts) | consolidated episodes |
| L4 | Semantic | persistent | BQ + ART | embeddings, generalized facts |
| L5 | Procedural | persistent | BQ + ART | skills, procedures |
| L6 | (reserved) | — | — | v0.2 proposal: Associative/Metacognitive |
| L7 | Identity | persistent | ART (fixed key) | persona, preferences, trust state |

**Current truth (v0.2.0):** layers are **storage/index classes**, not a
lifecycle. A doc written at L2 stays L2 unless the caller writes it elsewhere.
The lifecycle engine (Doc 02) is the v0.2 work.

## 3. Layer semantics (design)

- **L0/L1** — volatile, RAM-only, explicit `checkpoint()` flushes to storage.
  Purpose: here-and-now; never survives reboot without checkpoint.
- **L2/L3** — episodic. L2 = recent (retention window), L3 = consolidated
  (survives pruning). Both timestamped, ART-indexed, sortable via
  `sortable_ts_key`.
- **L4** — semantic: embedding (BQ bitvec) + payload text. Retrieval by
  similarity, not key.
- **L5** — procedural: what *to do*. Skills indexed by name (ART) + semantic
  description (BQ).
- **L6** — v0.2 proposal: **Associative / Metacognitive memory** — not a new
  storage backend but a **relation index** over other layers:
  relationships, causal links, confidence, provenance, uncertainty,
  importance, associations. Implemented as a `MemoryGraph` (Doc 03 §4) whose
  edges are ordinary MemoryDocs at L6 with `payload = (a, rel, b)`.
- **L7** — identity: persona, preferences, global state. LWW-appropriate for
  CRDT (Doc 04), HITL for mutations.

## 4. VectorClock (design)

Today: fixed `[u8; 8]` nodes + `[u64; 8]` counters (72B, part of NMD1). Fine
for v0.1 clusters ≤ 8 nodes.

v0.2 design — **dynamic node identity**:
```text
NodeId  = u16 (dynamic registry, compact)
Clock   = BTreeMap<NodeId, u64>   (in memory)
        = serialized as (n u16 | node u16 | counter u64)*  (on disk)
```
- Compaction: drop zero counters; cap at MAX_NODES (config) with oldest-first
  eviction + provenance note.
- **NMD1 impact:** clock section becomes variable-length → format version bump
  required (negotiate with OS).

## 5. Associations / Memory Graph (design)

Today: memories are independent documents. v0.2 adds a **cognitive topology**
without turning the DB into a graph database:

```text
ART        = key topology      (exact/prefix lookup)
BQ         = semantic topology (similarity)
Graph      = cognitive topology (relations)   ← v0.2 L6
```

Relation types: `related_to`, `causes`, `contradicts`, `supports`,
`derived_from`, `supersedes`.

Storage: each relation is an L6 MemoryDoc with `payload = (a, rel, b)` and a
direction-aware ART key (`md/L6/rel/a→b`). Lookup: `scan_prefix("md/L6/rel/")`
→ filter by a/b. This keeps relations queryable with the existing ART while
the semantic graph stays simple (no Neo4j ambitions).

## 6. Memory states (design)

Instead of `delete` as the primary op (Doc 02), a memory has a lifecycle state:

```text
active ──► superseded ──► archived ──► (removed by GC)
   │            │
   └── decayed ─┘
```

- `superseded`: a newer memory replaced it (e.g. "moved to Y" supersedes
  "lives at X"). History preserved (`valid_until`).
- `archived`: no longer active but retained for audit/reflection.
- `decayed`: importance dropped below threshold; GC candidate.
- `delete` remains available for HITL/security but is **not** the default
  lifecycle operation.

## 7. Open questions

- [ ] NMD1 v0.2 format negotiation with neural-os-core (fields + variable
      clock) — version marker + golden updates in same commit
- [ ] L6 = Associative layer: confirm naming and relation vocabulary
- [ ] `memory_id` generation: hash of (source, clock) vs UUID-v7-like
      time-sortable id (preferred: sortable → ART ts prefix reuse)

## 8. Relationship to other docs

- Doc 02 — Memory Lifecycle: states, transitions, consolidation engine
- Doc 03 — Retrieval: ART/BQ/Graph as three retrieval mechanisms
- Doc 04 — Distributed: VectorClock dynamic identity, provenance, CRDT per layer
- Doc 05 — Storage: durability levels, WAL/checkpoint/compaction
- Doc 06 — Cognitive API: remember/recall/associate/reinforce/...
