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
