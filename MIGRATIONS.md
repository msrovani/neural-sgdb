# Migration Guide — neural-sgdb

This file describes how binary formats and on-disk state evolve between
releases. It complements `docs/api.md` §Format versioning and
`VERSIONING.md`.

## Golden rule

**Never silently reinterpret old bytes.** Every format change must be: (1)
backward-decodable (old bytes still decode), (2) covered by a golden test
updated in the same commit, and (3) documented here. When a field is absent
in an old version, decode to a defined default — never guess.

## Format registry

| Format | Version | Encode/decode | Golden test | Lives in |
|--------|---------|---------------|-------------|----------|
| NMD1 | v1 (stable) | `MemoryDoc` | `golden_nmd1_bytes` | `src/memory_doc.rs` |
| MDM1 | v6 | side-table meta codec | golden/decode tests (via MemoryRecord) | `src/memory_doc.rs` |
| TKLV / TKCK | v1 (stable) | `TickvFile` | `golden_record_bytes` | `src/tickv.rs` |
| FNV-1a 64 | — | checksum | `fnv1a64_known_vector` | `src/fnv1a64.rs` (or engine) |
| CRDT state | "CRDT" | `CrdtState` | bounds-checked decode | `src/crdt.rs` |
| MDR1 | v1 | `MemoryRecord` | bounds-checked decode | `src/memory_doc.rs` |
| CFL1 | v1 | `ConflictRecord` | bounds-checked decode | `src/conflict.rs` |
| MDLT / MSNP | v1 | `MemoryDelta` / `MemorySnapshot` | bounds-checked decode | `src/crdt.rs` |

## NMD1 (document) — stays v1, byte-identical to neural-os-core

The OS interop contract. NMD1 NEVER changes without a joint bump in
`neural-os-core`. New metadata is added via **side-tables**, not in-record:

| Side-table | Purpose | Since |
|------------|---------|-------|
| `sys/state/` | `MemoryState` (Active/Superseded/Archived/Invalidated/Decayed) | v0.2 |
| `sys/validity/` | temporal `from|until u64le` window (invalidate-not-delete) | v0.2 |
| `sys/meta/` | `MemoryMeta` (memory_id, source, confidence, importance, created_tick, parent_ids, clock_overflow; + scope [v4], entities [v5], content_type [v6]) | v0.6 |
| `sys/version/` | per-version identity reverse index | v0.7 |
| `sys/rel/` | L6 relations (`<kind>/<a>#<b>`) + derived ART fwd/rev | v0.8 |
| `sys/conflict/` | `ConflictRecord` (MDR1 evidence per candidate) | v0.9 |
| `sys/crdt/` | durable `CrdtState` (opt-in) | v0.7 |

## Known migrations

### MDM1 v1 → v2 (v0.7)

Adds `version_id` (per-version identity). v1 records decode with
`version_id = memory_id` — **explicit migration, never silent**. NMD1 and
TKLV/TKCK unchanged.

### MDM1 v2 → v3 (v0.9)

Adds `last_reinforced` (importance reinforcement timestamp). v1/v2 records
decode with `last_reinforced = 0`. See `memory_doc.rs` decode discipline:
every flag advances `off` even on the 0 branch.

### MDM1 v3 → v4 (v1.1.4 item 7)

Adds `scope` (multi-tenant isolation string). v1–v3 records decode with
`scope = ""`. The decode MUST advance `off` after `last_reinforced` even when
the scope is present — `last_reinforced` was the last field of v3; without the
advance, v4 reads the scope from the wrong offset (real bug, fixed).

### MDM1 v4 → v5 (v1.1.4 item 10)

Adds `entities` (declared 1-hop entity strings). v1–v4 records decode with an
empty list. Same discipline: the scope was the last field of v4; every field
advances `off`.

### MDM1 v5 → v6 (v1.1.6 item 2)

Adds `content_type` (stable type label: `text`/`json`/`code`/`embedding`/
`binary`, `None` = not declared). v1–v5 records decode with `None`. NMD1 and
TKLV/TKCK are untouched — the label lives only in `sys/meta/`, travels on the
MDR1 (via `meta_for_import`), and never reinterpretes old bytes.

### Pre-v0.6 records → identity (lazy)

Records written before provenance existed return `meta: None` until re-put
or `set_importance`/`set_confidence`. On replication, identity is derived
from the AUTHOR's clock (never `self.node_id`).

### Records written before the prefix-key guard (P1-7, 2026-08-13)

ART does not support prefix keys; `engine::put`/`associate` now reject a
prefix-key with `SgdbError::Invalid` BEFORE writing. Data written earlier
under a prefix-key layout is unreachable by `get` — keys must use
fixed-width suffixes (e.g. `d{02}`, `k1`/`k2`). No bytes are corrupted;
only re-keying recovers the entries.

## Versioning a format (checklist)

1. Decide compat: backward-decodable (MINOR) vs breaking (MAJOR) — see
   `VERSIONING.md`.
2. Bump the format's version marker in the codec.
3. Update the golden tests **in the same commit** (`golden_nmd1_bytes`,
   `golden_record_bytes`, or add a new one) plus decode tests for the old
   version (v1/v2/v3/v4/v5 intermediate decodes — every field must advance
   `off`, see the MDM1 v4/v5/v6 bugfixes).
4. Update `docs/api.md` §Format versioning and this file.
5. If the OS shares the format, coordinate the bump in `neural-os-core` —
   never ship a divergent layout (bughunt parity is a contract).