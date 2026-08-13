# Roadmap — neural-sgdb

Status: **v1.0.0 shipped (2026-08-13)** — stable API, zero deps, `no_std` +
`std`, CI gates green. This roadmap is honest about what is DONE, what is
NEXT, and what is deliberately NOT planned.

Legend: ✅ done · 🔜 next · 💤 deliberate non-goal

## v1.x maintenance (2026-08-13 — ongoing)

✅ **P0 — Hardening & tooling** (docs aligned to v1.0.0, clippy zero-warnings,
   CI gates, MCP paginate clamp, arbitration empty-scores fix, storage
   truncation sweep test, SAFETY comments on all unsafe sites, differential
   SIMD test).
✅ **P1 — Robustness** (wire encode safety, centralized limits, deterministic
   LCG property tests, honest benchmarks + `BENCHMARKS.md`, scan pagination +
   RAG size caps, ART prefix-key rejection).
✅ **P2-1 — CRDT convergence** in random topologies + conflict semantics
   documented (evidence is local; content converges).
✅ **P2-2 — Governance docs** (this file, `SECURITY.md`, `VERSIONING.md`,
   `MIGRATIONS.md`, `docs/adr/`).
🔜 **P2-3 — Security-by-backend**: real crypto (Ed25519/HMAC) at the
   transport boundary via the existing `Signer`/`Transport` seams — the core
   stays `no_std`-clean and crypto-free; plus a `health()`/`validate()`
   observability surface replacing the low-value `ready()`.
🔜 **P2-4+ — Post-P2 audit**: re-run the full bughunt oracle pass against the
   hardened tree; fuzz more codecs (`MDR1`/`CFL1`/`MDLT`/`MSNP`); CI
   hardening (cross-target checks, examples gate).

## Medium-term

🔜 **Scalability signals, not architecture**:
- Incremental BQ maintenance (the flat index is append-only; compaction
  reclaims space — revisit only on benchmark evidence).
- Oversampling auto-tuning feedback loop (raise candidate pool on collision,
  lower `k`, never hard-fail).
- Benchmark-driven ART/batch improvements — no premature redesign.

🔜 **Distributed maturity**:
- Overlay routing / partial-mesh anti-entropy (currently edge-directed pull).
- Signed transport reference impl (see P2-3).
- Conflict resolution policy bundles (deterministic arbiters outside the
  core; the core never decides semantic truth).

🔜 **Cognitive surface** (all policy-pluggable, none inside the core):
- Real embedding backfill for L3→L4 consolidation (core never generates
  embeddings).
- Repetition/similarity-density consolidation signals.
- Reinforcement scheduling (`reinforce` exists; scheduling is the layer's
  job).

## Documentation / interop

✅ Architecture v0.2 design docs (`docs/architecture/`), API contract
   (`docs/api.md`), arXiv preprint draft (`docs/paper/`).
🔜 True bidirectional format test with the OS's own reader — blocked on
   `neural-os-core` publishing its TickvLite reader as a crate.

## Deliberate non-goals (💤 — do not propose without a written ADR)

- **No FAISS/HNSW/external vector index** — BQ + FP32 rescore is the design
  (ADR-0002); revisit only on benchmark evidence.
- **No crypto in the core** — transport seam + `SignedEnvelope` (ADR-0006).
- **No LLM in the core** — arbitration/lifecycle are policy consumers.
- **No global engine statics** — seams, not globals (clock/SIMD/log).
- **No dependency on `std`** — `no_std` is a contract.
- **No silent format reinterpretation** — formats are byte-contracts
  (ADR-0004), side-tables are the extension mechanism.
- **No prefix keys in the ART** — rejected at the API boundary (ADR-0005).