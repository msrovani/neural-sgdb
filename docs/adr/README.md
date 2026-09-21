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
| 0007 | Embedding model = era invariant | Model swaps are era transitions; migrate via re-embed + BQ rebuild; write-side dim guard; **`model_id` in MDM1 v7** (`mixed_models`) |
| 0008 | Default retrieval is lexical; embeddings are host-side | MCP default = lexical; `DemoEmbedder` is not the product path; optional local HTTP embedder; never in the core |
| 0009 | Index snapshot on open; metrics-gated auto-adapt | Storage = truth; TKCK-like index snapshot; measure `open_ms`/`doc_count`/reopens; auto persist+fast-mount when budget breaks — **not** mid-query; impl ROADMAP Next |
| 0010 | Harness commit_run / anti-patterns / obsolescence | No `Deprecated` state; `arch/rev/*` + supersede; `mom/anti-pattern`; facade `commit_run` (**v1.1.19**); null-scoping `ScopeDims` (**v1.1.20**) |
| 0011 | Derived-index integrity oracle | `index_fingerprint` = `fp(open) == fp(rebuild)`; ids/órfãos do BQ/floats FORA; `validate` checa `corpus_mean` com tolerância relativa; ADR-0009 §3 ganha o validador (**v1.1.21**) || 0012 | Adaptive recall: pay for the boundary only when it is ambiguous | `recall_adaptive` escala `1→4→8→16` enquanto o gap `k`/`k+1` couber em `SCORE_TIE_MARGIN`; `RecallProbe` distingue "store acabou" de "pool faminto"; opt-in (muda ordem) — e o bench MEDE que o default ainda escala demais (**v1.1.22**) |

## Retrospective history

The core was extracted from `neural-os-core` (`k_ai::sgdb`, ADR-0063 in the
OS project) — that project's ADR numbering is independent of this one.
OS-side ADRs referenced in the code: ADR-0060 (memory layers), ADR-0063
(portable core extraction), ADR-0081 (CRDT memory sync).