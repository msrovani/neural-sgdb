# ADR-0010 — Harness Engineering: commit_run, anti-patterns, obsolescence

Status: Accepted *(implemented in v1.1.19 — `src/harness.rs` + MCP
`curate` ops `commit_run` / `deprecate_run`)*

## Context

Autonomous / harness agents need three memory behaviours that the crate already
*almost* supports, but without a crisp product contract:

1. **Versioning & scoping hygiene** — avoid context pollution when architecture
   evolves (branch/project/run isolation; discard obsolete solutions).
2. **First-class anti-patterns** — retrieve not only what to do, but what
   **not** to repeat (linter scars, failed approaches, fixed bugs).
3. **Task-driven flush** — at the end of an atomic task, consolidate episodic
   working noise into durable semantic/procedural memory without hoarding.

Shipped substrate (v1.1.18) already provides: `ScopeDims{user,agent,app,run}`,
`memory_id`/`version_id`/`parent_ids`, `MemoryState`
(`Active|Superseded|Archived|Invalidated|Decayed`), `supersede`/`forget`/
`feedback`/`Contradicts`, `remember_episodic`, `consolidate_recurrences`,
`MemoryLifecycle::tick`, TTL/GC, `set_event`/`close_event`, host scheduler.
Doctrine unchanged: **core does not decide**; ADD-only; conflict at retrieval
time; NMD1/TKLV byte-identical (ADR-0003/0004).

The gap is not a new vector engine — it is (a) **conventions** for polarity /
architecture revision, and (b) a **facade verb** that orchestrates existing
primitives for end-of-task hygiene.

## Decision

### 1. No new `MemoryState::Deprecated`

Obsolescence is already expressible:

| Intent | Mechanism |
|--------|-----------|
| Replaced by a better fact | `supersede(old, new)` → old `Superseded` |
| Soft remove from default recall | `forget` → `Archived` |
| Time window closed | `set_validity` / `expire_old` → `Invalidated` |
| Importance collapsed | `decay_importance` → `Decayed` |

A fifth named state would churn `sys/state` / wire consumers without semantic
gain. Agents **MUST** treat `Superseded` + lineage as the harness “deprecated
solution” path.

### 2. Architecture revision = entities + supersede (not a meta field yet)

When the architecture evolves, writers:

1. `remember` the new constraint/decision with entity `arch/rev/<n>` (and MOM
   roles as today: `mom/constraint`, `mom/decision`, …).
2. `supersede` (or `forget`) memories that applied only under `arch/rev/<n-1>`.

Optional later (only if entities prove insufficient for ranking): additive
MDM1 field `arch_rev` — **requires a future ADR + golden tests**. This ADR
does **not** authorize MDM1 v8.

Branch/project isolation uses existing seams:

- legacy `scope` string (`project/<repo>`, `project/<repo>/branch/<name>`), and/or
- `ScopeDims.app` / `ScopeDims.run` (task id in `run`).

Do **not** add a fifth scope dimension for “branch” while strings suffice.

### 3. Anti-patterns = first-class **memories**, not downrank-only

`feedback(positive=false)` reweights an existing fact — it does **not** create
a durable “do not repeat X” lesson. Harness writers **MUST** store anti-patterns
as normal memories with canonical entities:

| Entity | Role |
|--------|------|
| `mom/anti-pattern` | MOM role: negative lesson (identical string on write + `recall_entities`) |
| `avoid/<slug>` | Stable id of the avoided behaviour (e.g. `avoid/f32-sqrt-in-core`) |

Payload: verbatim scar (error text, bad approach, fix). Prefer `type=json` when
structured. Link to the positive procedure via L6 when useful:

- Prefer documenting **`Contradicts`** / **`Supports`** for now.
- **`RelationKind::Avoids`** is **deferred** (optional follow-up ADR) unless
  graph queries need a distinct edge kind.

Retrieval discipline (host/agent, not core auto-policy):

```text
gather: recall_entities(mom/constraint, mom/anti-pattern)  [scoped]
     ∪  recall lexical/hybrid for the task
then act; only then remember
```

Do **not** add a `ContentType::AntiPattern` — content type is payload format
(text/json/code/…), not cognitive role (ADR cognitive API / v1.1.6).

Optional later: MDM1 `polarity` (`neutral|prefer|avoid`) for ranking filters —
**out of scope** until measured need; would be a separate ADR.

### 4. `commit_run` — orchestration facade, not a new memory engine

Authorize a public verb (Rust + MCP `curate` op) that **composes** existing
APIs for end-of-task flush. Sketch (names may refine in the implementing
commit):

```text
commit_run(filter: ScopeFilter /* typically run=… */, plan: CommitRunPlan)
  → CommitRunReport
```

`CommitRunPlan` (upper layer supplies content — core never LLM-summarises):

| Field | Effect |
|-------|--------|
| `facts` / `anti_patterns` | `remember_*` under the run’s dims (L3/L4 as chosen) |
| `supersede` pairs | `supersede(old, new)` |
| `archive_remaining_episodic` | `forget` / `Archived` on leftover `md/L2/…` matching filter |
| `ttl_episodic_ms` | `set_ttl` on episodic leftovers |
| `close_event_key` | `close_event` if the host opened a timeline event |
| `audit` | optional `audit_checkpoint(now)` |

`CommitRunReport`: counts of written / superseded / archived / ttl’d / closed;
storage keys of new facts (full `md/L…` keys for follow-ups).

Also authorize bulk hygiene helpers that only loop existing `set_state`/
`set_ttl` (no new states):

- `deprecate_run(filter)` — archive or TTL episodic under a closed run
- optional `consolidate_recurrences_scoped(filter)` — same algorithm as
  `consolidate_recurrences`, restricted by `ScopeFilter`

Forbidden inside `commit_run`:

- Silent overwrite of Active facts (ADD-only / supersede discipline).
- Embedding generation in core (ADR-0008).
- Auto-forget without plan flags (staleness remains host-curated).

### 5. Layer promotion remains explicit

- L2 → L3: recurrence consolidate and/or agent-supplied facts in `commit_run`.
- L3 → L4: host supplies embedding (or lexical L3 stays).
- L4 → L5: still HITL / agent (`transfer_to`) — not automatic in this ADR.

`MemoryLifecycle::tick` and `host_scheduler` remain valid background paths;
`commit_run` is the **foreground** harness flush at task boundary.

### 6. Documentation & agent packet

When implemented, update in the same commit(s):

- `docs/architecture/02-memory-lifecycle.md` + `06-cognitive-api.md`
- `docs/agent-self-program.md` (MOM table: `mom/anti-pattern`; commit_run ritual)
- `.cursor/skills/nsgdb-full-usage/` + MCP `curate` enum / doctrine resource if needed
- `examples/agent_protocol.rs` — harness flush auto-check
- `implementation-status.md` matrix row

Until conventions land in agent hosts, **§2–§3 remain the correct agent
behaviour** on the MCP surface (`remember` + entities + `supersede` +
`scope_run` + `commit_run`).

**Shipped (v1.1.19):** `Sgdb::commit_run` / `deprecate_run` /
`consolidate_recurrences_scoped` / `remember_episodic_scoped` in
`src/harness.rs`; MCP `curate` ops `commit_run` and `deprecate_run`.

**Hardening (v1.1.20):** null-scoping honra `ScopeDims` (não só
`MemoryMeta.scope`); `consolidate` herda dims do run; `recall_*_dims`
não depende do pool global filtrado.

Prompts corrigidos (origem externa → contrato real):
[`docs/harness-prompts.md`](../harness-prompts.md).

## Consequences

- Positive: harness agents get a clear obsolescence model without schema churn;
  anti-patterns become retrievable 1-hop evidence; end-of-task hygiene becomes
  one verb instead of ad-hoc curate chains; doctrine (core does not decide)
  preserved.
- Negative / cost: agents must learn entity strings (`mom/anti-pattern`,
  `arch/rev/*`, `avoid/*`); deferred `Avoids`/`polarity` may tempt premature
  MDM1 bumps (resist — entities first).
- Contract impact: **MINOR** — additive APIs + MCP `curate` op;
  no NMD1/TKLV change; no new `MemoryState`; MDM1 unchanged under this ADR.
  Hot test pin (`MCP_CONTRACT_VERSION`) bumps only if listed tools/args change
  in a versioned way (same discipline as prior MCP bumps).

## Non-goals

- Sleep-cycle LLM region rewriting (Auto-Dreamer-style) inside the crate.
- Automatic mid-session engine swap or FAISS/HNSW (ADR-0002/0009).
- Shared “always-in-context” memory blocks (Letta-style) as core types.
- Inferring anti-patterns from text (entities remain caller-supplied).

## References

- Diagnosis session (Harness Engineering × neural-sgdb), 2026-09-18
- `docs/architecture/02-memory-lifecycle.md`, `06-cognitive-api.md`
- `docs/agent-self-program.md` (MOM roles)
- ADR-0003 (side-tables), ADR-0008 (lexical-first / host embedder)
- `examples/host_scheduler.rs`, `examples/agent_protocol.rs`
