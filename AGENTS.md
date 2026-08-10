# AGENTS.md — neural-sgdb

Guia para agentes de IA (OpenCode, Cursor, Windsurf, Claude Code) trabalharem
neste repo. **Leia `codemap.md` (atlas) e `docs/api.md` (contrato) antes de
editar código.**

## Repository Map

A full codemap is available at `codemap.md` in the project root.

Before working on any task, read `codemap.md` to understand:
- Project architecture and entry points
- Directory responsibilities and design patterns
- Data flow and integration points between modules

For deep work on a specific folder, also read that folder's `codemap.md`
(`src/codemap.md`, `examples/codemap.md`).

## O que é

Banco de memória para agentes de IA (**memórias, não dados**): documentos com
camada cognitiva L0–L7, recall semântico BQ + rescore FP32 (sem FAISS/HNSW),
índice ART O(k), storage plugável, CRDT sync opcional, MCP server. Extraído do
neural-os-core como projeto comunitário: **zero deps, `no_std` + `std`, MIT OR
Apache-2.0**. Interop com o OS via formatos NMD1 e TKLV byte-idênticos.

## Regras de desenvolvimento

1. **Zero dependências no lib** — só `alloc` (no_std) / `std`. Exemplos podem
   usar dev-deps (`serde_json`). Não adicionar deps ao `[dependencies]`.
2. **`no_std` é contrato** — `cargo check --no-default-features --target
   x86_64-unknown-none` deve passar SEMPRE. `deny(warnings)` no_std eleva
   dead-code a erro: use `#[allow(dead_code)]` explícito em port-parity.
3. **`f32::sqrt` NÃO existe no core** p/ esse target — use `sqrt_f32` (Newton,
   em `sgdb.rs`) ou `libm` (não neste crate).
4. **ART não suporta chave-prefixo** — chaves onde uma é prefixo de outra quebram
   silenciosamente; use sufixos de largura fixa.
5. **Formatos são contratos** — NMD1 (`memory_doc.rs`) e TKLV (`tickv.rs`) são
   byte-idênticos ao OS; NÃO alterar encode/decode/layout sem atualizar o OS.
6. **Seams, não globals** — clock via `now: u64`, SIMD via `cpu_caps()`/
   `set_cpu_caps()`, log via `sgdb_log!`. Nada de statics globais de engine.
7. **Verificação** — `cargo test` (30+1 default, 34+1 `--features p2p`) e
   `cargo check` (std + no_std) antes de commitar.

## API rápida

```rust
use neural_sgdb::{Sgdb, InMemory, FileStorage, Storage, SgdbError};
use neural_sgdb::{MemoryLayer, MemoryDoc, ArtIndex, BqFlatIndex};

// abrir (InMemory p/ testes; FileStorage p/ persistir; TickvFile p/ formato OS)
let mut db = Sgdb::open(FileStorage::open("mem.db")?)?;

// memórias
db.remember_exchange("user", "resposta")?;             // L1 + L2
db.remember_semantic("k", "texto", &emb)?;             // L4 BQ (emb: &[f32])
db.remember_fact("fato", now)?;                        // L3 timestamped
db.checkpoint()?; db.prune_working_ram()?;             // flush L0/L1 RAM

// recall (requer embeddings do caller)
let hits: Vec<Hit> = db.recall(&query_emb, 5)?;        // BQ grosso + rescore FP32
let ctx = db.rag_context(&query_emb, 3)?;              // string pronta p/ prompt
let facts = db.scan_prefix("md/L3/")?;                 // ART prefix scan
```

## Como testar os exemplos

```bash
cargo run --release --example bench        # benchmarks (ART/BQ/recall vs FP32)
cargo run --release --example mcp_server   # MCP server p/ agentes de IA
```

## Como rodar testes

```bash
cargo test                                 # 30+1 testes (InMemory/FileStorage/TickvFile)
cargo test --features p2p                  # 34+1 (inclui CRDT sync)
cargo check --no-default-features --target x86_64-unknown-none   # gate no_std
```

## Gotchas específicos deste repo

- **MCP server** (`examples/mcp_server.rs`): stdout SÓ JSON-RPC (logs → stderr),
  uma mensagem por linha `\n`, handshake `2025-11-25`, não gatear tools no
  `notifications/initialized` (Claude Code envia tools/list antes), echo do id
  verbatim, `-32601` em desconhecidos (fallback de clients modernos). Embedding
  `demo_embed` é trigramas hash — NÃO é modelo semântico real.
- **TickvFile** (`src/tickv.rs`): não escreve checkpoint (v0.1) — OS monta por
  scan completo; GC/compactação é v0.2. Records 512-alinhados, tombstone
  `vlen=0` ou `TKL\0`.
- **CRDT** (`src/crdt.rs`): rate-limit com `Option<u64>` (sentinela 0 falha em
  first sync now=0); `UdpTransport` é demo não autenticada — usar transporte
  assinado em produção.
