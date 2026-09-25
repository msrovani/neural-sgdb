# ADR-0009 — Index snapshot on open; metrics-gated auto-adapt (not mid-query)

Status: Accepted *(design contract — §3 fast-mount IDX1 **implementado em
v1.1.29**: wire `IDX1`, `open_with_snapshot`, seam MCP
`NEURAL_SGDB_INDEX_SNAPSHOT`; §4 métricas de open desde v1.1.21; §5
auto-persist metrics-gated **implementado em v1.2.0**: o host persiste
quando os writes desde o último persist atingem
`MAX(8, open_rebuild_ms_last)` — falha log-only; snapshot paginado IDX2
(v1.2.0) elimina o teto MAX_VLEN)*

## Context

`Sgdb::open` rebuilds derived indexes (ART, BQ, lexical, entity map, …) from
storage on every mount. Measured cost is roughly linear in live docs
(~16 ms / 500 docs → ~1.6 s @ 100k). `TickvFile` already fast-mounts the
**volume** via TKCK (`src/tickv.rs`); ART/BQ have no equivalent snapshot.

Two distinct pain modes (agent/MCP analysis, 2026-09-16):

1. **Large DB** — `open_rebuild_ms` becomes agent-visible (hundreds of ms to
   seconds). Order of magnitude: ≤2k docs ≈ noise; ~10k ≈ noticeable cold
   start; ≥50–100k ≈ “large” for this crate’s rebuild path.
2. **Many session reopens** — cost is `open_ms × opens`. A small DB with a
   host that reopens per chat/reload still burns wall time (stress harness:
   reopen loops remain expensive even at ~100 docs).

Separately, recall@k residual after ADC-lite (v1.1.17) is a **different**
problem (candidate filter quality). This ADR does **not** authorize FAISS/
HNSW in-core (ADR-0002). Snapshot speeds **open**, not cosine quality.

The product question: can the system **measure** when rebuild hurts and
**auto-adjust** toward snapshots “a quente” as the DB grows?

## Decision

### 1. Storage remains the source of truth

Derived indexes (ART/BQ/lexical/…) may be **snapshotted** for fast mount, but
any corrupt/stale/missing snapshot **MUST** fall back to full
`rebuild_indices_from_storage`. Never treat a snapshot as authoritative over
NMD1/side-tables (same discipline as TKCK vs full `scan_volume`).

### 2. Snapshot is an open-path optimization, not a recall-path switch

- Gain appears on the **next** `Sgdb::open` (or explicit remount), not mid-
  `recall` / mid-`remember`.
- While a DB is open, indexes already live in RAM; “hot adapt” means
  **start persisting** snapshots and **use them on subsequent opens**, not
  swapping ranking engines in flight.

### 3. Shape (when implemented)

Mirror TKCK intent for derived indexes:

- Persist a versioned snapshot blob (side-table / companion record; exact
  wire TBD in the implementing commit + golden tests — ADR-0003/0004).
- Invalidate or bump generation on mutating writes that change index
  membership (`put`/`delete`/`rebuild`/era rebuild).
- `open`: try fast-mount → validate → else full rebuild.
- Optional `checkpoint` / compact hook writes a fresh snapshot.

NMD1/TKLV byte layouts stay untouched unless a future ADR explicitly bumps a
format marker (ADR-0004).

### 4. Measurement contract (how we know we need it)

Expose (core and/or host) at least:

| Metric | Role |
|--------|------|
| `doc_count` / `bq_len` | size proxy (already on `health`) |
| `open_rebuild_ms` | wall time of index rebuild (or whole `open`) |
| `opens` / `opens_per_hour` | host-counted reopens (MCP/launcher) |
| `open_ms_budget` | policy input (e.g. 100 ms agent / 500 ms batch) |

Honest benches: `open_ms(N)`, `open_ms(N)/N`, and `K × open_ms` reopen loops
(correlated with real session churn, not only microbench noise).

**Plausible enable thresholds** (tunable, not sacred):

- `doc_count ≥ 10_000`, **or**
- `open_rebuild_ms > open_ms_budget`, **or**
- `opens_per_window × open_ms` exceeds a host budget.

### 5. Auto-adapt policy (“a quente” as the DB grows)

Allowed and desirable:

```text
growth / slow open observed
  → policy flips snapshot mode to auto|on
  → next checkpoint/compact persists index snapshot
  → subsequent opens fast-mount when fresh
  → stale/corrupt → rebuild (always safe)
```

Policy may live in:

- **core** — small heuristic from `doc_count` + last `open_rebuild_ms`, or
- **host** — `NEURAL_SGDB_INDEX_SNAPSHOT=off|auto|always` (preferred seam;
  keeps no_std core free of env globals — ADR seams discipline).

Forbidden:

- Silent dependence on snapshot without rebuild fallback.
- Using snapshot enablement as an excuse to add FAISS/HNSW or break zero-deps
  (ADR-0001/0002).
- Claiming mid-query “engine swap” as the adapt mechanism.

### 6. Relation to ADC / ANN residual

- **In scope of other work:** oversample, ADC-lite extensions, existing ANN
  seams (`recall_ann_ivf` / Hnsw-lite) measured honestly.
- **Out of scope here:** external vector libs; changing BQ-as-filter design
  without a new ADR.

## Consequences

- Positive: clear definition of “large DB” / “many reopens”; measurable
  trigger; auto-adapt path that matches TKCK precedent; agents/MCP can budget
  cold start without guessing; storage truth preserved.
- Negative / cost: implementing commit is large (codec, invalidation, fuzz,
  reopen/compact/era tests). Snapshot writes add IO on checkpoint. Wrong
  invalidation → silent recall bugs until fallback is proven.
- Contract impact (when coded): likely **MINOR** if additive APIs + new
  side-table/snapshot record with explicit version; **MAJOR** only if NMD1/
  TKLV change (must not). Feature-gate optional (`std` / file-storage) is
  fine. Document thresholds in `BENCHMARKS.md` + `health` / `nsgdb://session`.

## Non-goals

- Replacing FileStorage single-writer discipline for multi-MCP (ops, not
  snapshot).
- Making shared MCP DB behave like CRDT telepathy (ADR/docs telepathy;
  `telepathy_two_db`).
- Shipping code in the same commit as this ADR — this record **accepts the
  design**; ROADMAP tracks implementation.

## References

- Precedent: TKCK fast-mount (`src/tickv.rs`, ADR-0004 interop).
- Cost notes: AI-user audit / `AGENTS.md` gaps; `BENCHMARKS.md`; stress reopen.
- Related: ADR-0002 (BQ filter), ADR-0003 (side-tables), ADR-0001 (zero deps).
