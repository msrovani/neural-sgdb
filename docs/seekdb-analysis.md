# seekdb (OceanBase) — análise de aderência e viabilidade (2026-09-25)

Fonte: <https://github.com/oceanbase/seekdb> + blog de lançamento + análise de
arquitetura. seekdb = "AI-Native Search Database", Apache 2.0, MySQL-compatible,
embedded ou server, construído sobre o motor SQL da OceanBase. Posicionamento
declarado: **"state store for AI agents"** — o MESMO público-alvo do
neural-sgdb. É o primeiro player do landscape que compete de frente (os
anteriores — mem0/Zep/Letta — são camadas, não bancos).

## Panorama: o que o seekdb traz de novo (vs. o landscape de 2026-08)

Três inovações arquiteturais declaradas:

1. **Streaming write + busca imediata (Change Stream + two-level HNSW)** —
   o write path comita e retorna SEM esperar o índice; um pipeline
   assíncrono consome o redo log e alimenta um **HNSW delta**; queries
   batem em delta + snapshot com read locks finos. Resultado vendido:
   1.523 QPS com P99 21,7 ms, P99 jitter 1,1× sob concorrência (10,7×
   Milvus em QPS no mesmo workload).
2. **COW sandbox em kernel (FORK DATABASE / MERGE TABLE / DROP)** —
   snapshot de banco inteiro em segundos sem cópia; o agente experimenta
   num sandbox (escreve, quebra, descarta) e faz MERGE de volta com
   estratégia FAIL/THEIRS/OURS. Posicionado como a feature-assinatura.
3. **Hybrid search num único plano SQL** — vector + full-text + filtro
   escalar no mesmo execution plan, sem fusão client-side (anti-N+1).

## O que o neural-sgdb JÁ cobre (sobreposição)

| Conceito seekdb | Equivalente nsgdb | Estado |
|---|---|---|
| Hybrid vector+FT num plano | `recall_hybrid`/`recall_hybrid_rrf` (lexical + semântico, RRF) | ✅ desde v1.1.8 |
| Filtro escalar no plano | `ScopeDims` + `scope_user/agent/app/run` + entities 1-hop | ✅ (null-scoping mais rígido que o deles) |
| Delta searchable logo após write | BQ flat é **append-only** — write é indexado na hora (sem rebuild); lexical idem (postings no put) | ✅ por construção (ADD-only) |
| P99 flat sob write+search | não medido — single-writer MCP, sem benchmark de concorrência | ⚠️ gap de MEDIÇÃO, não de arquitetura |
| Embedded mode | lib Rust in-process (`Sgdb::open(InMemory/FileStorage)`) | ✅ |
| ACID/transações | NMD1 append-only + checkpoint; sem multi-op tx | ➖ fora do escopo (memórias, não dados) |
| MySQL protocol/ecossistema | MCP é o protocolo | ➖ filosofia diferente (deliberado) |

**Leitura**: o nsgdb já resolve o problema nº 1 deles (write→search
imediato) pela arquitetura ADD-only, não por pipeline assíncrono. O que o
seekdb faz com engenharia de kernel (Change Stream), o nsgdb faz com uma
decisão de formato (BQ append-only). Ganho de simplicidade é nosso.

## Ideias viáveis para o nsgdb (roadmap, com aderência)

### 1. FORK/branch de memória — sandbox para o agente (valor alto, aderência alta)

A ideia mais exportável. O nsgdb já tem TODOS os primitivos: `scope_dims`
(`ScopeDims.run` = "branch"), supersede/DAG, CRDT delta, `sys/validity/`.
Um `fork_scope(run="exp-42")` + recalls escopados por dims JÁ dão o
sandbox **sem nada de novo no core** — só um harness MCP (`curate
op=commit_run` já é o merge!). O que falta é: (a) descoberta do branch
(`scopes_to_probe` já lista — OK v1.1.26), (b) `merge` = replay dos writes
do run no escopo base com estratégia THEIRS/OURS (o `commit_run` hoje
arquiva episódicos; falta o modo "promover L3→L3 base"), (c) DROP =
delete por prefixo de scope (existe via `scan_prefix` + delete).
**Estimativa: S–M, tudo no MCP/core existente. Risco baixo.** Aderência
total: o core não decide (merge é decisão do host), ADD-only preservado.

### 2. Métrica de P99 write+search concorrente (valor alto, custo baixo)

O claim central do seekdb é P99 flat. O nsgdb NUNCA mediu o seu. Um
`examples/bench_concurrent.rs` (thread-pool de writers + readers sobre
TickvFile, medir P50/P99 de recall com write contínuo) fecha ou derruba o
argumento de venda deles contra nós. **Estimativa: S.** Sem isto, qualquer
comparação honesta com seekdb é impossível.

### 3. Two-level index (delta + snapshot) para o BQ (valor médio, custo M)

O nsgdb tem o mesmo problema do seekdb num caso específico: o BQ flat é
append-only e o `reclaim_bq_orphans` só recompata com churn alto — um
DB longo acumula entradas inertes que o recall varre. Um "delta BQ"
(pequeno, hot, sem órfãos) + snapshot BQ (recompactado) com o recall
consultando os dois reduziria o custo do scan em DBs velhos. **MAS**: o
fast-mount IDX2 já mitiga o custo de restart, e o recall pula órfãos em
O(1). Medir antes (`bench` com 100k+ writes + 50% deletes) — só fazer se
o scan de inertes dominar. **Estimativa: M, condicionada a medição.**

### 4. MERGE com estratégias de conflito (valor médio, aderência média)

As estratégias FAIL/THEIRS/OURS do MERGE deles mapeiam para o que o CRDT
do nsgdb já decide (`Conflict, nunca sobrescreve` — ADR e `merge_remote`).
Não portar as estratégias para o core; expor como POLÍTICA do host no
harness de merge do item 1. **Aderência**: o core reporta o veredito do
clock; o host escolhe. Custo: embutido no item 1.

## Ideias REJEITADAS (incompatíveis com a doutrina)

- **SQL/MySQL protocol** — o protocolo do nsgdb é MCP por design (o
  consumidor é um MODELO, não um driver). Rejeitado.
- **ACID multi-op / transações** — memórias não são dados: a unidade é o
  write lógico com identidade própria; transação entre memórias não tem
  semântica cognitiva. Rejeitado.
- **GIS/JSON columns** — fora do escopo (L0–L7 já cobre payload estruturado
  via `ContentType::Json` + entidades). Rejeitado.
- **Async index pipeline como infra** — o nsgdb não tem redo log nem
  background tasks (no_std core, single-writer). O equivalente funcional
  já existe via formato. Rejeitado como infra, ideia do delta preservada
  no item 3.

## Veredito

seekdb valida o mercado que o nsgdb escolheu (agent state store) e traz
UMA ideia genuinamente nova para o nosso contexto: **fork/merge de estado
como primitivo de agente** — que no nsgdb é ~80% construído (scopes/dims +
commit_run + DAG) e só precisa de um harness de merge. Prioridade:
1. bench de concorrência (S) — fechar o gap de medição antes de qualquer
   claim;
2. harness fork/merge por scope run (S–M);
3. delta BQ — só com medição mostrando que inertes dominam.

Fontes: github.com/oceanbase/seekdb (README), en.oceanbase.com/blog/23848834048
(lançamento), zread.ai/oceanbase/seekdb (arquitetura: Change Stream,
two-level HNSW, FORK DATABASE).

## Plano priorizado — ideias de maior valor (pós-v1.2.0)

Ordenação por valor/custo, respeitando a lição da casa: **mede antes de
construir o instrumento**.

### Gap 0 — P99 write+search concorrente (o gap exposto, primeiro) — **MEDIDO (2026-09-25)**

Implementado: `examples/bench_concurrent.rs` (TickvFile, engine único sob
Mutex, writers `remember_text_with` + readers `recall_lexical`, 8s).

| fase | ops/s | P50 | P90 | P99 | P99.9 | jitter (P99/P50) |
|---|---|---|---|---|---|---|
| write  | ~210 | ~0.5 ms | ~0.7 ms | ~20 ms | ~1.2 s | ~38× |
| recall | ~160 | ~0.9 ms | ~1.5 ms | ~92 ms | ~1.0 s | ~108× |

**Veredito: o nsgdb NÃO tem P99 flat** — a mediana é ótima (sub-ms), mas o
tail paga o Mutex contencioso + flush do TickvFile (spikes de 20–100 ms e
cola de ~1 s). O claim do seekdb (P99 21,7 ms @ 1.523 QPS, jitter 1,1×)
soa hoje como distantíssimo — eles têm throughput ~8× maior com tail 5×
menor. Causas candidatas, a medir antes de qualquer fix: (a) lock global
serializa tudo (o single-writer é arquitetural, não incidental); (b)
fsync/append do TKLV no caminho de write; (c) auto-persist metrics-gated
do v1.2.0 pisando no tail. Isto é o achado honesto que o "mede antes de
construir" pedia: **a arquitetura ADD-only não compra P99 flat de graça**
— o que ela compra é simplicidade e write→search imediato, que a medição
confirma (P50 sub-ms em ambas as fases).

Nota metodológica: o bench compartilha UM engine sob Mutex (modelo MCP
real). O comparável do seekdb é server multi-session; um pool de engines
só seria possível com sharding por scope — não existe hoje.

O claim central do seekdb é P99 flat (21,7 ms @ 1.523 QPS). O nsgdb nunca
mediu o seu — e a matriz de testes toda é single-thread/single-writer.
Sem esse número, qualquer comparação com seekdb é retórica. **Ação:**
`examples/bench_concurrent.rs` — pool de writers (remember_exchange/
remember_text_with) + readers (recall_lexical/recall) sobre TickvFile,
P50/P99 de recall sob write contínuo. Estimativa **S**, risco baixo.
**Resultado possível:** o single-writer do MCP provavelmente NÃO tem P99
flat sob escrita concorrente — e isso é um achado honesto que define se
os itens seguintes valem o custo. Medir antes de construir.

### 1. Fork/merge de memória por scope run (valor alto, ~80% construído) — **FEITO (v1.2.1)**

Implementado: `Sgdb::promote_run(filter, base_dims, MergeStrategy)` + MCP
`curate op=promote_run` (`merge_strategy=fail|ours|theirs`) + `remember(key=)`
explícita (o host nomeia a key do run). 6 testes de lib; hot test fase 6e
(150/0). Aderência preservada: ADD-only intacta, core reporta / host decide.

A feature-assinatura do seekdb (FORK DATABASE / MERGE TABLE / DROP)
mapéa no nsgdb com primitivos que já existem: `ScopeDims.run` = branch,
`curate op=commit_run` = merge (hoje só arquiva episódicos),
`scopes_to_probe_dims` = descoberta, `scan_prefix`+`delete` = drop.
**Falta:** (a) modo "promover" no commit_run — replay dos L3/L4 do run
no escopo base com estratégia THEIRS/OURS/FAIL como POLÍTICA do host
(outra coisa: o core reporta o veredito do clock, nunca decide);
(b) bootstrap documentado do fluxo sandbox→merge no MCP. Estimativa
**S–M**, risco baixo (ADD-only intacta, formato intacto). É o item que
fecha o único diferencial genuíno do seekdb contra nós.

### 2. P2 — levar o resultado do Gap 0 ao BENCHMARKS.md e ao landscape

Com o número medido: atualizar `BENCHMARKS.md` com a seção de
concorrência e `docs/memory-landscape.md` com a linha do seekdb (ele
entra como primeiro competidor banco-nativo, não camada). Estimativa
**S** (docs), condicionada ao Gap 0.

### 3. Delta BQ two-level (valor médio, condicionado a medição) — **MEDIDO: NÃO VALE**

**Veredito medido (2026-09-25, `examples/bench_long_db.rs`, 256-dim,
FileStorage, 50% deletes):** os órfãos do BQ custam ~0 no recall —
com-órfãos ≈ pos-reclaim ≈ pos-rebuild dentro do ruído em N=2k, 20k e
60k (ex.: 60k: P50 697µs vs 694µs vs 701µs; os órfãos foram só 48 porque
o reclaim de threshold 64 dispara sozinho no caminho do `delete`). O
`reclaim_bq_orphans(0)` completo custou 2,3 ms em 60k. **O delta BQ
two-level está REJEITADO por medição** — o recall pula órfão em O(1)
como o design prometia; não há scan de inertes a amortizar.

**Achado REAL da medição (mais valioso que o item 3): o DELETE é O(N)**
(`examples/bench_delete_cost.rs`): 0,31 → 0,67 → 1,86 ms/delete com
N=5k/10k/20k (linear em N, constante no D). Causa: `engine::delete`
varre o `clock_index` INTEIRO por delete
(`clock_index.iter().filter(...)` procurando a sk morta). O
bench_long_db mediu 30k deletes em 265 s (8,8 ms/delete @ N=60k) — o
delete é ~240× mais caro que o write (~37 µs/doc) e DOMINA workloads de
churn, não os órfãos do BQ. **Fix proposto (M, aditivo, sem quebrar
formato): índice reverso `sk → Vec<(u8, u64)>` preenchido no
`index_doc` (o relógio já está em mãos lá), consultado no `delete`;
drop/rebuild segue igual.** Substitui o item 3 na fila.
**FEITO ainda em v1.2.1** — e a implementação achou DOIS outros O(N) no
mesmo delete (`entity_index` scan e `id_to_sk` scan), os três trocados por
reversos. Resultado: 30k deletes @ 60k docs caíram de 265 s para 43 s
(~6×); por delete 1,86→0,72 ms @ 20k. Ver BENCHMARKS.md.

### Ordem de execução

```
Gap 0 (bench_concurrent, S)   — FEITO: P99 NÃO flat (BENCHMARKS.md)
  ├─ item 1 (fork/merge, S–M) — FEITO: v1.2.1
  ├─ item 2 (docs)            — FEITO: landscape + README
  └─ item 3 (delta BQ, M)     — REJEITADO por medição; o achado real é
                                o delete O(N) via clock_index (bench
                                delete_cost) — índice reverso é o novo item
```

Total do lote: **~1 sprint (S–M)**. Nada muda formato (NMD1/TKLV
intactos); o bump de contrato esperado é minor (nova superfície no
`curate`, pin no mcp_client no mesmo commit).
