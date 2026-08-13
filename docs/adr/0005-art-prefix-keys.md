# ADR-0005 — ART rejects prefix keys at the API boundary

Status: Accepted (P1-7, 2026-08-13)

## Context

The ART (Adaptive Radix Tree) index does not support keys where one is a
prefix of another — they break silently (a shorter key is not inserted, or
two children share byte 0). Three existing tests had real prefix-key bugs
(`d1` prefix of `d10…`, `k` prefix of `k2`) that corrupted results silently.
The fix options: (a) make the ART fully prefix-safe (node rework, cost +
risk), or (b) reject prefix keys explicitly before any write.

## Decision

- **Reject, don't guess.** `ArtIndex::has_prefix_conflict(&self, key) ->
  bool` walks the tree O(k) without allocating and detects if `key` is a
  prefix of an existing key or vice versa.
- Guards in `engine::put_inner` and `engine::associate` return
  `SgdbError::Invalid` BEFORE writing storage or indexes — no silent loss,
  no partial state.
- Keys must use fixed-width suffixes (`d{02}`, `k1`/`k2`, `m{w:03}`).
- Existing tests with real prefix keys were fixed to fixed-width keys.

## Consequences

- Positive: deterministic failure instead of silent corruption; cheap guard
  (O(k)); API unchanged (new predicate method, additive).
- Negative: legitimate variable-width keys like `prefix`+`prefix2` are now
  rejected — callers must adopt fixed-width suffixes; documented in AGENTS.md
  and `MIGRATIONS.md`.
- Contract impact: no format change. If a prefix-safe ART is ever needed,
  that is a new ADR.