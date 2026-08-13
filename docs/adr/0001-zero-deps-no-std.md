# ADR-0001 — Zero dependencies + `no_std` contract

Status: Accepted (retrospective — core extraction, 2026)

## Context

`neural-sgdb` is the independent extraction of the OS memory manager
(`k_ai::sgdb`) for the community. It must run on bare-metal (the OS) and on
host (AI agents, MCP). The OS is `no_std`; dragging in a dependency tree
would break that and make the crate hard to audit. Embedding models, crypto
and transports all have host-specific needs that do not belong in a shared
core.

## Decision

- The lib depends ONLY on `alloc` (no_std) / `std`. `[dependencies]` is
  empty. Examples may use dev-deps (`serde_json`).
- `cargo check --no-default-features --target x86_64-unknown-none` is a CI
  gate that must ALWAYS pass.
- `f32::sqrt`/`f32::ln` do not exist in core for that target → local
  Newton/`ln_f32` helpers instead of `libm`.
- Host concerns (fs, UDP, SIMD auto-detect, MCP) live behind feature gates
  (`std`, `file-storage`, `simd-runtime`, `p2p`).
- Seams, not globals: clock via `now: u64`, SIMD via `cpu_caps()`/
  `set_cpu_caps()`, log via `sgdb_log!`. No global engine statics.

## Consequences

- Positive: auditable, portable, runs literally anywhere; CI catches
  `no_std` regressions immediately; `deny(warnings)` keeps dead-code honest.
- Negative: some conveniences require manual helpers (`sqrt_f32`,
  `ln_f32`); no ecosystem vector library — BQ/hamming is hand-written SIMD
  (dispatch seam + differential tests).
- Contract impact: `no_std` is a HARD gate; adding a lib dependency is a
  MAJOR decision (review it as one, `VERSIONING.md`).