# Triagem do relatório do consumidor #2 (uso real via MCP, 6 papers)

**Fonte:** relatório de um agente AI que fez ingestão real de 6 papers técnicos
(86 memórias, 6 lotes) e verificação de integridade por entities — colado no
chat em 2026-10-08. Relatório de alta qualidade: cada alegação foi checada
contra o código **1.4.5** antes do veredito. Vereditos: **ACEITO** (implementar),
**ADIADO** (roadmap), **DESLOCADO** (é do lado do host), **REJEITADO** (colide
com contrato/design intencional).

Data da triagem: 2026-10-08. Base: crate 1.4.5 (HEAD `1980f94`).

**Status (2026-10-08, mesmo dia): IMPLEMENTADO** — as issues 1/2/3/4/5/7 foram
CORRIGIDAS em `src/sgdb.rs` + `examples/mcp_server.rs`, cada uma com teste que
MORRA sem o fix; contrato MCP 1.4.5 → **1.4.6** (pins: `contract.json`,
serverInfo, `EXPECTED_CONTRACT` do conector, `docs/MCP.md`); hot test
**185/0** (+8 asserções da fase "triagem consumidor #2"); gate completo
verde (clippy/lib/lib-serial/p2p/no_std/integration/goldens/wire-fuzz/rustdoc/
target-none/host-crates/wasm32/connectors/hot-test). ISSUE 6 permanece
REJEITADO (design ADR-0008); o hint ADIADO segue em roadmap.

Veredito em resumo: **7 alegações, 6 confirmadas no código, 1 rejeitada por
design (com pedido parcial aceito).** As duas primeiras são P0 — uma delas já
causou perda de conteúdo no uso real.

---

## Bloco 1 — Crítico (perda de dados)

### ISSUE 1 — `supersede` meio-mutante reporta sucesso — **ACEITO (P0)**

**Confirmado.** A mutação acontece em duas metades e só a primeira pode falhar:

- `src/sgdb.rs:1590-1605` — ordem atual: `set_state(old, Superseded)?` PRIMEIRO,
  depois `set_state(new, Active)?`. Se `old` existe e `new` não, a primeira
  mutação já gravou e a segunda é um no-op que **nunca** retorna erro
  (`src/engine.rs:1765-1788`: `Active` é remove-only — o comentário no próprio
  código admite "supersede marca new→Active antes do new existir"). Resultado:
  `Ok(())`, old escondido do recall default, new não existe em lugar nenhum.
- O único teste de ghost cobre o ghost **antigo**
  (`src/sgdb.rs:8088` — `supersede(ghost-old, ghost-new)` erra na PRIMEIRA
  chamada). O caso real do usuário (old existe, new não) não tem teste — e
  `examples/audit.rs:248` **pinou a meia-mutação como design**
  (`db.supersede("md/L4/f2", "md/L4/f3").unwrap(); // f3 ainda não existe`).
- Handler MCP (`examples/mcp_server.rs:2793-2806`): devolve
  `"{old} superseded por {new}"` ecoando o parâmetro `new` VERBATIM — o usuário
  passou o **texto** da memória nova ("WATCHDOG RESILIENTE v2") e a resposta
  leu-se como sucesso. Sem `structuredContent`, sem checagem de existência.
- Schema (`examples/mcp_server.rs:932-933`): `old`/`new` são
  `{"type":"string"}` sem descrição — nada diz "storage key de memória
  EXISTENTE". A descrição global do `curate` só diz "use uma storage key
  completa md/L4/…" genérico.

**Plano (em duas camadas):**
1. **Core:** pré-validar que AMBAS as keys resolvem para doc existente ANTES da
   primeira mutação (ordem: validar → mutar). `new` fantasma →
   `SgdbError::Invalid` estático. Atualizar `examples/audit.rs:248-251` (o pin
   do design antigo) e adicionar o teste que morre sem o fix:
   `supersede(existing_old, ghost_new)` → Err E old NÃO mudou de estado.
2. **MCP:** o handler valida primeiro e devolve erro acionável ("new não existe
   — grave a memória nova com `remember` e passe a storage key"); no sucesso,
   `structuredContent: {old, new}` com as KEYS (não o texto ecoado).
3. Schema: descrição de `old`/`new` = "storage key completa de memória
   existente".

### ISSUE 2 — Key com prefixo duplicado (`md/L3/md/L3/…`) — **ACEITO (P0)**

**Confirmado.** A montagem da storage key prepende a camada às cegas:

- `src/engine.rs:1114` e `src/engine.rs:1184` +
  `src/memory_doc.rs:994`: `format!("md/{layer}/{key}")` — nenhum checa se `key`
  já começa com `md/`. Passei `md/L3/mcp/x` → gravou `md/L3/md/L3/mcp/x`.
- `validate_written` (`src/sgdb.rs:6175`) rejeita `#`, controle, `..`/`.`,
  mas **não** rejeita componente `md` nem prefixo `md/` — a validação óbvia
  não existe.
- Schema do `remember` (`examples/mcp_server.rs:856`): "key explicita opt-in" —
  não diz "key CRUA, sem prefixo". A doutrina regra 4 fala em "full storage key
  (md/L4/…)" — mas no contexto de **curate** (follow-ups). Duas convenções com
  o mesmo nome, exatamente como o relatório descreve.

**Plano:** `validate_written` rejeita key que começa com `md/` ou `sys/`
(componente 0) com mensagem estática acionável ("key é CRUA — o servidor monta
md/L{N}/<key>; para follow-ups use a storage key completa no curate"). Schema
do `remember.key` ganha "CRUA (sem md/L3/)". Doutrina: separar "key no
remember" (crua) de "key no curate" (completa).

---

## Bloco 2 — Verificabilidade

### ISSUE 3 — Truncamento sem sinal (entities/temporal sem cursor) — **ACEITO (M)**

**Confirmado, com precisão parcial:**
- `recall` comum JÁ sinaliza: sentinela `need = off+size+1`
  (`examples/mcp_server.rs:2064-2075`) + `nextCursor` no result
  (linha 2140). "20 de 65" é distinguível ali — mas só lendo o campo
  top-level `nextCursor`; `structuredContent.hit_count` é o tamanho da página,
  sem `total`/`truncated`.
- `recall_entities` (`examples/mcp_server.rs:2312-2347`) e `recall_temporal`
  (2264-2311): **não** têm cursor, não têm total, não têm flag de corte —
  `k` (teto 20 no schema, linha 885) é tudo que existe. Exatamente a dor
  relatada (21 memórias → 20 sem jeito de saber).

**Plano:** sentinela `k+1` + `truncated: bool` + `nextCursor` em
`recall_entities`/`recall_temporal`; `total` (candidatos do pool) e
`truncated` no `structuredContent` do `recall`.

### ISSUE 4 — `structuredContent` inconsistente no MESMO `recall` — **ACEITO (M)**

**Confirmado, e a causa exata é a que o relatório pressupôs:** o handler
`recall` usa `mcp_tool_result` (sempre `structuredContent`,
`examples/mcp_server.rs:993-999`), mas o routing em `expand_tool`
(linha 811-813) manda `recall`+`entities[]` para o handler `recall_entities`,
que monta resposta **só com `content[0].text`** (linhas 2340-2351) — inclusive
com `format=json`. Mesma tool, dois shapes, dependendo dos argumentos. O mesmo
vale para `recall_temporal` (2296-2307). Das 118 respostas `send(...)`, só 14
passam por `mcp_tool_result`.

**Plano:** toda a família recall (entities, temporal, ledger, absences) e o
`curate` respondem via `mcp_tool_result` — nunca `content` só (regra
"anunciado == servida" aplicada ao shape, não só ao enum).

### ISSUE 5 — Vazio é prosa mesmo com `format=json` — **ACEITO (S)**

**Confirmado:** em `recall_entities`, o branch vazio
(`Ok(hs) if hs.is_empty()`, linha 2340) retorna
`"nenhuma memoria com essas entidades"` **antes** de olhar `json_fmt` — o
format=json só é checado no branch não-vazio. Idem `recall_temporal`
("nenhuma memoria valida em at", linha 2296). O `recall` comum faz certo
(branch `json_fmt` primeiro, linha 2108) — por isso o parser quebrou num tool
e não no outro.

**Plano:** inverter a ordem: `json_fmt` primeiro; vazio → texto `"[]"` +
`structuredContent {hits: [], …}`. Regra de casa: **toda resposta a pedido
`format=json` é JSON, inclusive vazia.**

---

## Bloco 3 — Higiene

### ISSUE 6 — Embedder desligado por default — **REJEITADO (design)** / pedido parcial **ADIADO**

O default lexical sem vetor é o ADR-0008 intencional (não posar `demo` como
semântico; "GPU" não achar "placa de vídeo" é a consequência documentada de
`lexical`). O one-liner que o relatório pede **já existe**: `docs/doctrine.md:3`
(`NEURAL_SGDB_EMBEDDER=multilingual` → paraphrase-multilingual-MiniLM-L12-v2,
384-dim, in-process via `crates/nsgdb-embed`), e `health.semantic_ready` /
`retrieval_default` (v1.3.0) já declaram o estado. **ADIADO:** quando o default
efetivo é `lexical` e há texto no corpus, o `health` (ou o hint de recall vazio)
sugere o one-liner — o agente que não leu a doutrina descobre na chamada.

### ISSUE 7 — Sem op de higiene/duplicatas — **ACEITO (M, feature nova)**

**Confirmado:** `if_exists` default `add` (`examples/mcp_server.rs:870`) —
rodar o mesmo lote duas vezes gera N duplicatas (o probe age só sobre
equivalente E o default não substitui). Não existe nenhuma op que reporte as
quatro classes pedidas: superseded sem substituta, keys malformadas
(`md/*/md/…`), entities órfãs, texto duplicado (mesmo texto + mesmas entities).
As ops `audit_*` existentes são a hash-chain (integridade), não higiene de
conteúdo.

**Plano:** `curate op=hygiene` (ou extensão de `health view=validate`) com
relatório estruturado:
- `superseded_without_successor`: estado `Superseded` cujo `version_id` não
  aparece em nenhum `parent_ids` de doc ativo;
- `malformed_keys`: storage keys com componente `md` duplicado (o check da
  ISSUE 2 no read-side também);
- `orphan_entities`: entrada de `entity_index` apontando para doc ausente;
- `duplicate_text`: normalização BM25 + mesmas entities (o mesmo probe do
  `if_exists`, rodado de trás p/ frente — read-only, REPORTA, não decide —
  mesma postura do `stale_candidates`).

---

## O que funcionou (para o registro)

Confirmado pelo relatório e coerente com os testes existentes: lote
`remember(memories[])` (teto 64) — 6 round-trips para 86 memórias; resolução de
conflicto em tempo de recuperação (v1/v2 sem overwrite, v2 no topo); recall
lexical como default de verificação; entities como índice de proveniência
bateram com o corpus (6 papers → 6 proveniências).

## Lições de processo (as do próprio relatório — valem para os próximos reportes)

Duas fases em toda mutação (gravar → recall → assert), tools/list antes da
primeira mutação, JSON gerado e não escrito à mão, manifesto de keys,
contagem esperada registrada na escrita, assert sobre estado externo. O padrão
comum a quase toda dor relatada: **o servidor guardou o erro para si** — o
P0 do supersede é o caso extremo (metade da mutação + mensagem de sucesso).

## Matriz de verificação (reproduzível)

| # | Alegação | Evidência | Veredito |
|---|----------|-----------|----------|
| 1 | supersede meio-mutante com sucesso | `src/sgdb.rs:1590`, `src/engine.rs:1765`, `examples/mcp_server.rs:2793`, `src/sgdb.rs:8088`, `examples/audit.rs:248` | ACEITO (P0) |
| 2 | prefixo `md/L3/` duplicado | `src/engine.rs:1114,1184`, `src/memory_doc.rs:994`, `src/sgdb.rs:6175`, `examples/mcp_server.rs:856` | ACEITO (P0) |
| 3 | truncamento sem sinal | `examples/mcp_server.rs:885,2071,2140` (ok) vs `2264-2351` (sem cursor/total) | ACEITO (M) |
| 4 | `structuredContent` inconsistente | `examples/mcp_server.rs:993,811-813,2340-2351` | ACEITO (M) |
| 5 | vazio = prosa com format=json | `examples/mcp_server.rs:2340,2296` vs `2108` | ACEITO (S) |
| 6 | sem embedder = lexical | ADR-0008, `docs/doctrine.md:1,3` | REJEITADO (design) + ADIADO (hint) |
| 7 | sem dedup/higiene | `examples/mcp_server.rs:870`; enum de ops sem higiene | ACEITO (M) |
