# Versioning Policy — neural-sgdb

Versioned with [SemVer 2.0.0](https://semver.org/). Releases follow
[Keep a Changelog](https://keepachangelog.com/) in `CHANGELOG.md`.

## What the version number means here

`MAJOR.MINOR.PATCH` on a `1.x` line:

| Bump | Trigger | Examples |
|------|---------|----------|
| **MAJOR** | Public API break, format break (NMD1/TKLV/TKCK byte layout), removal of a feature, `no_std` contract break, or `[dependencies]` change | dropping a public method, changing a codec's byte layout, making `std` mandatory |
| **MINOR** | Additive public API, new opt-in feature, new format version that is backward-decodable, behavior refinement within a documented contract | new `recall_*` variant, new side-table `sys/*` (format-compatible), new optional feature gate |
| **PATCH** | Bug fix, hardening, doc alignment, performance work — no API or behavior-contract change | bughunt fixes, clamp/overflow fixes, recovery determinism |

**Format versioning is separate from crate versioning.** Binary layouts are
contracts pinned by golden byte tests (`golden_nmd1_bytes`,
`golden_record_bytes`, `fnv1a64_known_vector`). Any layout change MUST bump
the format's own version marker AND update the golden tests **in the same
commit** (see `docs/api.md` §Format versioning and `MIGRATIONS.md`).

## Rules enforced by CI / development

- **`no_std` is a contract**: `cargo check --no-default-features --target
  x86_64-unknown-none` must always pass. `deny(warnings)` in no_std elevates
  dead-code to error — use explicit `#[allow(dead_code)]` on port-parity.
- **Zero dependencies in the lib** — only `alloc` (no_std) / `std`.
  Adding a lib dependency is a MAJOR decision (review it as one).
- **Gates**: clippy `--all-targets --all-features -- -D warnings` and
  `cargo doc --no-deps` with `RUSTDOCFLAGS="-D warnings"`. `cargo fmt` is
  deliberately NOT gated (the repo is not rustfmt-clean).
- **Test matrix** (each release):
  `cargo test` / `cargo test --features p2p` /
  `cargo test --no-default-features` + the no_std target check above.
- **Features** (`Cargo.toml`): `std`, `file-storage`, `simd-runtime`, `p2p`
  (opt-in). Default = `["std","file-storage","simd-runtime"]`. New
  capabilities must be **additive** and feature-gated; never change default
  feature semantics in a PATCH.

## Release process

1. Verify the full matrix (above) on the commit to tag.
2. Update `CHANGELOG.md` (move `[Unreleased]` → new version heading).
3. Bump `Cargo.toml` `version`.
4. Tag `v<MAJOR.MINOR.PATCH>`; push tag + branch.
5. If the release touches a binary format, update `docs/api.md` format
   changelog, `MIGRATIONS.md`, and the golden tests in the SAME commit.

## Pre-1.0 note (history)

Versions 0.x were feature lines (v0.6 = provenance/replication blocks,
v0.7 = anti-entropy + per-version identity, v0.8 = lifecycle + L6 relations,
v0.9 = cognitive API + conflict model). The public `1.0.0` (2026-08-13)
stabilized: docs aligned, clippy zero-warnings, CI gates, and the contract
above. Changes between 0.x lines that touched formats documented explicit
migrations in `MIGRATIONS.md` and never silently reinterpreted old bytes
(e.g. MDM1 v1→v2 decodes v1 with `version_id = memory_id`).

## Current line (1.1.x)

Crate version in `Cargo.toml` is **1.1.0**; additive feature releases
**v1.1.2–v1.1.6** are documented in `CHANGELOG.md` and `docs/api.md` without
bumping the crate MINOR until the maintainer tags a release. Architecture
docs in `docs/architecture/` describe the **shipped** system at v1.1.6, not
a future design target.