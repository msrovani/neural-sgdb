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
- Tools: `remember(text)`, `recall(query, k)`, `rag_context(query, k)`
- **Demo embedding** (`demo_embed`): trigram hash → 256-dim normalized. NOT a
  real semantic model — swap in your own embeddings for production
- Persistence: `FileStorage` via env `NEURAL_SGDB_DB` (default `sgdb_memory.db`)
- Protocol: `initialize` (echo 2025-11-25) → `notifications/initialized`
  (ignore) → `tools/list` → `tools/call` → `ping`; unknown → `-32601`
  (modern-client fallback)
- Requires dev-dep `serde_json` (does not pollute the lib's zero-dep)

## Integration
- Depends on: `neural_sgdb` (lib), `serde_json` (dev-dep, mcp_server only)
- MCP gotchas: do not gate tools on `initialized` (Claude Code sends tools/list
  first); echo the `id` verbatim; trim `\r\n`; stdin EOF = shutdown
