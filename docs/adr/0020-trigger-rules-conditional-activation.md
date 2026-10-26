# ADR-0020 — Trigger rules: ativação condicional por caminho, declarada pelo host

**Data:** 2026-10-26 · **Status:** aceito · **Release:** v1.5.0

## Contexto

Num IDE o agente não decide o contexto do zero: o arquivo em foco já determina
metade da decisão. Editar `src/kernel/*.rs` deveria ativar as regras
`bare-metal`/`no_std`; mexer em `tests/*` deveria ativar os cenários de teste.
Frameworks de agente expõem isso como *hooks* que interceptam e consultam a
memória em segundo plano (Ruflo) — o host conhece o caminho, a memória conhece
o conteúdo.

O `recall` normal exige que o agente ESCOLHA o escopo/entities. Sem uma tabela
declarada, cada host reinventaria o mapeamento "caminho → namespaces" fora do
banco, e o banco perderia a única coisa que sabe fazer bem aqui: devolver o
material da decisão de forma determinística e auditável.

A tentação a evitar é o core **ler o arquivo** (ou inferir o namespace pelo
conteúdo). Isso contraria a doutrina (o core nunca extrai entidade de texto) e o
`no_std` (o core não tem filesystem).

## Decisão

Uma side-table `sys/trigger/<fnv1a64(padrão):016x>` guarda REGRAS DE ATIVAÇÃO
declaradas pelo host — `TriggerRule { pattern, namespaces }`, valor `TRG1`
versionado, **sem wire type novo** (mesma precedência de `sys/negative/`,
`sys/ttl/`, `sys/validity/`). O padrão é a IDENTIDADE da regra (mesmo padrão
reescreve; a key tem largura fixa de 16 hex — regra 4/ADR-0005, a ART não
suporta prefix-key).

- **Matching no core** (`src/trigger.rs`, zero-dep, `no_std`): glob `*` (não
  atravessa `/`), `**` (atravessa), `?` (um char, não `/`). Nada de regex.
- **Namespaces = entities**: quem escreve a regra usa as MESMAS strings que
  gravou na memória (mesmo contrato de `entities`/`Embedder`). A ativação é o
  1-hop que já existe — logo a MESMA escala de `dist`, sem uma terceira.
- **API**: `set_trigger`/`forget_trigger`/`trigger_rules`/`triggers_for_path`/
  `namespaces_for_path`/`recall_for_path`. Lista determinística (padrão asc —
  o scan do storage vem por hash, e ordem é contrato).
- **MCP**: `curate op=set_trigger|forget_trigger|triggers` (a regra é
  configuração) + `recall(path=…)` (a ativação). `path=` é o caminho de
  ENTIDADES: com `entities=` junto, ou com `embedding=`/`mode=semantic|hybrid|
  causal`, a chamada **falha em voz alta** em vez de ignorar o vetor em silêncio.
- **Null-scoping intacto**: a ativação por caminho roda no pool do 1-hop com o
  filtro de escopo ligado. Memória de outro escopo nunca vira evidência por
  causa de um caminho.

## Consequências

- **Positivo**: o host ganha o *hook* sem reinventar tabela fora do banco; a
  regra é auditável (`curate op=triggers`), persistente (sobrevive ao reopen) e
  recusa regra sem efeito (namespace vazio, padrão com `..`, controle/NUL).
  `recall(path=)` não é um recall sem filtro: sem regra que case devolve vazio
  **com hint**, e distinguir "nenhuma regra casou" de "as regras casaram e não
  havia memória" é parte da resposta (`activated` + `namespaces` ecoados).
- **Negativo / custo**: mais um `sys/*` para o `validate`/backup conhecerem, e
  a superfície MCP ganha três ops + um argumento (contrato precisa subir).
  O core não pode validar que os namespaces apontam para memórias existentes
  (eles são strings declaradas) — a regra pode "casar" e devolver vazio.
- **Contra a tentação**: nenhuma leitura de arquivo, nenhum watcher, nenhuma
  inferência de namespace pelo conteúdo. O host diz onde está; o core diz o que
  ativar. A exploração/uso disso continua sendo política da camada superior.
- **Contrato**: MINOR (novo `sys/*` é explicitamente MINOR em `VERSIONING.md`);
  NMD1/TKLV intocados; `MCP_CONTRACT_VERSION` sobe junto do release.
