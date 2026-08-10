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

Em extração ativa do neural-os-core. O núcleo portátil (ART + MemoryDoc + BQ +
Hamming SIMD) está sendo isolado em crate independente. A reference
implementation roda em bare-metal no OS mãe; este repo segue com ciclo próprio
de docs, CI e benchmarks.

## Licença

Licenciado sob **MIT** **ou** **Apache-2.0** (dual license), à sua escolha.

## Roadmap

- [ ] Extração do núcleo portátil (ART, MemoryDoc L0–L7, BQ + Hamming SIMD)
- [ ] Trait de storage plugável (RAM → flash → file) e relógio/CPUID injetáveis
- [ ] CRDT sync de memórias como feature opcional `p2p`
- [ ] Benchmarks publicados (P50/P99, recall vs FP32)
- [ ] Camada MCP server para agentes externos consumirem memória
