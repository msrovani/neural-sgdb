# ADR-0003 — Side-tables, not in-record metadata

Status: Accepted (retrospective — v0.6, 2026-08)

## Context

Identity/provenance (`MemoryMeta`: memory_id, source, confidence, importance,
created_tick, parent_ids, clock_overflow) and lifecycle state (`MemoryState`)
did not exist in the original NMD1 record. Adding them in-record would break
the byte-identical contract with `neural-os-core` (ADR-0004) and require a
joint format bump + OS migration for every future metadata need.

## Decision

- NMD1 stays v1, byte-identical. New metadata lives in **side-tables**
  keyed by the storage key: `sys/state/`, `sys/validity/`, `sys/meta/`,
  `sys/version/`, `sys/rel/`, `sys/conflict/`, `sys/crdt/`.
- The engine attaches side-metadata on `get` (`attach_meta`); it travels
  with the doc on replication as a `MemoryRecord` (MDR1) unit.
- VectorClock overflow (beyond 8 fixed nodes) is NOT serialized in the
  NMD1 72B — it persists in `sys/meta/` and is re-fused on read.
- Pre-side-table records decode to `meta: None` and migrate lazily on next
  put / `set_importance` — never silent reinterpretation.

## Consequences

- Positive: NMD1/TKLV stay stable forever; new metadata is additive and
  format-compatible; OS interop is untouched; replication carries everything.
- Negative: a logical doc is spread over several storage keys — reads need
  the side-table join; `sys/*` keys must be treated as reserved by callers
  and by the OS-format tools.
- Contract impact: no format bump. Side-tables are the documented extension
  mechanism (`MIGRATIONS.md`).