# 06 — Cognitive API

> Status: **v0.2 design target**. Today (v0.2.0): `remember_exchange`,
> `remember_semantic`, `recall`, `rag_context`, `remember_fact`,
> `scan_prefix`, `checkpoint`, `get`, `recovered_records`. This doc defines
> the v0.2 cognitive surface — verbs of memory, not verbs of storage. All
> English per repo policy.

## 1. Principle

The API speaks **memory verbs**, not data verbs:

```text
v0.1:  put / get / delete            (storage verbs)
v0.2:  remember / recall / associate / reinforce / supersede / consolidate /
       forget / explain / transfer / merge      (cognitive verbs)
```

`put`/`get` remain on `Storage` (the backend ABI). `Sgdb` grows the cognitive
layer on top.

## 2. Target surface (design)

```rust
impl Sgdb {
    // ── write (existing) ──────────────────────────────────────
    pub fn remember_exchange(&mut self, user, response) -> Result<(), _>;
    pub fn remember_exchange_full(&mut self, user, response, emb_u, emb_a, now) -> Result<(), _>;
    pub fn remember_semantic(&mut self, key, text, emb) -> Result<(), _>;
    pub fn remember_fact(&mut self, fact, now) -> Result<(), _>;

    // ── read (existing) ────────────────────────────────────────
    pub fn recall(&mut self, query: &[f32], k) -> Result<Vec<Hit>, _>;
    pub fn rag_context(&mut self, query: &[f32], k) -> Result<String, _>;
    pub fn scan_prefix(&mut self, prefix) -> Result<Vec<(String, u64)>, _>;
    pub fn get(&mut self, layer, key) -> Result<Option<MemoryDoc>, _>;

    // ── v0.2 cognitive verbs (design) ──────────────────────────
    pub fn associate(&mut self, a: &str, rel: Rel, b: &str) -> Result<(), _>;
    pub fn related_to(&mut self, key: &str) -> Result<Vec<MemoryDoc>, _>;
    pub fn contradicts(&mut self, key: &str) -> Result<Vec<MemoryDoc>, _>;

    pub fn reinforce(&mut self, key: &str, delta: f32) -> Result<(), _>;
    pub fn supersede(&mut self, old_key: &str, new_key: &str) -> Result<(), _>;
    pub fn consolidate(&mut self, now: u64) -> Result<LifecycleReport, _>;
    pub fn forget(&mut self, key: &str, reason: ForgetReason) -> Result<(), _>;

    pub fn explain(&mut self, key: &str) -> Result<MemoryProvenance, _>;
    pub fn transfer(&mut self, transport: &mut dyn Transport, peer: PeerId) -> Result<(), _>;
    pub fn merge(&mut self, incoming: MemoryDoc) -> Result<MergeVerdict, _>;

    // lifecycle / storage control (existing + new)
    pub fn checkpoint(&mut self) -> Result<usize, _>;          // exists
    pub fn gc(&mut self, now: u64) -> Result<GcReport, _>;     // v0.2
}
```

## 3. Verb semantics

| Verb | Meaning | Backed by |
|------|---------|-----------|
| `remember_*` | write a memory at a layer | MemoryDoc (exists) |
| `recall` | semantic retrieval | BQ + FP32 rescore (exists) |
| `associate` | assert a relation a–rel–b | L6 MemoryDoc (Doc 01 §5, Doc 03 §4) |
| `related_to` / `contradicts` | relation queries | ART prefix over L6 |
| `reinforce` | importance += delta | lifecycle (Doc 02 §4) |
| `supersede` | flip old→superseded, new→active (history kept) | lifecycle (Doc 02 §4) |
| `consolidate` | run the lifecycle tick | MemoryLifecycle (Doc 02 §5) |
| `forget` | decay/archive (never silent delete, except HITL) | lifecycle states |
| `explain` | provenance: source, clock, parents, confidence, state | MemoryDoc fields |
| `transfer` | push/pull memory deltas to a peer | CRDT replication (Doc 04 §4) |
| `merge` | apply an incoming doc per layer policy | CRDT merge (Doc 04 §2) |
| `gc` | remove decayed/expired | storage GC (Doc 05 §3) |

## 4. MCP surface (v0.2)

MCP tools mirror the cognitive verbs (MCP stays in `examples/`, not core):

```text
v0.1 tools:  remember, recall, rag_context
v0.2 tools:  + associate, related_to, reinforce, supersede,
             consolidate, forget, explain
```

Each tool: `inputSchema` (JSON Schema), `isError` on cognitive failure,
provenance in results. Embedding stays demo-trigram (documented) until a real
model is provided by the embedder.

## 5. Scope guard (v0.2)

| Verb | Scope |
|------|-------|
| `associate` / `related_to` / `contradicts` | ✅ core (L6) |
| `reinforce` / `supersede` / `consolidate` | ✅ core (lifecycle) |
| `forget` (decay/archive) | ✅ core |
| `explain` (provenance) | ✅ core |
| `transfer` / `merge` | ✅ core (delta replication) |
| `gc` | ✅ core |
| Relation *inference* | ❌ v0.3 (needs BitNet) |

## 6. Open questions

- [ ] `Rel` enum vocabulary: `related_to`, `causes`, `contradicts`,
      `supports`, `derived_from`, `supersedes` — sufficient?
- [ ] `forget` default: archive-then-gc vs direct decay? (HITL for L7)
- [ ] Does `transfer` push or pull-first (pull-first: request deltas by
      clock — safer, Doc 04 §4)?

## 7. Relationship to other docs

- Doc 01 — Memory Model (fields), Doc 02 — Lifecycle (verbs), Doc 03 —
  Retrieval (queries), Doc 04 — Distributed (transfer/merge), Doc 05 —
  Storage (gc/checkpoint)

---

# INDEX — neural-sgdb Architecture (v0.2 design)

| Doc | Title | Core question |
|-----|-------|---------------|
| [01](01-memory-model.md) | Memory Model | What is a memory? |
| [02](02-memory-lifecycle.md) | Memory Lifecycle | How does memory live, move, consolidate, die? |
| [03](03-retrieval-architecture.md) | Retrieval Architecture | ART + BQ + Graph + reranking |
| [04](04-distributed-memory.md) | Distributed Memory | VectorClock + CRDT + provenance + conflicts |
| [05](05-storage-architecture.md) | Storage Architecture | WAL + checkpoint + segments + compaction + GC |
| [06](06-cognitive-api.md) | Cognitive API | remember/recall/associate/... surface |

**Reading order:** 01 → 02 → 03 → 04 → 05 → 06. Each doc ends with scope
guard (what ships v0.2) and open questions (what to decide before coding).

**Positioning (from the architectural review):**

```text
        BitNet (Reasoning/Cortex)
                 │
          Cognitive API
                 │
         neural-sgdb (MEMORY)
                 │
         neural-os-core (substrate)
```

> neural-sgdb is the **memory of the brain**, not the brain itself.
