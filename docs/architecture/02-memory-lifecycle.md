# 02 — Memory Lifecycle

> Status: **current (v1.1.10)** — the lifecycle engine ships in `src/lifecycle.rs`.
> **implemented** = code + tests; **remaining** = honest gap. All English per
> repo policy.

## 1. Why a lifecycle?

The system answers both "where is this stored?" (layer) and "what state is it
in?" (active/superseded/decayed/…). Explicit transitions prevent layers from
being mere namespaces.

## 2. Canonical lifecycle

```text
                    ┌────────────┐
   sensor/input ──► │ L0 Sensory │  (volatile)
                    └─────┬──────┘
                          │ attention (upper layer)
                          ▼
                    ┌────────────┐
                    │ L1 Working │  (RAM)
                    └─────┬──────┘
                          │ checkpoint / remember_exchange
                          ▼
                    ┌────────────┐
                    │ L2 Episodic│  (short-term, timestamped)
                    └─────┬──────┘
                          │ MemoryLifecycle::tick (promote)
                          ▼
                    ┌────────────┐
                    │ L3 Episodic│  (long-term)
                    └─────┬──────┘
                          │ heuristic semanticization
                          ▼
                    ┌────────────┐
                    │ L4 Semantic│  (embedding + fact)
                    └─────┬──────┘
                          │ manual / HITL
                          ▼
                    ┌────────────┐
                    │ L5 Procedural│
                    └────────────┘

   ── orthogonal: L6 relations (sys/rel/), L7 identity
```

## 3. Transitions (implemented)

| Transition | Trigger | Implementation |
|------------|---------|----------------|
| L1 → L2 | `remember_exchange` / tick commit | exists; tick promotes working set |
| L2 → L3 | `MemoryLifecycle::tick` | importance + age threshold |
| L3 → L4 | tick semanticization | creates L4 **without** bitvec — embedding is upper layer's job |
| L4 → L5 | agent/HITL | `transfer_to` — not automatic |
| any → Decayed | tick decay | importance below threshold → `Decayed` |
| superseded chain | `supersede(old, new)` | state flip + `parent_ids` |

`MemoryLifecycle::tick(db, now)` is **deterministic and idempotent** — `now`
is injected (no hidden wall clock). Returns `LifecycleReport`.

## 4. Decay, reinforcement, supersede (implemented)

### Decay
- Lifecycle tick reduces importance per layer rates (`LifecycleConfig`).
- Below threshold → `Decayed` — never silent physical delete.

### Reinforcement
- `reinforce(key, delta)` — bumps importance + `last_reinforced` (MDM1 v3).
- `feedback(key, positive, amount)` — adjusts importance **and** confidence
  (v1.1.4).

### Supersede
```text
Memory A: "user lives at X"  → Superseded
Memory B: "user moved to Y"  → Active, parent_ids link to A
```
History preserved for audit and CRDT merge.

### Temporal validity (implemented)
- `set_validity` / `invalidate` / `expire_old(now)` — bi-temporal windows.
- Default recall ignores expired; `recall_historical*` / `recall_temporal` opt in.

## 5. Consolidation engine (implemented)

```rust
pub struct MemoryLifecycle { cfg: LifecycleConfig }
pub fn tick(&mut self, db: &mut Sgdb, now: u64) -> LifecycleReport;
```

Report fields: promoted, consolidated, semanticized, decayed, archived counts.
Every promotion wires `parent_ids` + `derived_from` relation.

## 6. Cognitive lifecycle verbs (implemented)

| Verb | Role |
|------|------|
| `supersede` | history-preserving update |
| `forget` | archive (logical) |
| `delete` | physical removal |
| `reinforce` / `feedback` | importance/confidence signals |
| `expire_old` | sweep closed validity windows |
| `transfer_to` | layer move with lineage |
| `merge_memories` | fusion with parent_ids=[A,B] |

## 7. Remaining gaps

- **Automatic tick scheduling** — explicit `tick()` only (no_std-friendly;
  embedder runs in app loop or session open — see `agent_protocol.rs`).
- **L3→L4 embedding backfill** — core creates L4 doc; upper layer must
  `remember_semantic` or raw put + bitvec.
- **L4→L5 auto proceduralization** — manual/HITL by design.
- **State-driven physical GC** — decay/archive mark state; compaction reclaims
  tombstones; no automatic purge of old superseded blobs by retention policy yet.

## 8. Relationship to other docs

- Doc 01 — Memory Model: states, side-tables
- Doc 03 — Retrieval: active-only default recall
- Doc 04 — Distributed: supersede chain in merge DAG
- Doc 05 — Storage: compaction vs lifecycle state
- Doc 06 — Cognitive API: verb surface + MCP
