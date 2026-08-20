# Roadmap — neural-sgdb

Status: **v1.1.x maintenance line shipped (2026-08-19, features até v1.1.6)** —
stable API, zero deps, `no_std` + `std`, CI gates green. Crate version mantém
**1.1.0**; feature releases v1.1.2–v1.1.6 são commits/unreleased no
`CHANGELOG.md`. This roadmap is honest about what is DONE, what is NEXT, and
what is deliberately NOT planned.

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
✅ **P2-3 — Security-by-backend** (delivered 2026-08-13): the reference
   signed-transport flow (sign → envelope → verify → reject tampered/untrusted)
   is proven end-to-end in `trust.rs` via the existing `Signer`/`Transport`/
   `TrustStore`/`SignedEnvelope` seams — the core stays `no_std`-clean and
   crypto-free (ADR-0006); `ready()` replaced by `Sgdb::health()` (observable
   state) and `Sgdb::validate()` (aggregated integrity checks); MCP server
   exposes `health`/`validate` tools; `examples/signed_peer.rs` is the
   "where to plug Ed25519" runnable.
✅ **P2-4 — Post-P2 audit**: bughunt oracle #1–#11 re-run green; `src/
   wire_fuzz.rs` fuzzes all 8 wire types (`MDR1`/`CFL1`/`MDLT`/`MSNP`/NMD1/
   MDM1/`SignedEnvelope`/`CrdtState`) with one deterministic LCG harness.
✅ **P2-5 — Layered multi-AI telepathy mesh** (delivered 2026-08-13): 8 agents
   in 5 cognitive layers on a directed mesh; an external AI writes at L1, each
   layer answers via its own recall, telepathy propagates L1→L5, and a deep
   layer recovers by semantic recall the exact memory that entered at L1 —
   `layered_ai_telepathy_mesh` test + `examples/mesh_simulation.rs`.
✅ **v1.1.2 — Guinea-pig plan** (guinea-pig user): `resolve_known_key`,
   `recall_weighted` por importância de DOC, `associate_checked`, trait
   `Embedder` + `DemoEmbedder`, `sqrt_f32` pub(crate). Hot test 49/49.
✅ **v1.1.3 — Co-author ergonomics** (S1–S5): recall LOUD em dim mismatch
   (`indexed_embedding_dims`), `examples/embedder_http.rs`, `get_texts_batch`,
   `BqFlatIndex::retain` + `reclaim_bq_orphans`, MCP recall lazy-paginado.
   Hot test 60/60.
✅ **v1.1.4 — Memory landscape** (itens 1–10): ADD-only contrato,
   `remember_episodic`/`feedback`/`diary`/`profile`/`expire_old`, scoping
   multi-agente (MDM1 v4), retrieval modes (semantic/lexical/hybrid),
   `recall_temporal`, entidades 1-hop (MDM1 v5). Hot test 84/0; MCP 22 tools.
✅ **v1.1.5 — Era guard (ADR-0007, write-side)**: `remember_semantic` fora da
   era → `Invalid`; `Sgdb::era_report()` (src/era.rs) + MCP `era_report`
   (tool 23). Hot test 81/0.
✅ **v1.1.6 — Hits TIPADOS p/ consumidor máquina** (itens 1–5): `src/ctype.rs`
   (ContentType/RecallPath), `Hit` + path/content_type/payload_type/score/
   matched_terms/validity/rel, projeção prosa só Text/Json/Code, seam de write
   `set_content_type` (MDM1 v6, declared wins), MCP `format=json` +
   `remember(type=)` + `rag_context rerank=/mode=`, `rag_context_reranked`
   (ancoragem lexical, `anchors=N`), `examples/two_ai_protocol.rs` (16/16).
   Hot test 90/0; matrix 229/181/275.
🔜 **CI hardening** (remaining tail of P2-4): cross-target checks and an
   examples gate in `.github/workflows/ci.yml`.

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

✅ Architecture docs aligned to v1.1.6 (`docs/architecture/`), API contract
   (`docs/api.md`), implementation status (`docs/implementation-status.md`),
   arXiv preprint draft (`docs/paper/`).
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