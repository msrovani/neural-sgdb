# examples/ — usage showcase

## Responsibility
Executable demonstrations of neural-sgdb: measured benchmarks and an MCP server
for AI agents to consume memory.

## bench.rs
- `cargo run --release --example bench`
- Measures: ART insert/get P50/P99 (100k keys), BQ top-5 latency (10k×1024 dims),
  recall@5 BQ vs FP32-exact, Sgdb 1k exchanges
- Zero-dep: `std::time::Instant` + percentiles by sort

## mcp_server.rs
- `cargo run --release --example mcp_server` — connectable to Claude Code/Cursor/OpenCode
- MCP (Model Context Protocol) over stdio: JSON-RPC 2.0, one message per `\n`
  line, stdout JSON-RPC ONLY (logs → stderr), legacy `2025-11-25` handshake
- Tools: `remember(text[, embedding])`, `recall(query[, embedding], k)`,
  `rag_context(query[, embedding], k)`
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

## mcp_server.rs (tool surface)
- 23 tools (22 historical + `era_report`): remember/remember_episodic/recall/
  rag_context/recall_temporal/recall_entities/feedback/diary/profile/expire_old/
  explain/reinforce/forget/associate/related_to/contradicts/supersede/conflicts/
  resolve_conflict/merge_memories/health/validate/era_report.
- `era_report` (read-only): ADR-0007 diagnostics — dims indexadas, contagem por
  dim, largura do BQ, cobertura `/L2/`, veredito empty/ok/mixed_dims + custo
  estimado de migração. A LLM gestora chama após um erro de era.

## Integration
- Depends on: `neural_sgdb` (lib), `serde_json` (dev-dep, mcp_server only)
- MCP gotchas: do not gate tools on `initialized` (Claude Code sends tools/list
  first); echo the `id` verbatim; trim `\r\n`; stdin EOF = shutdown
