# Architecture — neural-sgdb

> Status: **current (v1.1.6)** — these documents describe the **shipped**
> cognitive memory system. Crate version in `Cargo.toml` is **1.1.6**; feature
> releases through **v1.1.6** are documented in `CHANGELOG.md` and
> `docs/api.md`. Sections marked **implemented** reflect code + tests; **remaining**
> marks honest gaps or deliberate non-goals.

| Doc | Title | Core question |
|-----|-------|---------------|
| [01](01-memory-model.md) | Memory Model | What is a memory? |
| [02](02-memory-lifecycle.md) | Memory Lifecycle | How does memory live, move, consolidate, die? |
| [03](03-retrieval-architecture.md) | Retrieval Architecture | ART + BQ + lexical + entities + typed hits |
| [04](04-distributed-memory.md) | Distributed Memory | VectorClock + CRDT + provenance + conflicts |
| [05](05-storage-architecture.md) | Storage Architecture | Durability + WAL + checkpoint + compaction |
| [06](06-cognitive-api.md) | Cognitive API | remember/recall/associate/… + MCP surface |

**Reading order:** 01 → 02 → 03 → 04 → 05 → 06.

**Related docs:** [`docs/api.md`](../api.md) (public contract),
[`docs/implementation-status.md`](../implementation-status.md) (capability matrix),
[`ROADMAP.md`](../../ROADMAP.md) (done / next / non-goals),
[`CHANGELOG.md`](../../CHANGELOG.md).

**Positioning:**

```text
        Agent / LLM (reasoning — outside the crate)
                 │
          Cognitive API + MCP (examples/)
                 │
         neural-sgdb (MEMORY substrate)
                 │
         Storage trait (InMemory / FileStorage / TickvFile)
                 │
         neural-os-core interop (NMD1 + TKLV byte-identical)
```

> neural-sgdb is the **memory of the brain**, not the brain itself. The core
> never generates embeddings, never extracts entities from text, and never
> decides semantic truth — it supplies material for the layer above.
