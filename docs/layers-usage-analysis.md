# Estudo — camadas L0–L7: evolução, uso real e lacunas

> Status: **análise (2026-10-16)** — feita lendo o código como fonte da verdade
> (`memory_doc.rs`, `engine.rs`, `sgdb.rs`, `lifecycle.rs`, `crdt.rs`,
> `examples/mcp_server.rs`) e confrontando com `docs/doctrine.md`,
> `docs/agent-self-program.md` e `docs/architecture/01-memory-model.md`.
> Todos os números de linha são do estado corrente (crate 1.4.5).
> Lente: **eu sou o consumidor** — o que um LLM via MCP alcança de fato.

## TL;DR — o veredito

1. A taxonomia declara **8 camadas**; o consumidor LLM alcança **3** (L2, L3,
   L4). L0/L1 não têm caminho de retrieval; L5/L7 não têm caminho de escrita
   pelo agente; L6 não é camada de doc (é side-table de arestas).
2. A camada **não é escolhida** pelo agente (`author=agent` + `layer=` é
   recusado), **não é filtrável** no recall (não existe arg `layer`) e **não
   está no `Hit`** — só no `explain`, uma key por vez. Ou seja: a taxonomia é
   real no armazenamento e invisível na interface.
3. Não existe **contagem por camada** em nenhuma view do `health`: não dá para
   provar (nem detectar) uso — "tudo vira L3" é indetectável.
4. A escada de promoção (L1→L2→L3→L4) e o `transfer_to` são **lib-only**: o
   host tem que chamar; a superfície MCP não expõe o mecanismo nem como op.
5. Achado concreto (bug de contrato): a escrita **operacional** (`author=system`)
   em L4/L5 **apaga o texto** (payload vira bytes f32, sem companion) e
   **descarta silenciosamente** `scope`/`entities`/`type`/`model_id`/`index_key`
   — a meta é gravada MÍNIMA (`persist_meta`), então o doc fica global e sem
   entities (em L7 isso o torna inalcançável: sem entities não sobra rota).
   **Confirmado em runtime** por `scripts/probe-layers-mcp.py` (v. §3.1).
6. Drift de doc no caminho do cold-start: `docs/agent-self-program.md:32` e
   `docs/MCP.md:260` mandam confirmar `mcp_tool_count=4`, mas o `health` emite
   **5** (`EXPECTED_MCP_TOOL_COUNT`).

## 1. Mapa: definição → uso real

| Camada | Tier | Índice (engine.rs `index_doc`) | Escrita pelo AGENTE | Escrita `author=system` | Recuperável por |
|---|---|---|---|---|---|
| **L0** Sensory | RAM | ART | — | `layer=L0` (`put_operational`) | **nenhum** path |
| **L1** Working | RAM | ART | `remember_exchange` (lib) | `layer=L1` | **nenhum** path |
| **L2** EpisodicShort | storage | ART + **lexical** | `remember(user=,response=)`, `remember_episodic` | `layer=L2` | lexical, `diary` |
| **L3** EpisodicLong | storage | ART + **lexical** | `remember(text=)` sem vetor; `curate op=consolidate` | `layer=L3` | lexical, entities, `profile` |
| **L4** Semantic | storage | ART + **BQ** (+ lexical via companion `/L2/`) | `remember(text=,embedding=)`; `commit_run` facts | `layer=L4` + `embedding=` | semantic, lexical(companion), entities, `profile` |
| **L5** Procedural | storage | ART + **BQ** | **nenhuma** | `layer=L5` + `embedding=` | semantic, entities, `profile` |
| **L6** `L6Reserved` | — | side-table `sys/rel/` | `associate`/`related_to`/`contradicts` | — | ART fwd/rev (arestas) |
| **L7** Identity | storage | ART | **nenhuma** | `layer=L7` | entities *(se houver meta)*, `explain` |

`Hit` **não** carrega `layer` (`sgdb.rs:717-759`: key, text, dist, provenance,
path, content_type, payload_type, score, matched_terms, validity, rel,
score_breakdown, type_scores). O `layer` existe em `MemoryExplanation`
(`sgdb.rs:768`), devolvido por `curate op=explain` — **uma key por vez**.

### 1.1 Regras POR CAMADA (tabela única)

| Regra | L0 | L1 | L2 | L3 | L4 | L5 | L6 | L7 |
|---|---|---|---|---|---|---|---|---|
| `default_importance()` (`memory_doc.rs:952`) | 0.0 | 0.0 | 0.2 | 0.4 | 1.0 | 0.8 | 0.5 | 1.0 |
| Penalty no `recall_weighted` (`sgdb.rs:6104`) | 1.0 | 1.0 | 0.8 | 0.6 | 0.0 | 0.2 | **1.0** | 0.0 |
| Merge CRDT (`crdt.rs:253`) | Local | Local | MultiValue | MultiValue | CausalLww+hist | ControlledLww | Reserved | ControlledLww |
| Promoção automática (`lifecycle.rs`) | — | commit→L2 (age ≥1) | →L3 (imp ≥0.5, age ≥2) | →L4 (imp ≥0.8, age ≥3) | — | — | — | — |
| Decay (`LifecycleConfig`) | — | — | sim | sim | sim | sim | — | — |

Duas notas que a tabela expõe:

- A penalty é `1 − default_importance` em L2–L5 e L7, mas **L6 cai no
  `_ => 1.0`** (o default é 0.5) — duas tabelas querendo dizer a mesma coisa e
  divergindo em um caso. É o sintoma clássico da casa ("regra em N lugares").
- L4 e L7 empatam no topo (1.0), o que é coerente (fato e identidade são o
  mais retido) — mas como L7 é inalcançável pelo agente, na prática o topo da
  escala é exercido só por L4.

## 2. Regras e práticas vs. premissas

**Coerente (bom desenho, confirmado no código):**

- *"O core não decide"* — verdadeiro no conteúdo; a única decisão é de
  **formatação**: `remember` escolhe a camada pelo tipo do payload
  (`texto→L3`, `embedding→L4`, `user/response→L2`), ADR-0008. É determinístico
  e auditável, não heurística cognitiva.
- **ADD-only** intacta: L2/L3 usam `MultiValueRegister` no CRDT (`crdt.rs:254`),
  L4 `CausalLwwWithHistory`, L5/L7 `ControlledLww`, L0/L1 `LocalWorking`.
  Conflito preservado, arbitragem na leitura.
- **L4→L5 nunca automático** (`lifecycle.rs:17`) — procedimento exige HITL.
- **Camadas locais não adotam versão remota** (`crdt.rs:573`): L0/L1/L6
  bloqueados por política. Coerente com "working memory é do nó".
- **`TypeScores` (ADR-0016)** resolve a exclusividade da camada: 4 eixos
  sobrepostos derivados de `layer + entities` (`sgdb.rs:271`), nunca
  persistidos. É a resposta certa para "uma categoria não carrega tudo".
- **Retrieval é loud** em dim mismatch e o BQ tem guarda de era (ADR-0007).

**Tensão 1 — a camada cognitiva é derivada, não escolhida.** Como o agente não
escolhe camada e a leitura só devolve `type_scores` (probabilístico), a
semântica cognitiva real mora em `entities` (`mom/*`, `pref/*`, `adr/*`). Isso é
defensável — mas é bom explicitar: **L0–L7 é uma classe de armazenamento; o
papel cognitivo é a entity.**

**Tensão 2 — "8 camadas" é 7 + 1.** L0/L1 são RAM sem nenhum caminho de
retrieval, e L6 não guarda docs. A arquitetura já diz isso
(`01-memory-model.md` §2), mas a doutrina fala em "camadas L0–L7" como se as 8
fossem simétricas para o consumidor.

**Tensão 3 — dois dos quatro eixos de `TypeScores` são dead-end hoje.** O
eixo `preference` vale 1.0 só com entity `pref/*` (ou 0.8 em L7) e `procedural`
vale 1.0 só em L5 (`sgdb.rs:286-300`). Como L5/L7 são inalcançáveis pelo
agente, `procedural` **nunca** chega a 1.0 e `preference` só existe via tag.
O modelo promete quatro sinais; o consumidor exercita dois.

## 3. Achados (evidência)

- **A1 — camada não é filtrável.** O schema do `recall` aceita `query`, `mode`,
  `causal_depth`, `format`, `max_payload_bytes`, `embedding`, `k`, `scope*`,
  `cursor`, `entities`, `at`, `w_sem`, `w_time`, `rag`, `rerank`, `historical`,
  `annotations` — **não** `layer`. Não se pergunta "só procedural" nem "só
  identidade".
- **A2 — no write, `layer=` exige `author=system`** (`mcp_server.rs:229`), e o
  ramo `system` **exige** `layer=` e recusa L4/L5 sem `embedding=`
  (`mcp_server.rs:237-262`). Consequência direta: L5/L7 não têm escrita de
  agente.
- **A3 — escrita operacional descarta parâmetros em silêncio.** O ramo
  `author=system` monta o doc e chama `put_operational`, retornando cedo: os
  `RememberOptions` já construídos (`scope`, `entities`, `content_type`,
  `scope_dims`, `model_id`, `index_key`) **não são aplicados**. A resposta ainda
  ecoa `content_type: params["type"]` (`mcp_server.rs:280-282`) — anuncia o que
  não persistiu.
- **A4 — escrita operacional em L4/L5 perde o TEXTO.** Com `embedding=` presente,
  `doc.payload` é **sobrescrito** pelos bytes f32 (`mcp_server.rs:266-270`) e o
  ramo não grava companion (`companion_key: ""`). O comentário do próprio código
  diz "o texto E o payload" — o payload virou vetor. O texto é irrecuperável.
- **A5 — a meta operacional é mínima e ignora os params.** Caminho:
  `Sgdb::put_operational` → `engine.put_operational` → `put_inner` →
  `persist_meta` (`engine.rs:407`), que **cria** meta com `source = node_id`,
  `confidence = 1.0` e `importance = doc.layer.default_importance()`. O que não
  acontece é aplicar `scope`/`entities`/`content_type`/`model_id`/`authority` do
  request: o doc fica **global, sem entities, sem tipo declarado**. Em L7 isso é
  fatal — sem entities o L7 não tem NENHUMA rota (não é lexical, não é BQ).
- **A6 — nenhum caminho de recall toca L0/L1.** `index_doc` só alimenta o
  lexical com L2/L3 (`engine.rs:777`) e o BQ com L4/L5 (`engine.rs:791`); L0/L1
  só entram no ART. `remember_exchange` (L1) nem é usado pelo MCP — o MCP manda
  `user=`/`response=` para `remember_episodic` (L2 timestamped,
  `mcp_server.rs:837`).
- **A7 — a escada de promoção é lib-only.** `MemoryLifecycle::tick`
  (`lifecycle.rs:104`) e `transfer_to` não aparecem no `mcp_server` (zero
  ocorrências). O agente não promove, não observa promoção e não move camada; o
  que existe é `curate op=consolidate` (L2→L3 determinístico) e `commit_run`
  (fatos L3/L4).
- **A8 — zero observabilidade por camada.** O `health`/`HealthReport` expõe
  `doc_count` (total do ART `md/`) e distribuição por **escopo**, mas nenhuma
  contagem por camada. Não há `layer_distribution`.
- **A9 — drift de doc no cold-start.** `docs/agent-self-program.md:32` e
  `docs/MCP.md:260` dizem `mcp_tool_count=4`; `health` emite 5.
- **A10 — a camada é re-derivada da KEY em dois lugares.**
  `ctype::key_carries_embedding` e `sgdb::layer_importance` (`sgdb.rs:6104`)
  fazem scan de string no storage key (`/L4/`∨`/L5/`; `windows(4)==b"/L"`) em
  vez de usar `doc.layer` (a fonte única). `src/codemap.md` já manda reusar
  `key_carries_embedding`; `layer_importance` ainda reimplementa o scan — e é
  justamente onde a tabela de importância diverge (A-nota acima).

### 3.1 Prova em runtime (a interface real, não a leitura)

`scripts/probe-layers-mcp.py` sobe o `mcp_server` (DB temporário, não toca o
banco do projeto), faz `initialize` e exercita o fluxo pela superfície MCP:

```
remember(author=system, layer=L4, key=probe/l4/sentinela,
         text="SENTINELA_L4_TEXTO_9f2c", embedding=[0.1,0.2,0.3,0.4],
         scope="probe/scope", entities=["mom/probe"], type="text")
```

Resultado (crate 1.4.5, 2026-10-16):

| Observação | Valor | Achado |
|---|---|---|
| `structuredContent.companion_key` | `""` | A4 — nenhum companion de texto |
| `structuredContent.scope` | `""` (pedi `probe/scope`) | A3 — param descartado |
| `structuredContent.entities` | `[]` (pedi `mom/probe`) | A3 — param descartado |
| `recall(query="SENTINELA…", mode=lexical)` | `[]` (0 hits) | A4 — o texto não é recuperável |
| `recall(mode=semantic, embedding=mesmo vetor)` | hit `md/L4/probe/l4/sentinela`, `dist=0.0`, `payload_type=embedding` | a memória **existe**; o que se perdeu foi o texto |
| `curate(op=explain, key=md/L4/…)` | `layer: "l4"`, `importance: 1.0` | a meta é mínima mas existe (corrige a leitura inicial) |
| `health` — campos com "layer" | nenhum | A8 |
| `health.mcp_tool_count` | **5** | A9 (os docs dizem 4) |

Leitura: o doc é encontravel **geometricamente** e por `explain` (key exata), mas
não por texto, não por escopo e não por entidade — apesar de o request ter
pedido os três. Isso é a definição operacional de "camada entregue, contrato
não".

## 4. O que eu (LLM) preciso vs. o que existe

| Necessidade real | Onde vive hoje | Lacuna |
|---|---|---|
| Fatos do projeto | L3 (texto) + entities `adr/*`, `doc/*` | ok (lexical + entities) |
| Restrições latentes | L4/L3 + entity `mom/constraint` | ok (o paging sobe constraints) |
| Episódio verbatim / citação | L2 (`user=`+`response=`) | ok (lexical + `diary`) |
| Preferência do usuário | L3/L4 + entity `pref/*` | a camada L7 "preference" é decorativa |
| **Know-how / "como fazer X"** | nenhuma casa | L5 é inalcançável; vira fato L3/L4 |
| **Identidade/persona do agente** | regras da IDE, **fora do DB** | L7 é inalcançável; `profile` não cobre L7 |
| Auditoria "de onde veio este hit" | `explain` (uma key) | `Hit` não traz `layer` |
| "só memória procedural/identidade" | — | sem filtro por camada |
| Provar que as camadas são usadas | — | sem contagem por camada |

## 5. Melhoras propostas (priorizadas)

**M0 — corrigir A3/A4/A5 (bug de contrato, sem mudar schema).** No ramo
`author=system`: (a) gravar o companion L2 quando `text` não vazio (o `payload`
segue f32, preservando `corpus_mean`/rescore — não trocar o payload por texto);
(b) aplicar `scope`/`entities`/`type`/`model_id` via `finish_remember`-style,
ou **recusar em voz alta** os params que não serão honrados (a doutrina da casa:
não fingir). Custo: S. É a mesma classe do `index_key` fixo do 1.4.4.

**M1 — `layer` no `Hit` + filtro `layers=[...]` no `recall`.** O `Hit` é
explicitamente **não-wire** (`docs/api.md`), então é aditivo e barato; o filtro
roda no mesmo ponto onde o scope já filtra o pool. Habilita "só procedural" /
"só identidade" e torna o `type_scores` verificável (o consumidor vê a camada
que gerou o score). Custo: S–M + bump de contrato.

**M2 — `health.layer_distribution`** (`{L0..L7: count}` + lista de camadas
vazias). `scan_prefix("md/L{n}/")` já existe (usado em `profile`/`diary`);
8 scans baratos, ou um pass sobre o ART. Sem isso, "as camadas estão sendo
usadas" é indemonstrável — e um DB que virou 100% L3 é invisível. Custo: S.

**M3 — preferência/identidade de primeira classe para o agente.** Duas opções
não exclusivas: (a) aceitar `author=agent` + `layer=L7` **quando** `entities`
contém `pref/*` ou `identity/*` (a camada deixa de ser inalcançável sem abrir a
porteira); (b) incluir L7 no `profile` e expor `remember(pref=slug)`. Ativa o
eixo `preference` de verdade e faz o merge policy `ControlledLww` de L7 ser
exercido por memória de agente. Custo: S–M.

**M4 — abrir a escada de promoção ao host.** `curate op=lifecycle_tick` com
config explícita (ou `promote(key, to_layer=)`), reaproveitando
`MemoryLifecycle`/`transfer_to`. O host passa a *dirigir* a evolução L1→L2→L3→L4
sem o core decidir sozinho — coerente com "o core reporta, o host decide".
Custo: M.

**M5 — `procedural` de verdade.** Hoje `procedural` só vale 1.0 em L5, que é
inalcançável. Alternativa barata e coerente com ADR-0016: derivar `procedural`
também de entity `proc/*`/`howto/*` (mesmo padrão já usado por `pref/*`), ou dar
caminho de L5 ao agente via M3. Custo: S.

**M7 — fonte única de "camada de uma key".** Um `layer_of(key)` (ou usar
`doc.layer` direto nos dois sítios) mata A10 e a divergência de L6 entre as duas
tabelas de importância. Custo: XS — é o mesmo movimento do
`payload_content_type`/`key_carries_embedding` no v1.1.25.

**M6 — fechar A9 e a ambiguidade.** Corrigir `mcp_tool_count=4` → 5 nos dois
docs; declarar na doutrina que **L0/L1 não são recuperáveis** e que
`preference`/`procedural` hoje só são exercíveis via entity (ou via M3/M5).
Custo: XS.

## 6. Não-fazer (coerência com os non-goals)

- Não fazer o core extrair entidade/relação de texto para "preencher" L5/L6/L7 —
  contraria ADR-0008 e a doutrina ("quem fornece usa as mesmas strings").
- Não introduzir inferência de grafo no L6, nem LLM/embedding no core.
- Não persistir `layer` de novo em lugar nenhum: `type_scores` continua
  derivado; M1 só **expõe** o que já existe.
- Não trocar o payload f32 por texto para "resolver" A4 — quebraria `corpus_mean`
  e o rescore (a correção certa é o companion, não o payload).

## 7. Estado em 2026-10-21 — A3/A4 corrigidos (v1.4.6)

O caminho `author=system` virou `Sgdb::remember_operational(layer, key, text,
emb, opts)`:

- **A4 (texto perdido em L4/L5)** — resolvido **pelo companion `/L2/`**, como
  este documento já dizia ser a correção certa: o payload continua sendo o
  vetor (corpus_mean/rescore intactos) e o texto tem casa. `text_of`, `Hit.text`
  e o lexical voltam a enxergar a escrita operacional.
- **A3 (parâmetros descartados em silêncio)** — resolvido pela mesma
  `finish_remember` da escrita cognitiva: scope/scope_dims/entities/
  content_type/model_id/index_key são aplicados e o `RememberOutcome` devolve o
  que foi GRAVADO. `embedding` em camada de payload-texto agora é recusado, e o
  era guard do BQ passou a valer também aqui.
- `A8` (zero observabilidade por camada), `M1`, `M5`, `M7` seguem em aberto.

Regressão: `remember_operational_l4_keeps_text_and_metadata`,
`remember_operational_refuses_layer_and_era_mismatch`,
`remember_operational_scoped_ram_layer_materializes_and_validates` (lib) e o
bloco novo do hot test (8 asserções MCP ponta a ponta).

## 8. Estado em 2026-10-26 — ativação por contexto + trajetória (v1.5.0)

Nada de camada nova; as duas entregas mudam **quem escolhe** e **o que volta
para o peso**:

- **A ativação por caminho (ADR-0020)** ataca o passo 2 do "mapa: definição →
  uso real": o agente tinha que escolher scope/entities a cada prompt mesmo
  quando o arquivo em foco já decidia metade. Trigger rules gravam o mapeamento
  uma vez (`sys/trigger/`, padrão glob → namespaces) e `recall(path=)` o ativa
  — e o recall por caminho é o **mesmo 1-hop de `recall_entities`** (mesma
  escala de `dist`, e o filtro de scope continua valendo). Ou seja: continua
  sendo L3/L4/L5 via entidade declarada; o que muda é que o host não precisa
  reconstruir a lista de entidades a cada arquivo aberto.
- **O aprendizado por trajetória (ADR-0019)** fecha a lacuna "o desfecho não
  volta para o peso": a trajetória vai **verbatim para L2**
  (`remember_episodic` — reforça a regra 9 da doutrina: episódios em L2 para o
  `consolidate` ter o que fundir) e o desfecho re-pondera importance **e**
  confidence da memória que foi usada (`feedback`) em vez de criar memória
  nova. Medido em `examples/trajectory_learning.rs`: SR 50% → 100%, com a
  sessão 2 acertando desde a primeira tarefa.

Regressão: `trigger_rules_are_deterministic_and_match_paths`,
`trigger_rules_persist_across_reopen` (lib), a fase nova do hot test
(`curate op=set_trigger`/`triggers`, `recall(path=)` casando só o caminho certo,
`path`+`entities` recusando, `forget_trigger` desativando) e o exit 0 do
`trajectory_learning`. `A8`, `M1`, `M5`, `M7` seguem em aberto.
