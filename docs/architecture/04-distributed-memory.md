# 04 — Distributed Memory

> Status: **current (v1.1.11)** — CRDT sync, full record replication, and
> anti-entropy ship behind feature `p2p`. **implemented** = code + tests;
> **remaining** = honest gap. All English per repo policy.

## 1. What ships today

Full **cognitive memory replication**, not just version gossip:

```text
Replication unit: MemoryRecord (MDR1)
  = NMD1 doc + MemoryState + validity window + MemoryMeta

Wire: MemoryDelta (MDLT) / MemorySnapshot (MSNP) / ConflictRecord (CFL1)
Transport: Transport trait + UdpTransport (demo, unauthenticated)
Sync: CrdtMemorySync — version sync + delta pull + merge_remote
```

Memories travel with state, validity, provenance and lineage — not bare
payloads.

## 2. Per-layer merge policy (implemented)

| Layer | Policy | Verdict |
|-------|--------|---------|
| L0/L1 | local-only | `Rejected` — never accept remote |
| L2/L3 | multi-value friendly | Applied / history preserved |
| L4 | causal LWW + history | concurrent → Conflict |
| L5/L7 | controlled LWW | HITL expectation |
| L6 | relations set-add | edges accumulate |

`MergePolicy` table enforced in `merge_remote` — layer semantics matter.

## 3. VectorClock and causal identity (implemented)

- NMD1 clock: 72B fixed (interop).
- Runtime: dynamic nodes + overflow registry (v0.6).
- Per-version identity: `version_id` (MDM1 v2), `sys/version/` reverse index,
  `lineage()` walk, `supersede` links versions.
- One logical write = one causal version (`put_companion` for L4+L2 pairs).

## 4. Anti-entropy (v0.7+ — implemented)

Each sync round:

1. **Announce** full known clock per node.
2. **Pull** missing causal range `known+1..=v` via `keys_for_clock`.
3. **Merge** records through `merge_remote` (Conflict preserved, never blind LWW).

Tested: triangle mesh, partition/rejoin, relay through intermediate node,
durable `CrdtState` (`state()`/`restore()`).

**Remaining:** overlay routing / partial-mesh spanning — today reconciliation
is edge-directed per round (see ROADMAP.md).

## 5. Conflict model (v0.9+ — implemented)

- `ConflictRecord` persisted in `sys/conflict/` (deterministic id).
- Candidates carry parallel MDR1 evidence.
- `resolve_conflict` — import winner via evidence; `dismiss_conflict` — cleanup.
- Core **never** decides semantic truth — `ArbitrationPolicy` trait for
  deterministic policies outside LLM (v1.0).

## 6. Security and transport (partial)

| Item | Status |
|------|--------|
| `SignedEnvelope` + `Signer`/`TrustStore` seams | implemented (reference flow) |
| `examples/signed_peer.rs` | implemented (where to plug Ed25519) |
| Crypto in core | deliberate non-goal (ADR-0006) |
| `UdpTransport` | demo only — unauthenticated |
| Production signed transport | **remaining** — embedder implements |

## 7. Honest limitations

- **`node_versions` gossip** — partial in directed topologies; does not
  necessarily converge (content does).
- **`ConflictRecord`** — local merge evidence; not a replicated MDR1 unit.
- **Multi-value in one storage key** — conflicts preserved as evidence, not
  co-located value lists inside NMD1 (see Doc 01 §8).
- **Rate limit:** `Option<u64>` — 0 sentinel fails first sync at now=0 (fixed).

## 8. Remaining gaps

- Overlay / gossip routing for sparse meshes.
- Reference Ed25519 (or other) signed transport implementation in-tree.
- Trust/TOFU and confidence-weighted merge policies (upper layer).

## 9. Relationship to other docs

- Doc 01 — Memory Model: clock, meta, replication unit
- Doc 02 — Lifecycle: supersede chain feeds merge DAG
- Doc 03 — Retrieval: `contradicts` surfaces conflict adjacency
- Doc 05 — Storage: deltas persist via `Storage` trait
- Doc 06 — Cognitive API: `export_record` / `import_record` / MCP (no transfer tool — use p2p examples)
