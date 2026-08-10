# src/ — núcleo do neural-sgdb

## Responsibility
Camada de banco de memória cognitiva (SGDB) dual-mode `no_std` + `std`, zero
dependências externas (só `alloc`). Modelo de domínio: **memórias, não dados** —
documentos com camada L0–L7, vector clock, recall semântico por BQ + rescore
FP32, índice ART O(k), storage plugável.

## Design
Arquitetura em camadas com seams injetáveis (substituem o kernel de origem):

| Módulo | Responsabilidade | Padrão |
|---|---|---|
| `lib.rs` | Crate root, `cfg_attr(not(feature="std"), no_std)`, re-exports públicos | Facade |
| `memory_doc.rs` | `MemoryDoc` (formato NMD1), `MemoryDocView` zero-copy, `VectorClock`, `MemoryLayer` L0–L7 | Contrato binário byte-idêntico ao OS |
| `art.rs` | `ArtIndex` — Adaptive Radix Tree Node4/16/48/256, scan por prefixo, tombstone delete | Radix Tree (Leis 2013) |
| `bq.rs` | `BqFlatIndex` — quantização binária (1-bit) + top-k por Hamming | Flat scan quantizado |
| `hamming_dispatch.rs` | Dispatch SIMD scalar/AVX2/AVX-512 (`#[target_feature]`), seam `cpu_caps()`/`set_cpu_caps()` | Strategy runtime |
| `engine.rs` | `AiosDatabaseEngine` — RAM L0/L1 + Storage L2–L7, indexação ART/BQ, rebuild | Engine de persistência |
| `sgdb.rs` | `Sgdb` — facade pública: remember/recall/rag_context/checkpoint; `Hit` | Facade (port de layers.rs) |
| `storage.rs` | `Storage` trait (4 métodos) + `InMemory` + `FileStorage` (append-log CRC32 crash-safe) + `SgdbError` | Trait plugável / Strategy |
| `tickv.rs` | Codec TKLV/TKCK byte-exato do TickvLite do OS + backend `TickvFile` | Interop de formato |
| `crdt.rs` | `CrdtMemorySync` (LWW) + `Transport` trait + `UdpTransport` (feature `p2p`) | CRDT / Observer |

## Flow
1. `Sgdb::open(backend: impl Storage)` → cria `AiosDatabaseEngine` + `rebuild_indices_from_storage` (scan `md/`, reindexa ART/BQ)
2. `remember_*` → `MemoryDoc::encode` (NMD1) → L0/L1 RAM ou `Storage::put` (L2–L7) → `index_doc` (ART para chaves, BQ para L4/L5 com bitvec)
3. `recall(query: &[f32], k)` → `BqFlatIndex::top_k_f32` (BQ grosso) → rescore FP32 (distância 1−cos) → top-k fino → `Hit { key, text, dist }`
4. `checkpoint()` → flush RAM L0/L1 → Storage; `prune_working_ram()` → drop RAM
5. `TickvFile` grava records TKLV 512-alinhados; `scan_volume` = semântica do `recover()` do OS (hunt 512-aligned, EOF all-0x00/0xFF, last-wins)

## Integration
- Consumido por: `examples/` (bench, mcp_server), futuros apps host
- Depende de: apenas `alloc` (no_std) / `std` (FileStorage, UdpTransport, examples)
- Interop: `MemoryDoc` (NMD1) e `tickv` (TKLV/TKCK) byte-idênticos ao neural-os-core
- Seams: clock `now: u64` (não há relógio interno), `cpu_caps()`, `sgdb_log!` (no-op no_std / eprintln std)

## Gotchas (lições de port)
- `f32::sqrt` NÃO existe no core p/ `x86_64-unknown-none` → `sqrt_f32` Newton em `sgdb.rs`
- ART não suporta chave-prefixo (uma chave prefixo de outra) — usar chaves de largura fixa
- CRDT rate-limit usa `Option<u64>` (sentinela 0 falha quando primeiro sync em now=0)
- `deny(warnings)` no_std eleva dead-code a erro → `#[allow(dead_code)]` explícito em port-parity
