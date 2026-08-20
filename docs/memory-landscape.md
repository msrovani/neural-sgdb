# Memory landscape — benchmark de sistemas de memória persistente (2026-08)

Análise de projetos concorrentes de memória persistente para agentes de IA,
feita para extrair ideias copiáveis (licenças compatíveis: MIT / Apache-2.0;
o neural-sgdb é MIT OR Apache-2.0 — **ideias são copiáveis livremente**, código
exigiria atribuição). Fonte das melhorias v1.1.4 (itens 1–10, concluídos);
v1.1.5–v1.1.6 acrescentaram era guard e hits tipados — ver `CHANGELOG.md`.

## Panorama

| Projeto | Licença | Filosofia central | Stack |
|---|---|---|---|
| **mem0** | Apache-2.0 | "Extrai o que importa e descarta o resto" — LLM extrai fatos | SQL + vector + entidades |
| **mempalace** | MIT | Oposto: armazena TUDO verbatim (nada de sumarizar) | ChromaDB + SQLite |
| **Zep/Graphiti** | Apache-2.0 | Grafo de conhecimento temporal bi-tempo (fatos com janela de validade) | Neo4j + vector |
| **Letta (MemGPT)** | Apache-2.0 | Memória como OS: contexto=RAM, memória em blocos editáveis | Postgres + vector |
| **Supermemory** | MIT | Grafo de fatos + perfis de usuário + esquecimento automático | Engine próprio |
| **cognee** | Apache-2.0 | Ciclo `remember/recall/forget/improve` + grafo+vector híbrido | Kuzu+LanceDB+SQLite |

## O que cada um faz de melhor

### mem0 — multi-signal retrieval + escopo (multi-tenancy)
- **Scoping em 4 dimensões** (`user_id`/`agent_id`/`app_id`/`run_id`): isola
  memórias por usuário/agente/aplicação/sessão, com **filtro implícito**
  (busca sem scope não vaza de outros scopes).
- **Retrieval multi-sinal** (semântico + BM25 + entidade + temporal, fusionados
  por rank) — o neural já tem 2 dos 4 (BQ+FP32 e `recall_lexical`).
- **ADD-only extraction**: acumula fatos, não sobrescreve.

### mempalace — verbatim + organização espacial + agentes
- **Zero-sumarização**: guarda a conversa crua — "não deixe a IA decidir o que
  esquecer". LongMemEval 96.6% raw (0 API calls).
- **Estrutura espacial** (method of loci): wings (pessoas/projetos) → rooms
  (tópicos) → drawers (verbatim). Busca escopada por estrutura.
- **Diário por agente** + coordenação (logstream/handoff).

### Zep/Graphiti — grafo temporal bi-tempo
- **Fatos com janela de validade** (`valid_at`/`invalid_at` + `created_at`/
  `expired_at`): sistema-tempo vs mundo-tempo separados. Contradição **invalida,
  não apaga** — história preservada.
- **Episódios = proveniência**: todo fato derivado rastreia até o episódio cru.
- **3 camadas de grafo**: episódica → entidades semânticas → comunidades.

### Letta/MemGPT — memória em contexto, gerida pelo agente
- **Memory blocks** (persona/human/...): texto persistente e editável que fica
  **sempre no contexto** (~2000-5000 chars cada).
- **Hierarquia OS**: core memory (RAM) → recall (histórico pesquisável) →
  archival (longo prazo). O **agente decide** o que promover.
- **Sleep-time compute**: turnos em background para consolidar memória.
- **Blocos compartilhados entre agentes** (shared blocks).

### Supermemory — grafo vivo + perfis + esquecimento
- **User profiles** (~50ms): perfil auto-mantido (fatos estáveis + atividade
  recente) pronto para injetar no prompt.
- **Relações de fato**: `updates` (substitui), `extends` (enriquece), `derives`
  (infere).
- **Esquecimento automático**: temporal (expira), contradição (update vence),
  ruído (filtrado).
- `containerTag` = isolamento por projeto/usuário.

### cognee — ciclo de vida de memória + feedback
- API em 4 verbos: `remember`/`recall`/`forget`/**`improve`** — o `improve`
  re-pondera a memória pelo uso (feedback).
- 14 modos de retrieval (RAG clássico → CoT em grafo → temporal).
- `memify`: poda nós stale, fortalece conexões frequentes.

## O que o neural-sgdb JÁ tem (sobreposição)

- Camadas L0–L7 (≈ Mem0/Letta).
- `sys/validity` com invalidar-não-deletar (≈ Graphiti).
- `sys/rel/` L6 (≈ grafo raso).
- `recall_lexical` BM25 (≈ mem0).
- CRDT/mesh (≈ Letta shared blocks).
- Lifecycle de promoção L3→L4 (≈ mem0/letta).

## Roadmap por complexidade (aprovado 2026-08)

Status: itens **1–10 entregues em v1.1.4** (2026-08-14, commits por item com
regressão; na época: matriz 210+1 / 256+1 / 162+1; hot test 22 tools).
**Estado corrente (v1.1.6):** matriz 229+1 / 181+1 / 275+1; hot test 90/0;
MCP 23 tools (`era_report` + hits tipados).
Item 10 foi entregue no modo **custo baixo + risco baixo** (1-hop de
entidades): o multi-hop (grafo semântico completo) permanece fora de escopo.

| # | Item (origem) | Custo | Retorno | Status |
|---|---|---|---|---|
| 1 | Documentar ADD-only como recurso (mem0) | ~0 | clareza de contrato | ✅ v1.1.4 |
| 2 | Camada episódica explícita (mempalace) | 0.5 dia | antídoto à perda de contexto | ✅ `remember_episodic` |
| 3 | Feedback/`improve` (cognee) | 0.5–1 dia | memória melhora com uso | ✅ `feedback` |
| 4 | Diário por agente (mempalace) | 1 dia | legibilidade multi-agente | ✅ `diary` |
| 5 | Perfil agregado por scope (supermemory) | 1 dia | 1 chamada = contexto pronto | ✅ `profile` |
| 6 | Esquecimento temporal automático (supermemory) | 1–2 dias | memória envelhece sem crescer sem limite | ✅ `expire_old` |
| 7 | **Scoping multi-agente/projeto** (mem0) | 2–3 dias | **feature #1** (responde o modelo 1-pasta-por-projeto/agente) | ✅ `set_scope`/`recall_scoped` |
| 8 | Expor modos de retrieval existentes (cognee) | 2–3 dias | superfície rica sem tocar core | ✅ `mode` semantic/lexical/hybrid |
| 9 | Retrieval temporal com intenção (mem0/Graphiti) | 3–5 dias | "quando mudou X?" passa a funcionar | ✅ `recall_temporal` |
| 10 | Entidades + grafo semântico (Graphiti/cognee) | 1–2 semanas | multi-hop reasoning (maior risco) | ✅ 1-hop `recall_entities` (MDM1 v5) |

**Item 10 (modo 1-hop, 2026-08-14)**: entidades como **metadado explícito
fornecido pela camada superior** — `MemoryMeta.entities: Vec<String>` (MDM1
v5, migração explícita: v1–v4 decodificam com lista vazia). O core **nunca
extrai entidade de texto** (mesmo contrato do `Embedder`: quem fornece usa as
MESMAS strings na escrita e na busca). Índice derivado `entity_index`
(entidade → storage keys) reconstruído do `sys/meta/` no open e mantido por
`persist_meta`/`write_meta`; `delete` limpa. Superfície:
`set_entities`/`entities_of`/`recall_entities` (`_historical`/`_scoped`/
`_scoped_historical`), rank por overlap desc → importância desc → key asc.
MCP: `remember(entities=)` + tool `recall_entities`. **Multi-hop (grafo
semântico real, BFS/DFS em entidades, persistência de arestas) segue fora de
escopo** — custo 1–2 semanas, maior risco (ver discussão de custo/risco na
sessão).