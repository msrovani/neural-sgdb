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
6. **Não é gate:** `extension/` / Chrome Web Store estão **estacionados**.
   O crate entrega memória **agêntica** (MCP 4 tools, doutrina,
   `agent_protocol` / `two_ai_protocol` / `memory_arena_eval`). Republicar
   Store **não** faz parte do release.

## Pre-1.0 note (history)

Versions 0.x were feature lines (v0.6 = provenance/replication blocks,
v0.7 = anti-entropy + per-version identity, v0.8 = lifecycle + L6 relations,
v0.9 = cognitive API + conflict model). The public `1.0.0` (2026-08-13)
stabilized: docs aligned, clippy zero-warnings, CI gates, and the contract
above. Changes between 0.x lines that touched formats documented explicit
migrations in `MIGRATIONS.md` and never silently reinterpreted old bytes
(e.g. MDM1 v1→v2 decodes v1 with `version_id = memory_id`).

## Current line (1.1.x)

Crate version in `Cargo.toml` is **1.1.24**; additive feature releases
**v1.1.2–v1.1.24** are documented in `CHANGELOG.md` and `docs/api.md` without
a MAJOR bump. Architecture docs in `docs/architecture/` describe the
**shipped** system at **v1.1.24** (lexical-first MCP, ADR-0008, ADR-0010
`commit_run`, null-scoping `ScopeDims`, ADR-0011 oracle, ADR-0012 adaptive
recall). Typed hits
landed in v1.1.6; agent doctrine in v1.1.8; cognitive metadata in v1.1.10; host governance + micro-ganhos in v1.1.11; security hardening 11→1 em v1.1.12; v1.1.13 tentou Store/extensão — **track browser estacionado** (não é produto); v1.1.15 = MDM1 v7 + TTL/GC + ANN; **v1.1.17** = ADC-lite dual-path + state-first; **v1.1.18** = telepathy 2-DB FileStorage harness; **v1.1.19** = harness `commit_run` / anti-patterns (ADR-0010); **v1.1.20** = null-scoping honra `ScopeDims` + pool `recall_*_dims` + consolidate herda dims; **v1.1.21** = oráculo `index_fingerprint` + invariante de `corpus_mean` + métricas de `open` + erro de alias útil (ADR-0011); **v1.1.22** = `src/math.rs` consolidado + `recall_adaptive` / `RecallProbe` (ADR-0012); **v1.1.23** = revisão de documentação + fix do `enum` anunciado em `health` (schema divergindo do handler); **v1.1.24** = `ScopeDims` autoritativo (o `scope` legado vira espelho de `user`, write-through; ADR-0013) + ledger de negativos `sys/negative/` (ADR-0014); **v1.1.25** = `payload_type` honra a CAMADA (L3 de prosa deixa de ser reportado como `Embedding(len/4)`);  **v1.1.26** = descoberta de escopo com procedência + `recall` honra as dims que o schema anuncia (ADR-0015); **v1.1.27** = `TypeScores` sobrepostos derivados na leitura (ADR-0016); **v1.1.28** = vocabulário único prosa/JSON + tool `decide` (𝒥(S,𝒬), 5º tool) + `recall_candidates` com sinais decompostos + validate tipado + k=0 erro (ADR-0017); **v1.1.29** = fast-mount IDX1 do índice derivado (ADR-0009 §3/§5, seam `NEURAL_SGDB_INDEX_SNAPSHOT`) + RaBitQ avaliado e REJEITADO com A/B medido.; **v1.2.0** = batch write `memories[]` + dedup guard `if_exists` + decide inline + preview `max_payload_bytes` + stale candidates report + IDX2 (snapshot paginado, teto 16 MiB) + auto-persist metrics-gated (ADR-0009 §5 completo). (o `scope` legado vira espelho de `user`, write-through; ADR-0013) + ledger de negativos `sys/negative/` (ADR-0014).

## Host connectors (`connectors/`)

Adapters for OpenClaw / Hermes / similar hosts live **outside** the crate
SemVer line. They speak MCP to `examples/mcp_server.rs` and must not change
`src/`, NMD1, or TKLV. Ship them under `CHANGELOG.md` `[Unreleased]` (or a
dated note) **without** bumping `Cargo.toml` unless the core/MCP contract
also changes. Connector behavior is versioned by git commit + the README in
`connectors/`.