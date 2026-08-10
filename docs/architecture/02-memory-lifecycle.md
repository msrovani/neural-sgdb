# 02 — Memory Lifecycle

> Status: **v0.2 design target**. Today (v0.2.0) the crate is a *memory storage
> engine*: layers are storage classes. This doc defines the *memory lifecycle
> engine*: how a memory is born, moves between layers, consolidates, decays and
> dies. All English per repo policy.

## 1. Why a lifecycle?

The current system answers "where is this memory stored?" (layer) but not:

> **why is this memory in this layer, and when should it move?**

A cognitive memory system needs explicit transitions. Without them, layers are
just namespaces — which is exactly the v0.1 limitation the architectural
review identified.

## 2. Canonical lifecycle

```text
                    ┌────────────┐
   sensor/input ──► │ L0 Sensory │  (volatile, RAW)
                    └─────┬──────┘
                          │ attention (salience filter)
                          ▼
                    ┌────────────┐
                    │ L1 Working │  (current context, RAM)
                    └─────┬──────┘
                          │ turn ends → checkpoint
                          ▼
                    ┌────────────┐
                    │ L2 Episodic│  (short-term, timestamped)
                    └─────┬──────┘
                          │ consolidation (repetition/importance)
                          ▼
                    ┌────────────┐
                    │ L3 Episodic│  (long-term)
                    └─────┬──────┘
                          │ regularity extraction
                          ▼
                    ┌────────────┐
                    │ L4 Semantic│  (generalized fact, embedding)
                    └─────┬──────┘
                          │ proceduralization
                          ▼
                    ┌────────────┐
                    │ L5 Procedural│ (skill, procedure)
                    └────────────┘

   ── orthogonal: L6 Associative (relations), L7 Identity (persona/state)
```

## 3. Transitions (design)

Each transition is a **trigger + policy**. No hardcoded magic numbers in the
core — policy lives in a `MemoryLifecycle` config the embedder tunes (hardware
is never ideal on paper; leave the calibration knob).

### 3.1 L0 → L1 (attention)

- Trigger: salience filter (e.g. novelty, user-directed, task-relevant).
- Policy: `attention_threshold` — only salient input enters Working.

### 3.2 L1 → L2 (turn commit)

- Trigger: end of turn / `checkpoint()` (already exists).
- Policy: `remember_exchange` writes L1 user + L2 assistant — **exists today**;
  lifecycle adds automatic promotion of the whole working set, not just the
  last pair.

### 3.3 L2 → L3 (consolidation)

- Trigger: periodic (`SleepCycle`-like, or explicit `consolidate()`).
- Policy: promote L2 docs with
  `importance = f(repetition, reinforcement, recency, attention)` above
  `consolidation_threshold`. Prune L2 below `retention_threshold` (→ archived
  or decayed, never silent drop).

### 3.4 L3 → L4 (semanticization)

- Trigger: regularity detection.
- Policy: repeated episodes with high similarity (BQ neighbor density) are
  summarized into an L4 semantic doc with `derived_from = [episode ids]`.
  The episodes stay at L3 (history preserved); the L4 doc is the generalized
  fact.

### 3.5 L4 → L5 (proceduralization)

- Trigger: "this became a procedure" (agent-level decision, HITL for
  high-stakes).
- Policy: promote a semantic recipe to L5 skill, indexed by name (ART) +
  description embedding (BQ). `index_skill` exists today — lifecycle wires it
  to the promotion path.

### 3.6 L5/L4 → L6 (association)

- Trigger: relation observed (e.g. "X causes Y", "A contradicts B").
- Policy: write an L6 relation MemoryDoc (`payload = (a, rel, b)`); see Doc 01
  §5. No automatic inference in v0.2 — associations are asserted by the agent
  (or LLM) and stored; inference is v0.3+ (needs BitNet).

## 4. Decay, reinforcement, supersede (design)

### Decay
- `importance -= decay_rate * Δt` on each lifecycle tick (configurable per
  layer: L2 decays fast, L4/L5 slow, L7 never).
- Below `decay_threshold` → `decayed` state → GC candidate (Doc 05).

### Reinforcement
- `recall()` hit → `importance += reinforce_gain` (recency boost).
- Repeated semantic matches (BQ top-k overlap) → consolidation signal (3.3).
- API: `reinforce(key, delta)` (Doc 06).

### Supersede (history-preserving update)
```text
Memory A: "user lives at X"     → state=superseded, valid_until=T
Memory B: "user moved to Y"     → state=active,    valid_from=T
```
- `supersede(a, b)` writes B as active and flips A to superseded **without
  deleting A** — the audit trail and the causal chain survive (CRDT-friendly,
  Doc 04).
- `delete` remains for HITL/security only.

## 5. Consolidation engine (design)

A `MemoryLifecycle` component (feature `lifecycle`, off by default):

```text
pub struct MemoryLifecycle {
    cfg: LifecycleConfig,        // thresholds, rates (tunable knob)
}

pub fn tick(&mut self, db: &mut Sgdb, now: u64) -> LifecycleReport {
    // 1. promote L1 → L2 (working set commit)
    // 2. consolidate L2 → L3 (importance above threshold)
    // 3. semanticize L3 → L4 (regularity: BQ neighbor density)
    // 4. decay all layers (importance -= rate*Δt)
    // 5. supersede detection (optional, agent-asserted)
    // 6. GC candidates (decayed/archived past retention)
}
```

- **Idempotent** — re-running a tick must not double-promote (guard by
  `(layer, key)` + transition log).
- **Deterministic** given the same inputs (no wall-clock inside; `now` passed
  in, like the rest of the crate).
- **Observable** — `LifecycleReport { promoted, consolidated, decayed, gc }`
  for logging/telemetry.

## 6. What ships in v0.2 (scope guard)

| Item | Scope |
|------|-------|
| `MemoryLifecycle` component + `LifecycleConfig` | ✅ core deliverable |
| L1→L2 commit, L2→L3 consolidation, decay, reinforcement | ✅ core |
| L3→L4 semanticization (regularity via BQ) | ✅ core (heuristic) |
| L4→L5 proceduralization wiring | ⚠️ agent/HITL trigger only |
| L6 associations (relation writes) | ⚠️ asserted, no inference |
| Automatic semantic inference | ❌ v0.3 (needs BitNet) |
| Memory Graph queries | ❌ v0.3 (Doc 03 §4) |

## 7. Open questions

- [ ] Where does the lifecycle tick run? (explicit `tick()` call in app loop
      vs background task — prefer explicit: no_std friendly, deterministic)
- [ ] Importance decay rates: expose as `LifecycleConfig` with sane defaults;
      document "hardware never ideal" — embedders tune.
- [ ] Does `supersede` belong in core `Sgdb` or in `MemoryLifecycle`?
      (prefer lifecycle: core stays primitive, lifecycle adds cognition)

## 8. Relationship to other docs

- Doc 01 — Memory Model: states (`active/superseded/archived/decayed`),
  `importance`, `valid_from/until`
- Doc 03 — Retrieval: recall boost = reinforcement input
- Doc 04 — Distributed: supersede chain = causal DAG for CRDT merge
- Doc 05 — Storage: decayed/archived → GC
- Doc 06 — Cognitive API: `consolidate()`, `reinforce()`, `supersede()`,
  `forget()`
