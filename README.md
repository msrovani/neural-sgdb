# neural-sgdb

**Banco de memória persistente e transferível para agentes de IA.**

> Memórias, não dados.

`neural-sgdb` é um substrato de memória para sistemas de IA: o que ele armazena,
sincroniza e transfere são **memórias** — com camada cognitiva, vector clock e
identidade — não pacotes de dados genéricos.

Nascido dentro do [neural-os-core](https://github.com/msrovani/neural-os-core),
um OS bare-metal com IA desde o boot, este projeto é a extração independente do
seu sistema de gestão de memórias (SGDB) para uso da comunidade.

## O que ele faz

- **8 camadas de memória (L0–L7):** Sensory → Working → Episódica Curta/Longa →
  Semântica → Procedural → Identidade
- **`remember` / `recall` semântico:** busca vetorial binária quantizada (BQ) com
  dispatch SIMD (AVX-512 / AVX2 / scalar), sem dependências externas (sem FAISS,
  sem HNSW)
- **Transferência de memórias entre nós:** sincronização CRDT (last-write-wins)
  — memórias viajam entre agentes/instâncias com versionamento, não pacotes
- **Persistência power-loss safe:** append-log com CRC; memória sobrevive a
  crash/reinício (checkpoint/restore)
- **Busca por chave/fato em O(k):** índice ART (Adaptive Radix Tree)
  Node4→16→48→256, sem rebalanceamento
- **`no_std` + `std`:** roda em bare-metal e em aplicações host — um único núcleo

## Por que memórias?

Agentes de IA hoje têm contexto efêmero. `neural-sgdb` dá a eles um cérebro
persistente: camadas de memória com semântica real, recall semântico em
microssegundos e a capacidade de **transferir memórias entre instâncias** —
sem SQL, sem filesystem tradicional, sem runtime externo.

## Estado

**v0.1 extraído** ✅ — o núcleo portátil está no repo como crate `neural-sgdb`
dual-mode (`no_std` + `std`, zero dependências):

- `cargo test` no host: **20 testes + doc-test passando**
- `cargo check --no-default-features --target x86_64-unknown-none`: **limpo**
- Portados: ART (Node4/16/48/256 + SSE), MemoryDoc L0–L7 (formato NMD1
  byte-idêntico ao OS mãe), BQ + Hamming SIMD (AVX-512/AVX2/scalar), engine
  instance-based
- Novos: `Storage` trait + `InMemory` + `FileStorage` (append-log com CRC32,
  crash-safe) + facade `Sgdb` (`remember_exchange`, `remember_semantic`,
  `recall`, `rag_context`, `remember_fact`, `scan_prefix`, `checkpoint`)
- Contrato de API completo em [`docs/api.md`](docs/api.md)

A reference implementation roda em bare-metal no OS mãe (`k_ai::sgdb`, AGPL);
este repo evolui separado (MIT OR Apache-2.0).

## Uso rápido

```rust
use neural_sgdb::{Sgdb, FileStorage};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = Sgdb::open(FileStorage::open("agent_memory.db")?)?;

    db.remember_exchange("qual o clima?", "sol, 24 graus")?;
    db.remember_semantic("turno:1", "clima ensolarado em sao paulo", &emb)?;

    let hits = db.recall(&query_emb, 5)?;
    let ctx = db.rag_context(&query_emb, 3)?;
    println!("{ctx}");
    Ok(())
}
```

## MCP (agentes de IA)

`cargo run --release --example mcp_server` expõe `remember` / `recall` /
`rag_context` como MCP tools (JSON-RPC 2.0 sobre stdio, handshake
`2025-11-25`) — conectável a Claude Code, Cursor e OpenCode:

```bash
# Claude Code
claude mcp add neural-sgdb -- cargo run --release --example mcp_server
```

⚠️ O embedding de recall no MCP é de **demonstração** (hash de trigramas);
para recall semântico real, forneça embeddings próprios via `remember_semantic`
/ `recall`.

## Benchmarks

`cargo run --release --example bench` — números do ambiente local (AVX2):
ART get P50≈200ns, ART insert P50≈800ns, BQ top-5 ≈310µs em 10k×1024 dims,
recall@5 BQ vs FP32-exact = 100% (trade-off da quantização 1-bit, medido).

## Licença

Licenciado sob **MIT** **ou** **Apache-2.0** (dual license), à sua escolha.

## Roadmap

- [x] Extração do núcleo portátil (ART, MemoryDoc L0–L7, BQ + Hamming SIMD)
- [x] Trait de storage plugável (InMemory + FileStorage) e relógio/CPUID injetáveis
- [x] CRDT sync de memórias como feature opcional `p2p` (`CrdtMemorySync` +
      trait `Transport` + `UdpTransport` std; merge LWW simétrico)
- [x] Benchmarks publicados (`cargo run --release --example bench` — ART
      P50/P99, BQ top-k, recall BQ vs FP32)
- [x] Camada MCP server (`cargo run --release --example mcp_server` — expõe
      `remember`/`recall`/`rag_context` a agentes de IA via MCP sobre stdio;
      embedding demo por trigramas)
- [ ] Interop de storage TKLV byte a byte com o OS (FileStorage → formato
      TickvLite) — **adiado**: requer o leitor TickvLite do OS no host para
      verificação honesta; NMD1 (documento) já é interoperável
