# ADR-0006 — No crypto in the core

Status: Accepted (retrospective — v1.0 trust seam; hardens to P2-3, 2026)

## Context

The core is `no_std`, zero-dependency (ADR-0001). Real cryptography
(Ed25519/HMAC/TLS) would drag in dependencies, bloat the bare-metal build,
and duplicate what a host's transport already does. But memory transfer
between nodes (CRDT sync, telepathy) must be authenticable: a hostile relay
must not impersonate an author or rewrite clocks.

## Decision

- The core ships a **trust seam, not crypto**: `Peer` (node_id + identity +
  auth status + trust level), bounded `TrustStore`, `Signer` trait with
  `HmacFnvSigner` (keyed FNV-1a) — **explicitly a non-cryptographic demo**.
- `SignedEnvelope` is the authenticable envelope format for signed
  transports; `UdpTransport` is documented as an unauthenticated demo.
- Production hosts plug a real signer/transport (Ed25519/HMAC/TLS) at the
  boundary; the core stays clean. **P2-3 (2026-08-13)**: the reference
  signed-transport flow is exercised end-to-end in `trust.rs`
  (`signed_transport_reference_flow`, p2p) — sign → envelope → verify →
  reject tampered payload → reject untrusted peer. `Sgdb::health()` reports
  observable state (backend, counts, open conflicts) and `Sgdb::validate()`
  runs integrity checks, giving hosts the observability to act on
  authentication/trust failures.

## Consequences

- Positive: core stays portable/auditable; authentication strength is the
  host's choice; no crypto audit burden on the bare-metal build.
- Negative: out-of-the-box `UdpTransport` sync is NOT secure — users must
  wire a signed transport before exposing sync to untrusted networks.
- Contract impact: none to formats. The seam is API-stable (`Signer`,
  `Transport`, `SignedEnvelope`, `TrustStore`).