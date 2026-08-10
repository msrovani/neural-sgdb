# Repository Atlas: neural-sgdb

## Project Responsibility
Banco de memória persistente e transferível para agentes de IA — **memórias, não
dados**. Núcleo extraído do neural-os-core (`k_ai::sgdb`, ADR-0063) como projeto
comunitário independente: zero dependências, dual-mode `no_std` + `std`, licença
MIT OR Apache-2.0. 8 camadas de memória L0–L7, recall semântico BQ + rescore
FP32, índice ART O(k), storage plugável, CRDT sync (feature `p2p`), MCP server.

## System Entry Points
- `src/lib.rs`: crate root — `cfg_attr(not(feature="std"), no_std)`, re-exports
  públicos (`Sgdb`, `Storage`, `MemoryDoc`, `ArtIndex`, `BqFlatIndex`, `TickvFile`, …)
- `Cargo.toml`: features `default=["std"]`, `p2p=["std"]`; dev-dep `serde_json` (só examples)
- `docs/api.md`: contrato de API + mapa de migração (Storage trait, seams, aceite)
- `examples/bench.rs`: benchmarks medidos (ART P50/P99, BQ, recall vs FP32)
- `examples/mcp_server.rs`: MCP server para agentes de IA (JSON-RPC 2.0 stdio)

## Directory Map (Aggregated)
| Directory | Responsibility Summary | Detailed Map |
|-----------|------------------------|--------------|
| `src/` | Núcleo do banco de memória: MemoryDoc NMD1, ART, BQ + Hamming SIMD, engine, facade Sgdb, Storage trait, codec TKLV, CRDT p2p | [View Map](src/codemap.md) |
| `examples/` | Vitrine: bench (latência P50/P99, recall vs FP32) + MCP server (remember/recall/rag_context) | [View Map](examples/codemap.md) |
| `docs/` | Contrato de API (`api.md`), release notes | — |

## Format Contracts (interop com neural-os-core)
- **NMD1** (`src/memory_doc.rs`): documento de memória — magic `NMD1`, layer u8,
  klen u32 LE, key, VectorClock 72B, plen u32 LE, payload, bitvec flag
- **TKLV/TKCK** (`src/tickv.rs`): storage — header 16B (`TKLV` + klen/vlen/crc32
  u32 LE), CRC32 IEEE sobre key‖val, body pad 16, record pad 512, tombstone
  `TKL\0`/`vlen=0`, ckpt `TKCK` + FNV-1a 64

## Seams (injetáveis — substituem o kernel de origem)
- Clock: métodos recebem `now: u64` (não há relógio interno)
- SIMD: `cpu_caps()` (std auto-detect) / `set_cpu_caps()` (no_std)
- Log: `sgdb_log!` (no-op no_std, eprintln std)
- Storage: `Storage` trait (put/get/scan_prefix/delete) — `InMemory`, `FileStorage`, `TickvFile`

## Quick Start (para agentes de IA)
```rust
use neural_sgdb::{Sgdb, InMemory};
let mut db = Sgdb::open(InMemory::new())?;
db.remember_exchange("qual o clima?", "sol, 24 graus")?;
let hits = db.recall(&query_emb, 5)?; // query_emb: &[f32] do caller
```
- `recall` exige embeddings do caller (o crate não gera — demo: trigramas no MCP)
- Persistência: `FileStorage::open(path)` ou `TickvFile::open(path)` (formato OS)
