# CLAUDE.md — neural-sgdb

Guia para Claude Code trabalhar neste repo. **Leia `AGENTS.md` (regras + API
rápida), `codemap.md` (atlas) e `docs/api.md` (contrato) antes de editar código.**

## Repository Map

A full codemap is available at `codemap.md` in the project root.

Before working on any task, read `codemap.md` to understand:
- Project architecture and entry points
- Directory responsibilities and design patterns
- Data flow and integration points between modules

For deep work on a specific folder, also read that folder's `codemap.md`
(`src/codemap.md`, `examples/codemap.md`).

## Resumo executivo

Banco de memória para agentes de IA (**memórias, não dados**): camadas L0–L7,
recall semântico BQ + FP32 (sem FAISS/HNSW), índice ART O(k), storage plugável,
CRDT p2p, MCP server. Zero deps, `no_std` + `std`, MIT OR Apache-2.0. Interop
com o neural-os-core via NMD1 e TKLV byte-idênticos.

## Regras essenciais (detalhes no AGENTS.md)

1. **Zero deps no `[dependencies]`** — só `alloc`/`std`; dev-deps só p/ examples
2. **`cargo check --no-default-features --target x86_64-unknown-none`** deve passar
3. **`f32::sqrt` não existe no core** p/ x86_64-unknown-none — use `sqrt_f32`
4. **Formatos NMD1/TKLV são contratos byte-idênticos ao OS** — não alterar layout
5. **Verificação**: `cargo test` (+ `--features p2p`) e ambos `cargo check`

## Comandos

```bash
cargo test                                   # 30+1 testes
cargo test --features p2p                    # 34+1
cargo run --release --example bench          # benchmarks
cargo run --release --example mcp_server     # MCP server
cargo check --no-default-features --target x86_64-unknown-none
```
