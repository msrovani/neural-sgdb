# ADR-0008 — Local embedder = thin client of a local model server, never in the core

Status: Proposed

## Context

The core never generates embeddings (ADR-0002): `remember_semantic`/`recall`
take `&[f32]` from the caller and the `Embedder` trait (`src/embedder.rs`) is
the seam; the shipped default `DemoEmbedder` is a character-trigram hash —
good for keyword-ish recall, NOT a semantic model. The P4 contract requires
the SAME model on write and query, and the S1 guard rejects queries whose
dimension matches none of `indexed_dims`; ADR-0007 fixes the model as an era
invariant per DB file.

Real-world friction: to actually USE semantic recall, an agent must supply
embeddings from a real model. Cloud APIs (OpenAI & co.) leak data and depend
on the network; the offline scenarios we target (personal assistant on
Windows/Linux, WASM/browser, edge devices) need embeddings computed on the
same machine, under local control. ADR-0001 forbids dependencies in the lib
and `no_std` is a hard gate — an inference runtime (ONNX/candle/ggml) can
never be linked into the core. The HTTP seam is already proven:
`examples/embedder_http.rs` implements `Embedder` over raw HTTP/1.1 against a
mock embedding server.

## Decision

- A **local embedder is a thin client implementing `Embedder` that talks to a
  local model server process** — ollama (`/api/embed`, e.g.
  `nomic-embed-text`), llama.cpp (`llama-server --embedding --model *.gguf`),
  or any local HTTP embedding endpoint. The model runs as a separate process,
  never linked into the crate.
- The reference implementation is an example mirroring `embedder_http.rs`
  (`examples/local_embedder.rs`): locate the local server (configurable base
  URL + model name), embed via HTTP, validate dimension/non-finite response,
  normalize the output to the cosine 0..1 scale, cache by text hash (optional,
  the trait allows the caller to persist embeddings).
- The SAME `Embedder` instance feeds both `remember_semantic` and `recall` —
  that enforces P4 by construction. The DB pins the era via `indexed_dims`
  (ADR-0007): switching model/dim in a live DB is a loud `SgdbError::Invalid`,
  and migration is the documented re-embed-from-`/L2/` + `rebuild_indices()`
  path. `era_report()` shows the cost before migrating.
- Fully in-process inference (candle/ort/ggml) is explicitly OUT of the core:
  if ever needed it lives in a separate crate (`nsgdb-embed`) implementing
  the same `Embedder` trait.

## Consequences

- Positive: offline, privacy-first semantic recall for every target surface
  (Windows/Linux assistant, browser WASM, edge); core keeps ADR-0001
  (zero-dep + `no_std`); the seam is now proven twice (mock HTTP + real local
  server); deterministic embeddings keep persisted vectors stable.
- Negative / cost: the runtime needs a local model server + model download
  (user-side concern); one more moving part in the stack; an HTTP hop adds
  per-embed latency (mitigated by text-hash caching); same-dim model swaps
  stay silent until `model_id` lands (ADR-0007's optional MDM1 v6).
- Contract impact: none — no lib API, binary format, or feature change; the
  deliverable is a new example and a documented environment (`NEURAL_SGDB_EMBEDDER`).