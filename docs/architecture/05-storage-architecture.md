# 05 — Storage Architecture

> Status: **current (v1.1.9)** — `Storage` trait + three backends + durability
> levels ship in production code. **implemented** = code + tests; **remaining**
> = honest gap. All English per repo policy.

## 1. Durability levels (implemented)

Explicit levels — embedder chooses cost vs safety:

```text
VOLATILE        InMemory                    — no survival
WRITE_BUFFERED  write, no flush             — process crash may lose
FLUSHED         write + flush (default)       — survives process crash
DURABLE         write + flush + fsync         — survives power loss
```

`FileStorage` and `TickvFile` expose `sync_durable()`. Checkpoint records
should use `Durable`; per-turn remember can stay `Flushed`.

## 2. Storage trait (implemented)

```rust
pub trait Storage {
    fn put(&mut self, key: &[u8], val: &[u8]) -> Result<(), SgdbError>;
    fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, SgdbError>;
    fn scan_prefix(&mut self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, SgdbError>;
    fn delete(&mut self, key: &[u8]) -> Result<(), SgdbError>;
}
```

Semantics: append-log, idempotent put, tombstone delete, CRC recovery.
Implement over flash/NVMe by providing these four methods.

### Backends

| Backend | Role |
|---------|------|
| `InMemory` | tests, prototyping |
| `FileStorage` | CRC32 append-log, lazy persistent handle (~38×), atomic compaction |
| `TickvFile` | byte-exact TKLV/TKCK — OS-readable volume |

## 3. Append-log and recovery (implemented)

**FileStorage record:** `[klen][vlen][crc32][key‖val]`, tombstone `vlen=MAX`.

**TickvFile record:** 512-aligned TKLV header, CRC over key‖val, in-place
invalidation (`TKL\0` / magic[3]=0) before append.

Recovery: validate CRC → truncate corrupt tail; `scan_volume` skips tombstones
and checkpoint record; last-wins for duplicate keys.

Fault-injection tests: truncated tail, bit rot, reopen parity across backends.

## 4. Checkpoint and fast-mount (implemented)

**TickvFile:**
- `checkpoint()` writes TKCK as **last** record.
- `open()` tries `try_mount_from_ckpt` (FNV-1a index + per-entry CRC + stale
  check); falls back to full `scan_volume`.
- `compact()` rewrites live set + ckpt + atomic rename.

**FileStorage:** compaction drops tombstones and duplicate keys; must drop
lazy append handle before rename (bughunt: inode reuse).

## 5. Index rebuild discipline (implemented)

ART, BQ, lexical, and entity indexes are **derived state**:

```text
open → scan md/ + sys/meta/ + sys/rel/ → rebuild_indices
```

Any Storage impl that preserves bytes can remount indices deterministically.
`Sgdb::validate()` cross-checks counts and side-table integrity.

## 6. Physical delete vs logical state

| Operation | Effect |
|-----------|--------|
| `set_state` / `supersede` / `forget` | logical — side-table `sys/state/` |
| `invalidate` / `expire_old` | temporal — `sys/validity/` |
| `delete` | physical tombstone + index removal + side-table cleanup |

BQ orphans after delete are inert at recall (skipped) until `reclaim_bq_orphans`.

## 7. Remaining gaps

- **Sealed WAL segments** — single append file today; segment manifest model
  is medium-term (compaction discipline already ports OS TickvLite behavior).
- **Automatic compaction trigger** — explicit `compact()` / `checkpoint()`;
  no background GC thread (no_std-friendly).
- **State-driven retention GC** — decay/archive mark state; no automatic
  purge of aged superseded records by retention policy yet.

## 8. Interop constraints (immutable)

- NMD1 / TKLV / TKCK byte layouts — golden tests pin them.
- Any layout change = format version bump + OS negotiation + MIGRATIONS.md +
  same-commit golden update (ADR-0004).

## 9. Relationship to other docs

- Doc 01 — Memory Model: what gets stored
- Doc 02 — Lifecycle: state drives logical retention
- Doc 04 — Distributed: side-table bytes replicate via Storage
- Doc 06 — Cognitive API: `checkpoint`, `health`, `validate`
