# examples/ — usage showcase

## Responsibility
Executable demonstrations of neural-sgdb: measured benchmarks, an MCP server
for AI agents to consume memory, and the agent decision / machine→machine
protocols that codify HOW to use the DB.

## bench.rs
- `cargo run --release --example bench`
- Measures: ART insert/get P50/P99 (100k keys), BQ top-5 latency (10k×1024 dims),
  recall@5 BQ vs FP32-exact, Sgdb 1k exchanges
- Zero-dep: `std::time::Instant` + percentiles by sort

## stress.rs
- `cargo run --release --example stress` (requires `file-storage`)
- 100k-op stress with reopen; verifies delete integrity

## audit.rs
- `cargo run --release --example audit` — battery 1: attack (ghost keys,
  orphan side-tables, prefix-key rejection, BQ width-lock, ...)

## mcp_client.rs — HOT TEST
- `cargo run --release --example mcp_client` — drives `mcp_server` like an IDE;
  **90/0 checks exit 0** (v1.1.6). Covers recall modes + `format=json` +
  `remember(type=)`, temporal, entities, lazy pagination (raw `rpc` for
  top-level `nextCursor`), scope, persistence across restart.

## mcp_server.rs
- `cargo run --release --example mcp_server` — connectable to Claude Code/Cursor/OpenCode
- MCP (Model Context Protocol) over stdio: JSON-RPC 2.0, one message per `\n`
  line, stdout JSON-RPC ONLY (logs → stderr), legacy `2025-11-25` handshake
- Tools (23): `remember(text[, embedding][, scope][, entities][, type])`,
  `remember_episodic`/`feedback`/`diary`/`profile`/`expire_old`,
  `recall(query[, embedding][, k][, scope][, mode][, format])`,
  `recall_temporal`/`recall_entities`, `rag_context([, rerank][, mode][,
  format])`, `explain`/`reinforce`/`forget`/`associate`/`related_to`/
  `contradicts`/`supersede`/`conflicts`/`resolve_conflict`/`merge_memories`/
  `health`/`validate`/`era_report`
- **Hits TIPADOS (v1.1.6)**: `format=json` devolve hits ESTRUTURADOS
  (`[{key,text,dist,score,path,type,dim,matched_terms,validity,rel,
  provenance}]`); `remember(type=)` declara o rótulo (seam MDM1 v6);
  `fmt_hit` unificado preserva invariantes de paginação (`- {key} | `, ` [state=`)
- **Embedder plugável** (`neural_sgdb::Embedder`): default `DemoEmbedder`
  (trigram hash → 256-dim normalized, NOT a real semantic model). Agente pode
  fornecer `embedding` no payload (mesmo modelo na gravação E na busca);
  fallback = embedder ativo (`NEURAL_SGDB_EMBEDDER`). Contract: dimensões
  diferentes não casam por design.
- Persistence: `FileStorage` via env `NEURAL_SGDB_DB` (default `sgdb_memory.db`)
- Protocol: `initialize` (echo 2025-11-25) → `notifications/initialized`
  (ignore) → `tools/list` → `tools/call` → `ping`; unknown → `-32601`
  (modern-client fallback)
- Requires dev-dep `serde_json` (does not pollute the lib's zero-dep)

## era_migration_bench.rs
- `cargo run --release --example era_migration_bench` (requires `file-storage`)
- ADR-0007: analytical benchmark of the **re-embed era migration** — scans the
  era-OLD `md/L4/` keys, reads the preserved L2 companion texts, re-embeds with
  the era-NEW model (simulated `EraEmbedder` 384-dim via the same `Embedder`
  trait), rewrites payload+bitvec (overwrite keeps `memory_id`), then
  `rebuild_indices()` to reset the BQ width.
- Reproduces the **width-lock trap** (bughunt #11): a different-dim vector
  inserted into a locked BQ is silently truncated — v1 is returned as
  distance 0 for v2. Asserts resurrection (40/40 self-recalls), loud S1 for
  era-OLD queries, the **write-side era guard** (`remember_semantic` → Invalid,
  nothing written), `era_report()` mid-state `mixed_dims` → final `ok`, lexical
  recovery net, clean `validate()`.
- Measured numbers: `BENCHMARKS.md` §Era migration (rewrite ~72µs/doc,
  rebuild ~50ms/4k docs).

## embedder_http.rs
- `cargo run --release --example embedder_http` (S2, v1.1.3) — the `Embedder`
  trait plugged into a real HTTP endpoint (raw HTTP/1.1 + existing `serde_json`
  dev-dep, zero new deps): `HttpEmbedder` + self-contained mock embedding
  server. Proves the same-model contract and the S1 guard end-to-end (4/4).

## agent_protocol.rs — DECISION PROTOCOL (v1.1.4 itens 2–6 + P1–P6)
- `cargo run --release --example agent_protocol` — codifies how the UPPER
  layer USES the DB: entity ontology, structured facts, evidence weighted by
  provenance, lifecycle, two-pass gather/register, rerank gate (P1/P5),
  write-path filter (P2), reflection grounding (P3), forgetting + bi-temporal
  (P4), multi-session checkpoint (P6). 23 auto-checks.

## memory_arena_eval.rs — MEMORY-ARENA EVAL (P7)
- `cargo run --release --example memory_arena_eval` — evaluates the UTILITY
  (not the recall) of memory: memory–agent–environment loop with interdependent
  subtasks across sessions; Config A (naive hoarder) vs Config B (protocol v2).
  Deterministic; exit 0 sse quiz ties AND SR(B) > SR(A) AND sPS(B) > sPS(A).

## two_ai_protocol.rs — MÁQUINA→MÁQUINA (v1.1.6 item 5)
- `cargo run --release --example two_ai_protocol` — the COMPLETE
  machine→machine contract end-to-end (16 auto-checks): IA-A (writer) stores
  JSON intenção, JSON NÚMERO `"42"` (the detector would say Text), non-UTF8
  binary (L3 + entity `datum/checksum`) and era vector — ALL DECLARED via
  `set_content_type`; IA-B (reader) consumes the TYPED hits (`content_type`
  declared wins, `payload_type`, `rel`, `matched_terms`), parses JSON verbatim,
  follows `rel=` to the primary, consumes raw binary by key (never
  `from_utf8_lossy`). Deterministic (InMemory, no LLM, LCG embeddings). Exit 0
  sse 16/16.

## p2p_telepathy / mesh_simulation / signed_peer (feature `p2p`)
- `cargo run --release --example p2p_telepathy --features p2p` — two-instance
  convergence via CRDT version sync + record pull
- `cargo run --release --example mesh_simulation --features p2p` — layered
  multi-AI telepathy mesh (P2-5)
- `cargo run --release --example signed_peer --features p2p` — signed-transport
  reference flow (P2-3)

## Integration
- Depends on: `neural_sgdb` (lib), `serde_json` (dev-dep, mcp_server only)
- MCP gotchas: do not gate tools on `initialized` (Claude Code sends tools/list
  first); echo the `id` verbatim; trim `\r\n`; stdin EOF = shutdown
