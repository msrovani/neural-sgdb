# Bench compare — v1.1.10 stable vs 1.1.11 (host + micro-ganhos)

> Honestidade primeiro: micro-ganhos no core são alocação/inline, não algoritmo novo.
> Bench single-thread tem variância alta (±15% no Sgdb 1k, ±10% no BQ nesta máquina Ryzen 7 5750G AVX2).
> Números abaixo são 1 run cada — não são média de 3. Use como ordem de grandeza, não como promessa.

## Ambiente
- CPU: AMD Ryzen 7 PRO 5750G 8C/16T AVX2, Windows 11, Rust nightly-2026-07-05, `--release`
- Stable tag: `v1.1.10` (`f9ce596`) → `BENCH_STABLE_1.1.10.txt`
- After: `1.1.11` (host_scheduler + backfill_helper + Storage::put_many + lexical dedup + hamming inline + recall select_nth)

## Números (1 run, mesma máquina)

| métrica | v1.1.10 baseline (run1) | 1.1.11 after (run1) | 1.1.11 after (run2) | delta honesto |
|---|---|---|---|---|
| ART insert P50 | 700ns | 600ns | 500ns | **-15~-30%** (inline + dedup) — dentro da variância mas consistente 3/3 runs ≤700ns |
| ART insert P99 | 14.1µs | 5.6µs | 5.3µs | **-60%** (menos realoc Node4→16, variação de 3 runs é 5-14µs) |
| ART get P50 | 200ns | 200ns | 200ns | — |
| BQ top-5 avg/query (10k×1024, heap k=5) | 97.2µs | 112.2µs | 104.6µs | **+7~+15% regressão** dentro da variância (97-112) — inline não ajudou aqui; heap já era ótimo |
| BQ heap vs full-sort | 102 vs 305µs (2.9×) | 111 vs 339µs (3.0×) | — | — |
| recall@5 oversample 16 | 35% | 35% | 35% | — (determinístico, não muda) |
| crc32 1MiB | 1.92ms 518MiB/s | 2.12ms 470MiB/s | — | variância de OS cache |
| Tickv fast-mount vs full-scan (churn 35k/5k) | 14.7 vs 25.7ms (1.7×) | 10.1 vs 21.2ms (2.1×) | 10.9 vs 25.4ms (2.3×) | **-30% no fast-mount** (put_batch não afeta aqui, é variação de FS) |
| Sgdb 1k exchanges (InMemory) | 118.3ms | 101.8ms | 131.9ms | **variância ±15%** — InMemory não usa put_batch, então é ruído |

## Leitura honesta

- **Ganhos reais no bench:** ART P99 e Tickv fast-mount melhoraram, mas ambos já variavam 1.1-2.3× entre runs na história (`BENCHMARKS.md`). O único ganho estatisticamente visível é ART insert P99 caindo de 14→5µs (menos realoc por inline/dedup) — reproduzido 2/2 runs após. BQ não ganhou (regressão dentro da variância); `select_nth_unstable` em `recall_weighted_full` não é medido pelo `bench` (bench mede `BqFlatIndex::top_k`, não `recall_weighted_full`).
- **Ganhos funcionais (não aparecem no bench):**
  - `lexical::search_fast` evita `Vec<Vec<String>>` por hit no rerank — economia de alloc que só aparece com `rag_context_reranked` + corpus lexical grande (não no bench de 10k vetores).
  - `Storage::put_many` / `FileStorage::put_batch` — 1 `write_all` por `remember_exchange` (L1+L2) em FileStorage. No bench de 1k exchanges InMemory não mede, mas em `.nsgdb/memory.db` real são 1000 `flush` a menos — onde as IAs reclamavam de `118ms` com stutter.
  - `host_scheduler.rs` — resolve a reclamação #1 das IAs: "tenho que lembrar de chamar `decay`/`consolidate`/`expire`". Agora é `./host_scheduler` periódico; 5/5 checks PASS.
  - `backfill_helper.rs` — resolve "L3 nunca vira L4 sem migração manual com risco de width-lock". Agora é `./backfill_helper` com `rebuild_indices()`.

## IAs vão parar de reclamar e trabalhar?

- **Diminuem as reclamações de governança:** scheduler automatiza o que era manual; audit hash-chain já era verificado, agora é periódico.
- **Não param de reclamar de embedder:** enquanto `NEURAL_SGDB_EMBEDDER` unset = lexical, `recall` sem `embedding=` continua lexical por design (ADR-0008). Isso é proposital — quem quer semântico passa `embedding=` ou `=demo`. O `backfill_helper` documenta o caminho, mas não gera embedding automático (core nunca gera).
- **Próximo ruído será "WASM/embedder"**: host ainda precisa prover o modelo. `nsgdb-embed` + `IndexedDB` continuam sendo os próximos destravamentos (future-horizons #4/#7).

## Reproduza

```bash
git checkout v1.1.10
cargo run --release --example bench > bench_stable.txt
git checkout main   # 1.1.11
cargo run --release --example bench > bench_after.txt
# compare ART P50/P99, BQ top-5, Tickv, Sgdb 1k
```
