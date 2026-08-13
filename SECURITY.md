# Security Policy — neural-sgdb

## Reporting a vulnerability

Please report security issues privately — **not** in a public issue.

- GitHub: use the private vulnerability reporting flow on
  `https://github.com/msrovani/neural-sgdb/security/advisories`
  (recommended).
- Email: open an advisory first; if you must use email, describe the issue
  without reproducing it in public channels.

Expect an acknowledgement within **5 business days** and a fix/triage plan
as soon as the impact is understood. Security fixes ship as patch releases
(see [VERSIONING.md](VERSIONING.md)).

## What is in scope

- **Memory integrity**: persisted bytes (NMD1/TKLV/FileStorage) must never
  corrupt or resurrect on decode/recovery/compact/rebuild. Bit rot in values
  is detected (CRC covers key‖val).
- **Denial of service**: malformed/malicious wire or storage input must fail
  cleanly, never panic, never allocate unboundedly. Bounds-checked decoders
  are part of the contract (P1-2); `scan_prefix_page`/`rag_context` have
  hard caps.
- **CRDT correctness**: no silent data loss, no phantom versions, conflicts
  preserved (never blind LWW). A hostile relay must not be able to rewrite
  an author's clock as its own.
- **Memory/safety**: all unsafe sites are isolated in
  `src/hamming_dispatch.rs` (SIMD) and `src/art.rs` (index node unions),
  each with a SAFETY comment; `unsafe` is not a design tool in this repo.

## What is NOT in scope (by design)

This crate is **`no_std`-clean, zero-dependency, and ships NO cryptography in
the core**. Consequently:

- **Wire security** (integrity/authentication/confidentiality) is the
  TRANSPORT's job. `UdpTransport` is an explicit, documented, unauthenticated
  demo. Production deployments must plug a signed/encrypted transport at the
  `Transport` seam; `SignedEnvelope` is the authenticable envelope format —
  the core does not implement crypto (P2-3 / ADR-0006).
- **Embedding security** is the caller's job. The MCP demo embedding is a
  trigram hash, not a semantic model, and must never be used for untrusted
  input classification.
- **Arbitration is policy-pluggable**: the core detects/preserves conflicts
  and never decides semantic truth. A pluggable `ArbitrationPolicy` (v1.0)
  runs outside the core — its security is the host's responsibility.

## Trust model

- Storage and transport are treated as **untrusted** input: everything that
  crosses a byte boundary goes through bounds-checked decoders.
- Peer identity is only as strong as the transport: with an unauthenticated
  transport, any node can announce any clock. Version-adoption rules
  (`local_version` counts only own writes) limit, but do not eliminate, the
  blast radius of a hostile relay.
- `MemoryMeta.memory_id` is stable per key and author-derived on replication;
  re-created docs get a NEW id (watermark counter) so deleted memories do
  not resurrect.

## Hardening history

Post-P0 (2026-08-13): CRC over key‖val; in-place tombstone skip in
`scan_volume`; prefix-key rejection (ART, P1-7); MCP paginate overflow clamp;
arbitration empty-scores fix; SAFETY comments on all 9 unsafe sites; no_std
gate + clippy/doc gates in CI. P1 series (2026-08-13): wire encode safety
(bounds-checked, no truncating casts), centralized limits, deterministic LCG
property tests, honest benchmarks, scan pagination + RAG size caps,
ART prefix-key guards. P2 series: CRDT convergence in random topologies
(P2-1), governance docs (P2-2), crypto-by-backend + health API (P2-3).