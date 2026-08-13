# ADR-0004 — Formats are byte-contracts with the OS

Status: Accepted (retrospective — core extraction, 2026)

## Context

`neural-sgdb` interoperates with `neural-os-core` at the byte level: a
volume written by the OS is read by this crate and vice versa. This is an
extraction acceptance requirement. NMD1 (document) and TKLV/TKCK (storage)
must never drift silently — OS-written deletes must not resurrect, OS
checkpoints must fast-mount, values must be checksummed end-to-end.

## Decision

- NMD1 (`memory_doc.rs`) and TKLV (`tickv.rs`) are byte-identical to the
  OS. Change encode/decode/layout ONLY together with the OS.
- Layouts are pinned by **golden byte tests** (`golden_nmd1_bytes`,
  `golden_record_bytes`, `fnv1a64_known_vector`); any layout change bumps a
  version marker and updates the golden tests in the same commit.
- Cross-direction tests: OS→crate (`scan_volume` re-parse incl. tombstones
  and corruption) and crate→OS (512-aligned byte-exact writes). A true
  bidirectional run against the OS's own reader is deferred until the OS
  publishes its TickvLite reader as a crate.
- Storage CRC covers key‖val, not just the key — bit rot in values is
  detected.
- Record/overwrite invalidation is in-place (`magic[3]='V'→0`, TKL\0) and
  `scan_volume` skips tombstones before CRC (OS-written deletes never
  resurrect).

## Consequences

- Positive: cross-system memory transfer actually works; golden tests keep
  both sides honest; CRDT/format decisions inherit byte-level rigor.
- Negative: any format evolution is a coordinated two-repo change; the crate
  cannot unilaterally modernize NMD1/TKLV. Metadata growth is forced into
  side-tables (ADR-0003).
- Contract impact: MAJOR (format break) requires the full checklist in
  `MIGRATIONS.md`.