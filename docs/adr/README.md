# Architecture Decision Records — neural-sgdb

ADRs capture **why** the code is shaped this way, so future agents and
maintainers can change a decision without re-litigating it. Recorded
retroactively for decisions already made (marked *retrospective*); new
decisions MUST add an ADR in the same commit as the code.

## How to add one

1. Copy `0000-template.md` to `NNNN-slug.md` (next number).
2. Fill Status (Accepted / Superseded by ADR-XXXX / Rejected / Deprecated),
   Context, Decision, Consequences. Keep it short — the code is the detail,
   the ADR is the reasoning.
3. Reference the ADR from the code (top doc-comment) and from this index.
4. A MAJOR format/API decision goes in the same commit as its ADR (see
   `VERSIONING.md`).

## Index

| ADR | Title | Decision |
|-----|-------|----------|
| 0001 | Zero dependencies + `no_std` contract | The lib depends only on `alloc`/`std`; `no_std` is a hard gate |
| 0002 | BQ + FP32 rescore instead of FAISS/HNSW | O(k) ART + binary-quantized flat index, zero deps |
| 0003 | Side-tables, not in-record metadata | NMD1 stays v1 byte-identical; new metadata in `sys/*` |
| 0004 | Formats are byte-contracts with the OS | NMD1/TKLV pinned by golden tests; never diverge |
| 0005 | ART rejects prefix keys at the API boundary | No silent loss — `has_prefix_conflict` guard |
| 0006 | No crypto in the core | Transport seam + `SignedEnvelope`; production plugs a real signer |

## Retrospective history

The core was extracted from `neural-os-core` (`k_ai::sgdb`, ADR-0063 in the
OS project) — that project's ADR numbering is independent of this one.
OS-side ADRs referenced in the code: ADR-0060 (memory layers), ADR-0063
(portable core extraction), ADR-0081 (CRDT memory sync).