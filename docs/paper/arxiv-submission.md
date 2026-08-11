# arXiv submission package — neural-sgdb

Everything needed to submit `docs/paper/neural-sgdb-telepathy.tex` to arXiv.
The actual submit happens on `arxiv.org` with your own account (see §4); this
file gives you the copy-paste metadata, the file list, and the click-path.

---

## 1. Files to upload

Upload **only** the LaTeX source:

```
neural-sgdb-telepathy.tex
```

- `thebibliography` is inline → no `.bbl` needed.
- Packages used (`geometry, amsmath, amssymb, booktabs, graphicx, hyperref,
  microtype, tikz`) are all part of arXiv's TeX Live → no `.sty` needed.
- Do **not** upload the `.pdf`, `.aux`, `.log`, `.out` — arXiv compiles the
  source itself and emails you the generated PDF for approval.
- The file compiles cleanly with pdfLaTeX (verified with tectonic 0.17): 1
  cosmetic `Overfull \hbox` (13pt) in the `fig:oversample` caption; no
  undefined references, no errors.

## 2. Metadata (copy-paste)

**Title**
```
Memories, not Packets: A Zero-Dependency no_std Memory Substrate for AI Agents
with Conflict-Preserving Peer Synchronization
```

**Authors** (add your ORCID if you have one; arXiv shows name + ORCID)
```
Marcelo Scapin Rovani
```

**Abstract** (plain text, no LaTeX)
```
AI agents operate with ephemeral context windows: memory is lost between
turns, across instances, and after crashes. We present neural-sgdb, a
persistent, transferable memory database for AI agents extracted from a
bare-metal operating system, designed under strict constraints: zero external
dependencies, dual no_std + std operation, and byte-identical on-disk formats
shared with the parent OS. The system stores memories - documents with a
cognitive layer (L0-L7), a vector clock and an identity - rather than opaque
data. Semantic recall combines binary quantization (BQ) as a coarse candidate
filter with FP32 cosine rescore, SIMD Hamming dispatch (AVX-512/AVX2/scalar), a
multi-index-hashing (MIH) accelerator, and an optional lexical BM25-style dual
path. Storage is an append-log with per-record CRC, atomic compaction, and a
byte-exact log format (TKLV/TKCK) with checkpoint fast-mount. Peers
synchronize memories through a conflict-preserving CRDT: concurrent writes are
preserved, never last-writer-wins-discarded, and an AI at the root of the
process arbitrates the preserved conflicts at read time using recency,
importance and temporal validity. We report measured performance (an
order-of-magnitude write throughput improvement via a persistent lazy storage
handle, microsecond recall, about 3x faster volume mounting under churn), a
reproducible two-instance peer-synchronization convergence demo, and the honest
costs of the model: eventual consistency, absence of global ordering, and
deferred conflict resolution.
```

**Comments**
```
12 pages, 8 figures, 2 tables (est.). Code: https://github.com/msrovani/neural-sgdb (MIT OR Apache-2.0)
```

**Categories**
- Primary: `cs.DC` (Distributed, Parallel, and Cluster Computing)
- Cross-list: `cs.AI`, `cs.IR`

> Why `cs.DC`: the novel contribution is conflict-preserving peer-to-peer
> memory synchronization + arbitration. If you prefer an agents-first framing,
> make `cs.AI` primary and cross-list `cs.DC`/`cs.IR`.

**License (in the submission form)**
- Choose **CC BY 4.0** for the preprint (the repo code stays MIT OR Apache-2.0;
  the two licenses are independent).

## 3. Submission form checklist (arxiv.org/submit)

1. Sign in (create account if needed; connect ORCID if you have one).
2. "Start New Submission".
3. Accept the license agreement → pick `CC BY 4.0`.
4. Metadata form → paste the fields above (Title, Authors, Abstract,
   Comments, Categories).
5. Upload the `.tex` file.
6. Answer the arXiv questions (author of the paper? sole author? etc.) — all
   "yes"/appropriate to your situation.
7. Submit → arXiv compiles and emails you the **generated PDF for approval**
   (a few minutes). Review it, then confirm → **published** with an arXiv ID
   (e.g. `arXiv:2608.xxxxx`).

### First-time endorsement
For your first submission in a `cs.*` category, arXiv may require
*endorsement* from an existing author in that category. The submission form
auto-offers the endorsement request (it emails the endorser and auto-approves
after the first submission).

## 4. Important (what I can and cannot do for you)

I prepared the package and verified the source compiles, but the submission
itself must be done from **your** arXiv account: it is tied to your identity,
requires your acceptance of the license, and (first time) your endorsement
flow. I can:
- adjust the `.tex` after arXiv's compile check (fix warnings it flags);
- draft the response to an arXiv admin/compile notice;
- prepare a Zenodo DOI + a GitHub release linking the arXiv ID after it
  exists.

## 5. After publication

- Save the arXiv ID + DOI.
- Add a GitHub release (`v0.5.0`) pointing to the arXiv link.
- (Optional) add the badge to the README.
