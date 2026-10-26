# ADR-0019 — Aprendizado por trajetória é convenção de HOST, não primitivo do core

**Data:** 2026-10-26 · **Status:** aceito · **Release:** v1.5.0

## Contexto

Frameworks de agentes vendem "trajectory contrastive learning": guardar o
histórico de tentativas (a solução que funcionou, o erro que repetiu, o padrão
que passou no teste) e **ajustar o peso das memórias conforme o desfecho real**,
para o roteador escolher melhor na próxima tarefa igual. O pedido de um
consumidor (Ruflo) chegou nessa forma: banco vetorial próprio, HNSW, aprendizado
por contraste, namespaces por enxame.

A parte vetorial é explicitamente não-construível aqui: HNSW/FAISS externo é
💤 (`ROADMAP.md`, ADR-0002) e a quantização já existe (BQ + ADC-lite). A parte
de **aprendizado** é outra coisa: ela não pede um índice novo, pede uma
CONVENÇÃO sobre primitivos que o crate já tem:

- `remember_episodic(user, response, now)` — o par cru, verbatim (ADR-0004 da
  doutrina: `remember_episodic` existe justamente porque extrair/resumir perde
  o contexto que a decisão precisava);
- `feedback(key, positive, amount)` — re-pondera importance **e** confidence
  pelo resultado real, em `sys/meta/` (v1.1.4 item 3), sem tocar no ranking
  default de quem não o chama;
- o 1-hop de entidades (`recall_entities`) — ranking determinístico
  (overlap desc → importância desc → key asc).

Ou seja: **o material da decisão já existe; o que faltava era a disciplina de
uso ser explícita e MEDIDA** — sem isso "aprendizado por trajetória" vira uma
feature que ninguém consegue provar que funciona.

## Decisão

O core **não** ganha primitivo novo. A convenção fica registrada e é demonstrada
por um harness determinístico, `examples/trajectory_learning.rs` (19 auto-checks,
`cargo run --release --example trajectory_learning`):

1. **A trajetória é escrita verbatim.** Cada tentativa grava o par
   `remember_episodic("tarefa …", "estrategia … -> sucesso|falha")` e declara a
   entity da trilha (`traj/<classe>/<braço>`) — a tentativa fica CONTÁVEL, e a
   lição aponta para a memória usada (`associate_checked(DerivedFrom)`).
2. **O desfecho re-pondera a memória USADA.** `feedback(strat_key, ok, 0.1)` no
   mesmo instante. O sinal é CONTRASTIVO: o braço que falha **desce** de
   importance **e** confidence (não é só reforço positivo).
3. **O roteador consulta a memória** (`recall_entities("strategy/<classe>")`),
   não uma tabela paralela. A "aprendizagem" é, literalmente, a ordem do 1-hop
   mudando — auditável com `explain`/`meta`.
4. **Exploração é política do HOST.** O roteador guloso nunca arrisca um braço
   novo (importance inicial < incumbente); quem decide explorar é o host, e o
   desfecho dessa exploração entra na mesma memória. O core não explora, não
   decide, não gera texto.

## Medição (o que o exemplo pinou)

Duas configurações sobre a MESMA bateria determinística (2 classes × 2 braços,
gabarito fixo) e o MESMO custo de exploração (1 tentativa forçada por braço):

| Config | Exploração | Época 1 (roteada) | Época 2 | Sessão 2 (1ª tentativa) |
|---|---|---|---|---|
| **A — hoarder** (escreve, nunca re-pondera) | 2/4 | 4/8 (50%) | 4/8 (50%) | erra |
| **B — trajetória** (`feedback` por desfecho) | 2/4 | 8/8 (100%) | 8/8 (100%) | acerta de primeira |

A leitura: **acumular memória não ensina; re-ponderar pelo desfecho ensina.** O
mesmo custo de exploração que em A vira nada (SR travado em 50%) em B vira
100%, e a decisão sobrevive ao fim da sessão porque ela mora na memória, não no
processo.

## Consequências

- **Positivo**: o comportamento que o mercado chama de "aprendizado contínuo"
  fica disponível sem formato novo, sem dependência e sem `no_std` quebrado; e
  fica PROVADO por um exemplo que roda no CI local (`release-gate`).
- **Negativo / custo**: a responsabilidade é do host — quem não escrever o
  desfecho, ou não chamar `feedback`, não aprende nada. O core não pode
  detectar isso (e não deve tentar: inferir desfecho exigiria julgar o mundo).
- **Contra a tentação**: nada de "auto-reflexão" no core, nada de extração de
  lição a partir de texto, nada de LLM. A lição é do host; o core guarda e
  re-pondera. Mesmo contrato de `entities`/`Embedder`: **quem fornece, decide**.
- **Contrato**: nenhum vestígio de wire novo (NMD1/TKLV/MDM1 intocados); o
  release sobe como MINOR pelo conjunto da obra (trigger rules + doc) e o
  contrato MCP referencia esta ADR na descrição das ops de curadoria.
