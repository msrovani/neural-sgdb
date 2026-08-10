# neural-sgdb v0.1.0

**Banco de memória persistente e transferível para agentes de IA — memórias, não dados.**

Núcleo extraído do [neural-os-core](https://github.com/msrovani/neural-os-core)
(`k_ai::sgdb`, ADR-0063) como projeto comunitário independente: **zero deps,
dual-mode `no_std` + `std`, MIT OR Apache-2.0**.

## O que entrega

- **8 camadas de memória L0–L7** (Sensory → Working → Episódica → Semântica →
  Procedural → Identidade) com formato `MemoryDoc` (NMD1) byte-idêntico ao OS
- **`remember` / `recall` semântico** — BQ (quantização binária) + rescore FP32,
  dispatch SIMD AVX-512 / AVX2 / scalar
- **Índice ART** (Adaptive Radix Tree) O(k), Node4→16→48→256
- **Storage plugável** via `Storage` trait — `InMemory`, `FileStorage`
  (append-log CRC32 crash-safe) e **`TickvFile` (formato TKLV byte-exato do
  TickvLite do OS — interop de volumes)**
- **CRDT memory sync** (feature `p2p`) — `CrdtMemorySync` + `Transport` +
  `UdpTransport`, merge LWW simétrico
- **Benchmarks** — ART P50/P99, BQ top-k, recall BQ vs FP32
- **MCP server** — `remember`/`recall`/`rag_context` para agentes de IA
  (Claude Code, Cursor, OpenCode) via MCP sobre stdio

## Verificação

- `cargo test` — 30+1 testes (default), 34+1 com `p2p`
- `cargo check --no-default-features --target x86_64-unknown-none` — limpo
- Interop: NMD1 e TKLV/TKCK verificados por golden test + re-parse
  (`scan_volume`, semântica do `recover()` do OS)

## Roadmap

6/6 completo. Próximos (v0.2): GC/compactação do `TickvFile`, checkpoint TKCK
no backend, CI GitHub Actions.
