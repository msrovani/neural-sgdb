# 05 — Storage Architecture

> Status: **v0.2 design target**. Today (v0.2.0): `Storage` trait with
> `InMemory` (RAM), `FileStorage` (CRC32 append-log, crash-safe) and
> `TickvFile` (byte-exact TKLV/TKCK interop with neural-os-core). This doc
> defines storage v0.2: explicit durability levels, WAL → checkpoint →
> immutable segments → compaction, and GC over lifecycle states. All English
> per repo policy.

## 1. Durability ≠ persistence (the review's key point)

Current `FileStorage::append` does `write_all` + `flush`:

```rust
f.write_all(&rec)?;
f.flush()?;
```

`flush` moves data to the OS, **not** to the platter. On sudden power loss,
"write returned OK" ≠ "data survived". The fix is explicit **durability
levels**, chosen by the embedder:

```text
VOLATILE        RAM only (InMemory)                     — no survival
WRITE_BUFFERED  OS buffer (write, no flush)             — survives process crash
FLUSHED         OS page cache (write + flush)           — survives process crash, not power loss
DURABLE         device (write + flush + fsync/fdatasync) — survives power loss
```

Design:

```rust
pub enum Durability { Volatile, WriteBuffered, Flushed, Durable }

pub trait Storage {
    fn durability(&self) -> Durability;   // what this backend guarantees
    fn put(&mut self, key: &[u8], val: &[u8], d: Durability) -> Result<(), SgdbError>;
    // ...
}
```

- Default: `Flushed` (fast, process-crash safe). `Durable` is opt-in per
  write — fsync is expensive; the caller decides (checkpoint = Durable,
  per-turn remember = Flushed).
- Honest reporting: `backend()` / docs must state the level; a "persistent"
  backend at `WriteBuffered` is *not* power-loss safe and must say so.

## 2. Append-only today, WAL tomorrow

### 2.1 Current (exists, correct for v0.1)

- `FileStorage`: `[klen u32][vlen u32][crc u32][key‖val]` records, tombstone
  `vlen=u32::MAX`, crash-tail truncation on open (validate CRC → truncate).
- `TickvFile`: byte-exact TKLV (512-aligned, CRC over key‖val, in-place
  `TKL\0` + `vlen=0` tombstones, EOF all-0x00/0xFF) — **interop contract**.
- Problem: append-only grows forever.

```text
put A, put A, put A, put A, put A  →  5 live copies of A on disk
```

### 2.2 v0.2 storage pipeline (design)

```text
active log (WAL)
     ↓ checkpoint
immutable segments
     ↓ compaction (merge live set, drop dead)
GC (decayed/archived past retention)
```

- **WAL**: append-only, Durable for checkpoint records, Flushed for normal
  writes. Crash recovery = replay WAL (already the FileStorage/TickvFile
  model — formalized).
- **Checkpoint**: periodic snapshot of the live index (`sys/tickv_ckpt` in
  TKLV already exists as the OS contract; FileStorage gets an equivalent).
  After a checkpoint, the WAL segment it covers is sealed.
- **Immutable segments**: sealed WAL chunks; reads consult segment map
  (newest wins). Segments are never mutated in place — compaction creates new
  segments.
- **Compaction**: rewrite the live set (state=active + archived within
  retention) into a fresh segment; old segments become GC candidates. The OS
  TickvLite `compact()` (zero-fill + rewrite live set) is the reference
  behavior — port the discipline, keep the byte format.

## 3. GC over lifecycle states

GC is driven by memory **state**, not just tombstones:

| State | GC policy |
|-------|-----------|
| active | never |
| superseded | keep (audit) until `retention_superseded` |
| archived | keep until `retention_archive` |
| decayed | candidate — removed when importance stays 0 past `gc_grace` |

- Tombstones (`vlen=0` / `TKL\0`) are compacted away during compaction (the
  live set has no dead keys).
- **Order discipline** (from the OS's own lessons, SESSION_252): data first →
  commit → only then reclaim old segments. Never reclaim before the new data
  is durable.

## 4. Interop constraints (TKLV/TKCK)

- The byte-exact TKLV/TKCK format is a **contract with neural-os-core** —
  compaction must produce the same byte format the OS reads (golden tests
  pin it: `golden_record_bytes`, `fnv1a64_known_vector`).
- `TickvFile` writes TKCK checkpoints (`checkpoint()` — TKCK record as the
  LAST record) so a crate volume can fast-mount (`try_mount_from_ckpt`, FNV-1a
  index check, per-entry CRC + stale check, ckpt-must-be-last) instead of full
  scan; `open()` falls back to `scan_volume`. GC/compaction (`compact()`)
  rewrites the live set + ckpt with an atomic rename.
- Any NMD1/TKLV layout change (Doc 01 §4 variable clock, Doc 04 §2.1 value
  lists) is a **format version bump**, negotiated with the OS, golden tests
  updated in the same commit.

## 5. Scope guard (v0.2)

| Item | Scope |
|------|-------|
| `Durability` enum + per-write level on `Storage` | ✅ core |
| `FileStorage` fsync path (`Durable`) | ✅ core |
| `TickvFile` TKCK checkpoint writes | ✅ core |
| Segment model (sealed WAL chunks) | ⚠️ staged after durability lands |
| Compaction (rewrite live set) | ⚠️ v0.2 (port OS discipline) |
| State-driven GC | ✅ core (tied to lifecycle Doc 02) |
| Automatic compaction trigger | ⚠️ explicit call first, auto later |

## 6. Open questions

- [ ] `Durable` per-write vs per-batch (fsync cost: batch checkpoints, flush
      per remember)
- [ ] Segment file naming/layout — plain appended files with a MANIFEST, or
      directory-per-segment?
- [ ] TKCK write cadence (every checkpoint, not every write)

## 7. Relationship to other docs

- Doc 01 — Memory Model: state fields drive GC
- Doc 02 — Lifecycle: decay/archive → GC input
- Doc 03 — Retrieval: indexes rebuilt from storage on open
- Doc 04 — Distributed: replication deltas persist via Storage
- Doc 06 — Cognitive API: `checkpoint()`, `gc()` exposure
