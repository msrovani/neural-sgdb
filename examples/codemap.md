# examples/ — vitrine de uso

## Responsibility
Demonstrações executáveis do neural-sgdb: benchmarks medidos e um MCP server
para agentes de IA consumirem memória.

## bench.rs
- `cargo run --release --example bench`
- Mede: ART insert/get P50/P99 (100k chaves), BQ top-5 latência (10k×1024 dims),
  recall@5 BQ vs FP32-exact, Sgdb 1k exchanges
- Zero-dep: `std::time::Instant` + percentis por sort

## mcp_server.rs
- `cargo run --release --example mcp_server` — conectável a Claude Code/Cursor/OpenCode
- MCP (Model Context Protocol) sobre stdio: JSON-RPC 2.0, uma mensagem por linha `\n`,
  stdout SÓ JSON-RPC (logs → stderr), handshake legado `2025-11-25`
- Tools: `remember(text)`, `recall(query, k)`, `rag_context(query, k)`
- **Embedding de demonstração** (`demo_embed`): hash de trigramas → 256-dim
  normalizado. NÃO é modelo semântico real — trocar por embeddings próprios em produção
- Persistência: `FileStorage` via env `NEURAL_SGDB_DB` (default `sgdb_memory.db`)
- Protocolo: `initialize` (echo 2025-11-25) → `notifications/initialized` (ignora) →
  `tools/list` → `tools/call` → `ping`; desconhecidos → `-32601` (fallback client moderno)
- Requer dev-dep `serde_json` (não polui o zero-dep do lib)

## Integration
- Depende de: `neural_sgdb` (lib), `serde_json` (dev-dep, só para mcp_server)
- Gotchas MCP: não gatear tools no `initialized` (Claude Code envia tools/list antes);
  echo do `id` verbatim; trim de `\r\n`; EOF no stdin = shutdown
