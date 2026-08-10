# 04 — Distributed Memory

> Status: **v0.2 design target**. Today (v0.2.0): symmetric LWW version sync
> (`CrdtMemorySync`, `Transport` trait, `UdpTransport` demo) and a fixed
> 8-node VectorClock. This doc defines distributed memory v0.2: dynamic node
> identity, per-layer CRDT policy, causal merge, provenance and full memory
> replication (not just version sync). All English per repo policy.

## 1. Honest current state (v0.2.0)

What ships today is **memory version synchronization**, not full cognitive
memory replication:

```text
wire format: [node_id u8][version u64 LE]   (9 bytes)
semantics:   LWW — higher version wins
```

The README says "memories travel between agents"; the CRDT actually exchanges
versions. Both claims are true, but they are different things. v0.2 closes
that gap.

## 2. Per-layer CRDT policy (review P3 + api.md roadmap)

LWW is right for **state**, wrong for **memory**. v0.2 defines merge policy by
layer:

| Layer | Policy | Rationale |
|-------|--------|-----------|
| L0/L1 | local-only (no sync) | volatile working memory |
| L2 | **multi-value register** | episodic: conflicts coexist as perspectives |
| L3 | **multi-value register** | episodic: history preserved |
| L4 | LWW + reindex | semantic fact; later assertion wins, BQ rebuilt |
| L5 | LWW (HITL gated) | procedural changes are deliberate |
| L6 | set-add (relations) | relations accumulate; conflicts = contradictions |
| L7 | LWW (HITL gated) | identity/state: last write wins, human-approved |

### 2.1 Multi-value register (L2/L3)

```text
Node A: "Rovani prefers X"  (source=A, confidence=.72)
Node B: "Rovani prefers Y"  (source=B, confidence=.41)
→ keep BOTH (same logical key, different values)
  recall shows both with provenance — not X-or-Y
```

- Storage: same `(layer, key)` maps to a value set; each value carries
  `source`, `confidence`, `valid_from/until`, `state`.
- NMD1 impact: the "key → single payload" contract becomes
  "key → value list" for L2/L3 → **format version bump, negotiated with OS**.

### 2.2 LWW (L4/L5/L7)

Higher version wins; loser marked `superseded` (not deleted — Doc 02 §4).

## 3. Causal merge — Memory Version DAG

Instead of "erase the loser", merge **histories**:

```text
       M1
      /  \
     /    \
   M2      M3
    \      /
     \    /
      M4
```

Each memory carries:
```text
memory_id     stable global id
parent_ids    causal parents (from supersede chain / merge)
clock         VectorClock (dynamic, Doc 01 §4)
source        creating node
confidence
timestamp
layer
state
```

Merge rule:
- If one clock dominates (∀n: ca[n] ≤ cb[n]) → causal order; apply the
  dominant value.
- If concurrent (both have wins the other lacks) → multi-value (L2/L3) or LWW
  by (clock, source) tiebreak (L4/L5).
- `parent_ids` records the merge → the DAG survives for audit/reflect.

## 4. Full memory replication (v0.2)

Evolve the wire protocol from version-sync to **delta replication**:

```text
v0.2.0 (today):   [node_id][version]                          — version sync
v0.2 (design):    [memory_id][base_clock][payload-hash]
                  + follow-up: full MemoryDoc diffs (L2/L3 registers)
```

- **Anti-entropy**: node publishes its clock per namespace; peers request
  deltas (`missing_after(clock)` → docs). Existing `Transport` trait carries
  this — no new transport, new payloads.
- **Idempotent**: applying a delta is a merge, not an overwrite — re-delivery
  is safe (CRDT property).
- **Fragment-friendly**: reuse the OS mesh's FRAG\0/FRACK\0 pattern for large
  deltas (already proven in neural-os-core).

## 5. Provenance & trust

- Every replicated memory carries `source` + `confidence` (Doc 01).
- `UdpTransport` today is **unauthenticated demo** — production requires a
  signed transport (the OS mesh signs Ed25519; the crate documents this seam).
- v0.3: peer trust (TOFU-style), confidence-weighted merge, contradiction
  surfacing (`contradicts` from Doc 03 §4 feeds CRDT conflict resolution).

## 6. Scope guard (v0.2)

| Item | Scope |
|------|-------|
| Dynamic VectorClock (serialized, compact) | ✅ core (NMD1 v0.2) |
| Multi-value register for L2/L3 | ✅ core |
| Delta replication (clock → missing docs) | ✅ core |
| Merge DAG (`parent_ids`, causal rule) | ✅ core |
| LWW tiebreak for concurrent L4/L5 | ✅ core |
| Signed production transport | ⚠️ seam documented; impl optional |
| Trust/TOFU, confidence-weighted merge | ❌ v0.3 |

## 7. Open questions

- [ ] Multi-value register encoding in NMD1 (value list vs single payload) —
      negotiate with OS
- [ ] Delta granularity: per-doc vs per-layer-snapshot (prefer per-doc, bounded
      by FRAG limits)
- [ ] Conflicting L6 relations: keep both (set-add) vs contradiction record?

## 8. Relationship to other docs

- Doc 01 — Memory Model: `memory_id`, `parent_ids`, dynamic clock, provenance
- Doc 02 — Lifecycle: supersede chain = causal DAG input
- Doc 03 — Retrieval: `contradicts` surfaces CRDT conflicts
- Doc 05 — Storage: replication deltas persisted via Storage trait
- Doc 06 — Cognitive API: `transfer()`, `merge()`
