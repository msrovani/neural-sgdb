# Telepathy — peer-to-peer memory sync in neural-sgdb

> **Memories, not packets.** Two instances of `Sgdb` exchange their memories
> with versioning, causality and conflict preservation — no central server.
> Run the demo: `cargo run --release --example p2p_telepathy --features p2p`.

This document explains (a) the CRDT that powers the exchange, (b) the
telepathy flow between two instances, (c) the honest cost of the model
(eventual consistency, no global order, conflict preservation), and (d) how an
AI at the root of the process arbitrates the preserved conflicts.

---

## 1. The CRDT — what actually runs

`CrdtMemorySync` is a **version-counter CRDT with conflict preservation**.
Each node keeps a minimal local state:

| Field | Role |
|---|---|
| `local_version` | monotonic counter of **own** writes |
| `own_writes` | number of independent local writes — the **base** of concurrency detection (without it, a causal successor from the same peer would become an eternal conflict; fixed in v0.3 review) |
| `node_versions` | what I know about each peer: `(node_id, version)` |
| `conflicts` | concurrent versions **preserved** — never discarded by blind LWW |
| `pending` (delta, #10) | local versions not yet delivered to peers |

The merge runs through an explicit verdict (`apply_remote_version`):

| Verdict | Meaning |
|---|---|
| `SelfPacket` | echo of my own broadcast → ignored |
| `Stale` / `Duplicate` | `v ≤` known → ignored, **no regression** |
| `Applied` | new version, no conflicting local state → adopted |
| `Conflict` | concurrent version (I wrote independently) → kept in **both** `node_versions` and `conflicts` |

In the telepathy demo you see this live: A writes `m1` and B writes `m2`
before they meet → both sides log `CONFLITO preservado (concorrente)`. Neither
write is lost.

Since v0.5 the sync is **delta-based** (#10): `record_change` accumulates
`pending` deltas and `sync` sends only what a peer hasn't seen (`send_delta`,
whose trait default falls back to `send_crdt`) — payload ∝ unseen changes, not
full history.

---

## 2. The telepathy flow — two instances converge

Each instance = `Sgdb` + `CrdtMemorySync`. The exchange is two phases:

1. **Version sync (the causal trigger).** `sync()` swaps versions over a
   `Transport` (in the demo, an in-memory pipe; in the real world,
   `UdpTransport`, TLS, serial). Each node learns the peer's `local_version`.
2. **Diff-pull (the payload).** When a node learns the peer advanced, it
   replicates the docs it's missing: `Sgdb::get(layer, key)` → `Sgdb::put(doc)`
   — idempotent, keyed by storage key. `Sgdb::put` is the public restore/import
   primitive that re-indexes any `MemoryDoc` (ART/BQ/lexical).

Demo flow and output:

```
[A] lembra m1  (versão A = 1)
[B] lembra m2  (versão B = 1)
CRDT sync: node=2 v=1 CONFLITO preservado (concorrente)
CRDT sync: node=1 v=1 CONFLITO preservado (concorrente)
[↔] ronda 1: A→B 2 doc(s), B→A 2 doc(s)
[↔] ronda 2: A→B 0 doc(s)          ← idempotente, já convergidos
[↔] ronda 3: B→A 2 doc(s)          ← B responde m3, volta para A
[✓] A conhece 6 docs, B conhece 6 docs
[B] recall da memória de A: ["eu sou a instancia A..."]  ← telepatia semântica
```

Two instances converge with **no central server**. The result of arbitration
(§4) is itself a new causal write — so resolutions propagate to every peer
through the same mechanism.

---

## 3. The honest cost

### 3.1 Eventual consistency — "they diverge temporarily"

Traditional model: read the server → everyone sees the same state at any
instant. CRDT: each node owns its local state and only exchanges when the nodes
meet (`sync` is rate-limited by `SYNC_INTERVAL` and requires connectivity).

Before round 1 in the demo, A only knows `{m1}` and B only knows `{m2}` — a
query on A answers differently than a query on B. That is the **divergence
window** between a write and the sync.

Convergence is **guaranteed** (the merge is commutative, associative and
idempotent; `apply_remote_version` never regresses — counters are monotonic),
just not **dated**: there is no "when", only "if enough sync happens". Worst
case is divergence lasting longer; never lost or resurrected data.

### 3.2 No global ordering

A central database serializes writes → a total order exists (server timestamp).
A CRDT has no global sequence: each node counts with its own `local_version`.
Two writes on different nodes have **no global before/after**.

What the CRDT *does* provide is **causal order**: a reply written after
receiving another node's memory is causally after it (knowable locally), while
two independent writes are **concurrent** — incomparable.

Consequence: "what is the latest?" has no global answer. Methods that need real
time take `now: u64` **from the caller** (wall clock), not from the CRDT — the
CRDT knows causality, not time. This is exactly the gap the higher layer fills.

### 3.3 Conflicts are preserved, not resolved

If A writes `"user prefers dark mode"` and B writes `"user prefers light mode"`
concurrently, the CRDT:
1. marks `Conflict`;
2. keeps **both** in `conflicts` and `node_versions`;
3. discards nothing.

Blind LWW (bigger version wins) requires a global "latest" — which does not
exist for concurrent versions (§3.2). And even with one, resolution is
**semantic**: which preference is true requires understanding the content, not
ordering it. The CRDT must not *lose* a version; deciding is the caller's job.

The higher layer resolves **at read time**, using the policies the crate ships:
- **`recall_weighted`** — `score = w_sem·dist + w_rec·recency + w_imp·importance`
  (recency from the `/ts/<hex>` wall-clock key, importance from the layer). The
  newer/`important` version outranks at *use*, not at *write*.
- **`recall_at` + validity window** (`sys/validity/`) — an invalidated version
  disappears from recall while history stays intact.
- **`conflicts` exposed** (multi-value) — the application (or an LLM, or an
  operator) can inspect both versions and decide, explicitly and auditably.

The philosophical difference: the traditional model resolves at **write** (the
server picks and destroys the loser — silent loss); the CRDT resolves at
**read** (everything is preserved; the policy of the moment decides). You trade
"strong by default" for "never loses + conscious decision".

---

## 4. Arbitration — an AI at the root of the process

The AI at the root of the process (in the parent OS, it runs from boot) is
precisely the "higher layer" the CRDT defers to. It arbitrates like this:

### 4.1 Harvest — the CRDT guarantees complete information

The root enumerates the conflicts (`conflicts` is public) and reads **both full
versions**: `layer`, `clock` (causality), payload, and the real recency
(`/ts/<hex>` key). Unlike the traditional model — where the server already
destroyed the loser and arbitration runs on truncated data — here nothing was
lost: analysis runs over both versions plus the causal history.

### 4.2 Context — arbitrate with episodic memory

To decide `"dark mode"` vs `"light mode"`, the root follows the causal chain to
the associated L2/L3 memories: "when did the user last complain about
brightness?", "what did the assistant answer?". That is `scan_prefix` + `get`
guided by the vector clock.

### 4.3 Signals — weigh orderable evidence

- **Recency** (`recall_weighted`, `w_rec`): newer wall-clock `ts`.
- **Importance** (`w_imp` by layer): an L4 semantic fact outweighs an L2
  episodic note.
- **Consistency with behavior**: the version that matches the episodic
  evidence.

### 4.4 Decision — four arbitration policies (cheap to expensive)

| Policy | Mechanism in the repo | When |
|---|---|---|
| **Recency-first** (deterministic, no AI) | `recall_weighted` with high `w_rec` — resolves **at read** | routine cases, cheap |
| **Validity** | `invalidate(key, now)` / `recall_at` — the loser **disappears from view**, history preserved | obsolete but not deletable |
| **Semantic merge** | root reads both → writes a new doc that unifies → `supersede(old, new)` + `set_state(Superseded)` | real content conflict ("dark by day, light at night") |
| **Escalation** | keep in `conflicts`, log, expose to a human/agent | ambiguity that needs the user |

### 4.5 Materialize and propagate

The resolution is not local: the root writes the result and calls
`record_change()` → the new doc (or invalidation) is a **new causal write** →
the next telepathy round spreads the verdict to every peer. Arbitration
converges across the network, and the chain is auditable (who arbitrated, when,
based on what).

### 4.6 Concrete example

A writes `dark mode` (v1); B writes `light mode` (v1) → `Conflict`.
1. Root reads both plus the causal L2 chain → discovers `dark mode` has a newer
   `ts` **and** the user later said "dim the screen, my eyes hurt".
2. Verdict: `light mode` is obsolete → `invalidate("md/L4/pref", ts_dark)`
   (recency + evidence).
3. The conflict is marked resolved; `dark mode` stays the only visible value in
   `recall_at`.
4. `sys/validity/` propagates via telepathy → B stops offering `light mode`.

### 4.7 Why this is stronger than the traditional model

In the central model, "arbitration" is a trigger/checkpoint on the server —
one policy, one time, over already-truncated data. Here the root arbitrates
**at any moment, with both versions, the complete causal chain and the wall
clock**, and the verdict **becomes CRDT state that converges**. It is the
difference between a database that deletes and a brain that decides with the
whole history on the table.

---

## 5. Running it

```bash
# two Sgdb instances exchange memories (CRDT version sync + diff-pull)
cargo run --release --example p2p_telepathy --features p2p

# benchmarks / stress / MCP server
cargo run --release --example bench
cargo run --release --example stress
cargo run --release --example mcp_server
```

Implementation: `src/crdt.rs`, `examples/p2p_telepathy.rs`, `Sgdb::put` in
`src/sgdb.rs`. Related: `recall_weighted`, `recall_at`, `sys/validity/`
(§3.3/§4), delta sync (#10).
