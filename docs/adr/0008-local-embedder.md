# ADR-0008 — Default retrieval is lexical; embeddings are a host-side era, never in the core

Status: Accepted (2026-08-20; replaces the Proposed “local embedder as product default”)

## Context

The core never generates embeddings (ADR-0002): `remember_semantic`/`recall`
take `&[f32]` from the caller. ADR-0007 makes the model an era invariant per
DB file. ADR-0001 forbids linking an inference runtime into the lib.

The shipped `DemoEmbedder` (character-trigram hash, 256-dim) is useful in
tests and for same-word recall, but it is **not a semantic model**. Making it
the undocumented MCP default (`mode=semantic` + auto-embed) trained agents to
expect synonym recall and then fail. Lexical/entity paths already exist and
are the recovery net after any era switch (ADR-0007).

True semantic recall still matters — but only when the host supplies a real
vector (agent `embedding=` or a local model server). Cloud embedding APIs leak
memory text; the surfaces we care about (Cursor/Windows, OS, future WASM)
need the model on the same machine if they want an era at all.

## Decision

1. **Honest default (MCP):** `recall` / `rag_context` without a caller-supplied
   `embedding` use **`mode=lexical`** (BM25 over L2/L3). `mode=semantic` and
   `hybrid` are opt-in and require a real vector source: the tool argument
   `embedding` and/or a host `Embedder` that is **not** `DemoEmbedder`.
2. **`DemoEmbedder` is not the product default.** It remains for unit tests,
   benches, `ensure_doctrine` seeding when the host has no model, and explicit
   `NEURAL_SGDB_EMBEDDER=demo`. A live MCP must not imply “semantic” when the
   vector is a trigram hash.
3. **Write path:** MCP `remember(text=)` without a real vector does **not**
   open a fake BQ era. Persist text that lexical/entities can retrieve
   (`remember_fact` / companion L2, declared `type=text|json|code`).
   `remember_semantic` (L4 BQ) stays in the lib as-is (`&[f32]`); the host
   calls it only with a real era or an explicit demo.
4. **Optional local semantics:** a thin `Embedder` client that talks HTTP to a
   **local** model server (ollama `/api/embed`, llama.cpp `llama-server
   --embedding`, or any loopback endpoint). Same instance on write and query
   (P4). Era rules stay ADR-0007 (`era_report`, rebuild, no mixed dims in a
   live BQ). Inference never links into this crate; in-process runtimes
   (candle/ort/ggml) belong in a separate crate if ever needed.

Reference: `src/embedder.rs` (seam), `examples/embedder_http.rs` (HTTP proof),
MCP `recall.mode` in `examples/mcp_server.rs` (implementation follows this ADR).

## Consequences

- Positive: the default path matches what actually works without a model
  (lexical + entities + scope); ADR-0001/0002/0007 stay intact; a local
  server remains the only honest way to get cosine recall; agents stop
  treating trigram hamming as meaning.
- Negative / cost: MCP default `mode` changes from `semantic` → `lexical`
  (MINOR, documented contract); hosts that relied on demo same-word
  “semantic” must pass `mode=semantic` + `NEURAL_SGDB_EMBEDDER=demo` or a
  real `embedding`; one extra process if the host wants an era; same-dim
  model swaps stay silent until `model_id` exists (**MDM1 v7** — v6 is
  `content_type`).
- Contract impact: none on NMD1/TKLV/`no_std`. MCP behavior change is MINOR
  when implemented (`VERSIONING.md`). This ADR authorizes that change; it
  does not by itself bump the crate version.

## Other uses of the memory (not just IDE dev)

The local-embedder decision is the enabler of the non-IDE surfaces; this ADR
records them so future work does not re-derive them. Landscape research
2026-08 (see `docs/future-horizons.md`):

- **Linux daemon / fleet**: one shared `Sgdb` serving a fleet of agents, each
  isolated in its own `scope`; the CRDT mesh syncs nodes (`docs/telepathy.md`).
- **Windows local-first assistant**: fully offline memory (ollama/llama.cpp +
  local embedder); data never leaves the machine.
- **Browser WASM**: the `no_std` core compiles to wasm32; `IndexedDB`/OPFS as a
  `Storage` backend — the session's memory persists across page loads
  (mnem and Kurumi already ship this pattern).
- **Browser extension**: memory of what the user read/navigated, recalled via
  `recall_temporal`/`recall_entities`.
- **Edge / embedded**: `no_std` + zero deps → runs on minimal Linux devices;
  NMD1/TKLV keep the OS interop contract.

Where the AI enters stays at the seams (`Embedder`, entity extraction, the
decision protocol) — the DB is the memory of the brain, not the brain. The
"memory, not data" contract is what these uses consume: docs carry
importance/confidence/validity/provenance; ADD-only with supersede (no silent
overwrite); conflicts preserved; the queries are "what was the state at T?",
"does this contradict what I know?", "how much do I trust this?" — none of
which a relational row answers.