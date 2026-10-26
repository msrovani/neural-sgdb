# Migration Guide — neural-sgdb

This file describes how binary formats and on-disk state evolve between
releases. It complements `docs/api.md` §Format versioning and
`VERSIONING.md`.

## Golden rule

**Never silently reinterpret old bytes.** Every format change must be: (1)
backward-decodable (old bytes still decode), (2) covered by a golden test
updated in the same commit, and (3) documented here. When a field is absent
in an old version, decode to a defined default — never guess.

## MCP contract changes (non-binary, but breaking for consumers)

| Contract | Change | Migration |
|---|---|---|
| 1.4.6 → 1.5.0 | `curate` ganha `op=set_trigger` (`pattern`,`namespaces`)/`forget_trigger` (`pattern`)/`triggers` (`path` opcional) e o `recall` ganha `path=` (ativação por caminho, ADR-0020) | Aditivo: quem não usa as ops novas não vê diferença. `recall(path=)` é **exclusivo** com `entities=`/`embedding=`/`mode=semantic\|hybrid\|causal` (recusa em voz alta em vez de ignorar o argumento) e, sem regra que case com o caminho, devolve **vazio** — não cai para o pool global (o null-scoping continua valendo). |
| 1.4.5 → 1.4.6 | **P0**: `curate(op=supersede)`/`Sgdb::supersede` é ATÔMICO — `new` precisa EXISTIR (validado ANTES de mutar; sucesso devolve `structuredContent {old,new,state}` em vez do texto ecoado). `remember(key=)` recusa key iniciada em `md/` (namespace composto pelo servidor; antes gravava `md/L3/md/L3/...`). `recall`+`entities[]`/`at=` passam a devolver `structuredContent` SEMPRE, com `truncated` + `nextCursor` (antes: só `content[0].text`, sem cursor — corte invisível) e `format=json` responde `[]` no vazio (antes: prosa); `recall` comum ganha `truncated`. Erros do `recall`/`rag_context` ganham `structuredContent`. Novo `curate(op=hygiene)`: relatório READ-ONLY (superseded sem sucessora, keys malformadas, entities órfãs, texto duplicado). | Quem passava texto como `new` ou key `md/...` recebia "sucesso" silencioso e perdia conteúdo — agora erro acionável (`remember` primeiro, depois `supersede`). Parsers de vazio em `format=json` deixam de quebrar (`[]`); `truncated`/`nextCursor` são aditivos. `hygiene` é op nova (adicional). |
| 1.4.2 → 1.4.3 | `decide(evidence_sufficient)` com 0 hits → `sufficient:false` (era `true` por fronteira vazia vacuous — decidir no vácuo). `expire_ttl` roda no OPEN da sessão: TTL vencido some automaticamente na próxima conexão do mcp_server (a consulta já reflete a expiração) | Quem decidia com `sufficient:true` e 0 hits passa a ver `false` (o correto). Quem confiava em TTL "pendente" na próxima sessão: agora é honrado no open. |
| 1.4.1 → 1.4.2 | `curate(op=set_ttl)` agora honra `expires_at` absoluto (ms). Antes lia `seq`/`now` (herança do `rollback_to`) e ignorava `expires_at` → TTL=0 silencioso (no-op) | Quem passava `expires_at` tinha no-op; agora o deadline é cumprido. `now` continua como fallback (expirar imediatamente). |
| 1.4.0 → 1.4.1 | contrato = build marker (F4 probe + docs; nenhuma mudança de superfície) | Nada. |
| 1.3.1 → 1.4.0 | `recall_graph` + `recall_provenance_tiebreak` na lib; `RecallPath::Graph`; nenhuma superfície MCP nova | Nada (adicional). |
| 1.3.0 → 1.3.1 | `tensions.unseen_scopes` agora é **só legacy** (a versão 1.3.0 ainda vazava labels de dims `///x`); dims vão em `unseen_scope_dims` (`{label,display,user,agent,app,run,count}`); `scope_dim_labels`/`scopes_to_probe_dims` ganharam `display` (ex. `run=release-v1.1.24`); `recall_empty_hint` legível | Quem lia `unseen_scopes` para dims passe a ler `unseen_scope_dims`; `display` é a forma humana, `label` a chave canônica. |
| 1.2.1 → 1.3.0 | `recall`/`rag_context` default is `hybrid` when a **real** host embedder is set (was lexical unless the caller passed `embedding`); `health.scope_labels` is **legacy-only** (multi-dim now in `scope_dim_labels` / `scopes_to_probe_dims` fields); `contract.json` + alias `stable`/`deprecated` classification; `semantic_ready`/`retrieval_default` fields; resource `nsgdb://contract` | If you depended on lexical-when-embedder, pass `mode=lexical`. Read multi-dim scopes from the structured fields, not the old merged `scope_labels`. Deprecated aliases keep working for 2 releases. |

## Format registry

| Format | Version | Encode/decode | Golden test | Lives in |
|--------|---------|---------------|-------------|----------|
| NMD1 | v1 (stable) | `MemoryDoc` | `golden_nmd1_bytes` | `src/memory_doc.rs` |
| MDM1 | **v8** (corrente) | side-table meta codec — ver §"MDM1 — evolução dos campos" | `golden_mdm1_v8_layout_pins_authority_as_last_field`, `meta_v8_authority_roundtrip_and_v7_migration`, `meta_v1_decodes_with_slot_version_migration` | `src/memory_doc.rs` |
| TKLV / TKCK | v1 (stable) | `TickvFile` | `golden_record_bytes` | `src/tickv.rs` |
| FNV-1a 64 | — | checksum | `fnv1a64_known_vector` | `src/fnv1a64.rs` (or engine) |
| CRDT state | "CRDT" | `CrdtState` | bounds-checked decode | `src/crdt.rs` |
| MDR1 | v1 | `MemoryRecord` | bounds-checked decode | `src/memory_doc.rs` |
| CFL1 | v1 | `ConflictRecord` | bounds-checked decode | `src/conflict.rs` |
| MDLT / MSNP | v1 | `MemoryDelta` / `MemorySnapshot` | bounds-checked decode | `src/crdt.rs` |
| AUD1 | v1 | `AuditEntry` (hash-chain ledger) | `src/audit.rs` + `wire_fuzz` | `src/audit.rs` |
| "2222" | v1 | `SignedEnvelope` (transporte assinado; **sem** magic — corrupto por field length) | `src/wire_fuzz.rs` | `src/trust.rs` |

O harness único de fuzz (`src/wire_fuzz.rs`) cobre **todos** os tipos wire —
adicionar um tipo novo lá (mais o `prop_tests` do próprio módulo) é parte de
integrar o tipo, não um extra.

## MDM1 — evolução dos campos do side-table meta (fonte única)

`MDM1` é o codec de `sys/meta/` (proveniência, escopo, era, autoridade). O
**formato corrente é v8**. O byte de versão vive no offset 4 (magic `MDM1` em
0..4) e `decode` aceita **todas as oito** versões — sempre com migração
explícita, nunca reinterpretando bytes antigos.

Esta tabela é a **fonte única** dos rótulos `[vN]` que aparecem espalhados
(registry de formatos, registro de side-tables, `docs/api.md`): qual versão
introduziu cada campo, em que release, e o que um registro mais antigo
decodifica. As seções de "Known migrations" abaixo guardam o detalhe e as
armadilhas de `off`; se alguma delas divergir daqui, **esta tabela ganha** e o
texto deve ser corrigido.

| Ver | Release (origem) | Campo(s) adicionados | Decode default (< vN) | No wire v8, na ordem |
|-----|------------------|----------------------|-----------------------|----------------------|
| **v1** | v0.6 | `memory_id`, `source`, `confidence`, `importance`, `created_tick`, `parent_ids`, `clock_overflow` | — (piso; v1 é a base) | `magic` · `u8 ver` · `u16+bytes memory_id` · `u8 source` · `f32 confidence` · `f32 importance` · `u64 created_tick` · `u16 n + n×(u16+bytes) parent_ids` · `u16 n + n×(u8+u64) clock_overflow` |
| **v2** | v0.7 | `version_id` (identidade por versão) | `version_id = memory_id` | `u16+bytes version_id` |
| **v3** | v0.9 | `last_reinforced` | `0` (nunca reforçada) | `u64 last_reinforced` |
| **v4** | v1.1.4 (item 7) | `scope` (legado, multi-tenant) | `""` (global) | `u16+bytes scope` |
| **v5** | v1.1.4 (item 10) | `entities` (1-hop declarado) | lista vazia | `u16 n + n×(u16+bytes)` |
| **v6** | v1.1.6 (item 2) | `content_type` (rótulo declarado) | `None` (detector decide) | `u16+bytes` (len 0 = `None`) |
| **v7** | v1.1.14 (ADR-0007) | `scope_dims{user,agent,app,run}` + `model_id` | dims vazias + `""`; `scope != ""` legado promove `user` | `4×(u16+bytes)` + `u16+bytes model_id` |
| **v8** | 1.2.2 (triagem s413) — **wire**; setter público em 1.4.4 | `authority` (0 = learned, 255 = HITL) | `0` (learned) | `u8 authority` (último byte, sem comprimento) |

Três regras que a tabela condensa (detalhe nas seções abaixo):

- **Todo campo avança `off`, mesmo ausente.** Os bugs reais do v4/v5/v6 foram
  exatamente ler do offset errado por não avançar após o último campo da versão
  anterior.
- **`scope` (v4) e `scope_dims` (v7) coexistem no wire**, mas `scope_dims` é
  autoritativo (ADR-0013): na leitura de v1–v6, `scope != ""` sem dims promove
  `user`; o byte legado continua lá, sem reinterpretação.
- **Bump de MDM1 não toca o NMD1 nem o TKLV/TKCK.** A meta vive só em
  `sys/meta/` e viaja no MDR1 na replicação.

## NMD1 (document) — stays v1, byte-identical to neural-os-core

The OS interop contract. NMD1 NEVER changes without a joint bump in
`neural-os-core`. New metadata is added via **side-tables**, not in-record:

| Side-table | Purpose | Since |
|------------|---------|-------|
| `sys/state/` | `MemoryState` (Active/Superseded/Archived/Invalidated/Decayed) | v0.2 |
| `sys/validity/` | temporal `from|until u64le` window (invalidate-not-delete) | v0.2 |
| `sys/meta/` | `MemoryMeta` — v1 base + version_id [v2], last_reinforced [v3], scope [v4], entities [v5], content_type [v6], scope_dims + model_id [v7], authority [v8]. **Rótulos: fonte única em §"MDM1 — evolução dos campos".** | v0.6 |
| `sys/version/` | per-version identity reverse index | v0.7 |
| `sys/rel/` | L6 relations (`<kind>/<a>#<b>`) + derived ART fwd/rev | v0.8 |
| `sys/conflict/` | `ConflictRecord` (MDR1 evidence per candidate) | v0.9 |
| `sys/crdt/` | durable `CrdtState` (opt-in) | v0.7 |
| `sys/audit/` | hash-chain ledger (`AUD1`, `sys/audit/<seq:016x>`) + snapshot for `rollback_to` | v1.1.10 |
| `sys/ttl/` | TTL per-key → 8B `expires_at u64le` (host-driven sweep `expire_ttl`) | v1.1.15 |
| `sys/event/` | evento temporal (start/close) → timeline `recall_timeline` | v1.1.15 |
| `sys/negative/` | ledger de negativos: `NDG1` (probes/first/last/scope/query) por `fnv1a64(scope‖0x1f‖query)` — "o que já procurei e não estava lá" | v1.1.24 |
| `sys/trigger/` | regras de ativação por caminho: `TRG1` (pattern + namespaces) por `fnv1a64(pattern)` — "em que arquivo o cursor está ⇒ quais namespaces ativar"; CONFIG do host, não memória | v1.5.0 |

## v1.5.0 — sem bump de formato (side-table nova `sys/trigger/`)

O v1.5.0 **não muda nenhum formato**: NMD1/TKLV/MDM1 (v8)/AUD1 intocados.

- Trigger rules (ADR-0020) são uma **side-table derivada**: chave
  `sys/trigger/<fnv1a64(pattern):016x>` (largura FIXA de 16 hex, porque a ART
  não suporta chave que é prefixo de outra — regra 4) e valor `TRG1`, que se
  auto-descreve (`magic TRG1` + `u8` de versão do valor) para que um leitor de
  formato errado devolva `None` em vez de reinterpretar bytes. **Não é wire
  type**: fica fora do registry de formatos e do `wire_fuzz`, e o decode é
  bounds-checked em todo campo (truncado/corrompido/namespace vazio → `None`).
- A convenção de aprendizado por trajetória (ADR-0019) é **documentação de
  host** — nenhum byte novo, nenhuma API nova: usa `remember_episodic` (L2) e
  `feedback` (side-table `sys/meta/`, já existente).

## v1.2.1 — sem bump de formato (otimizações + fork/merge)

O v1.2.1 **não muda nenhum formato**: NMD1/TKLV/MDM1 intocados.
- `promote_run` escreve memórias novas por caminhos existentes (re-escopadas,
  run vazio) — nenhum byte antigo reinterpretado.
- Índices reversos (`sk_clocks`, `sk_ids`) e `remove_entities_exact` são
  estado derivado em RAM — reconstruídos no `rebuild_indices`, nunca
  persistidos.
- TickvFile buffered (`open_buffered`) é opt-in do host; o layout TKLV no
  disco é idêntico (flush antes de invalidate/checkpoint/compact).

## Known migrations

O **mapa autoritativo de versão → campo é a tabela de §"MDM1 — evolução dos
campos"** (acima). As seções abaixo são o detalhe de cada transição e das
armadilhas de `off`.

### Sem bump no v1.1.24 (MDM1 permanece v7 — canonicalização de escrita)

O v1.1.24 (ADR-0013) torna `ScopeDims` **autoritativo** e o `scope` legado um
espelho de `scope_dims.user`: `set_scope` passa a escrever os DOIS campos
(write-through), então os bytes de `sys/meta/` ficam canônicos. **Nenhuma
mudança de layout, nenhuma mudança de versão, nenhum byte reinterpretado** — a
regra de promoção do decode (legado → `user` quando as dims vêm vazias)
continua valendo para registros pré-v7 e para payloads de peer. Um leitor que
decodifica os campos sem essa promoção passa a ver os dois preenchidos (antes
via `scope` setado e dims vazias).

### Sem bump no v1.1.26 (correção de classificação + invariante de escrita)

O v1.1.26 (ADR-0015) **não muda formato**: NMD1/TKLV intocados, MDM1 sem bump,
nenhum byte reinterpretado. Duas mudanças de *valor* que um consumidor sente:

1. **`scope_distribution()` / `global_memory_count` / `scope_labels`.** Global
   passou a ser `scope == ""` **e** `scope_dims.is_global()` (era só
   `scope.is_empty()`). Uma memória gravada com `scope_run`/`agent`/`app` sem
   `user` deixa de ser contada como global e passa a aparecer em `scope_labels`
   sob o rótulo de dims (`///run-x`). Corpus escrito só com escopos de `user`
   (o caso dos conectores: `tenant/x/agent/y/workspace/z`) **não muda** — as
   labels legadas são as mesmas.
2. **`scope == scope_dims.user` na escrita com AMBOS os escopos.** Quem gravava
   `scope="proj/x"` **e** `scope_run="r1"` obtinha `dims.user == ""` (o
   `set_scope_dims` clobberava o write-through do `set_scope`); a dim `user` era
   perdida nos filtros. Agora `dims.user` é preenchido com o `scope`. Um corpus
   gravado pelo caminho antigo tem metas com a divergência: `validate()` §6
   reporta, e reescrever o escopo (`set_scope` + `set_scope_dims`, ou a
   `scope`+dims numa nova escrita) reconcilia. Não é preciso migrar para ler.

Também **novo** (aditivo): `Sgdb::scope_probes()`, `ScopeProbes`, `ScopeDims::label()`,
`scopes_to_probe_dims` no `nsgdb://session`, e o `recall` do MCP honrando
`scope_user/agent/app/run` (recusando em `hybrid`/`temporal`).

### MDM1 v1 → v2 (v0.7)

Adds `version_id` (per-version identity). v1 records decode with
`version_id = memory_id` — **explicit migration, never silent**. NMD1 and
TKLV/TKCK unchanged.

### MDM1 v2 → v3 (v0.9)

Adds `last_reinforced` (importance reinforcement timestamp). v1/v2 records
decode with `last_reinforced = 0`. See `memory_doc.rs` decode discipline:
every flag advances `off` even on the 0 branch.

### MDM1 v3 → v4 (v1.1.4 item 7)

Adds `scope` (multi-tenant isolation string). v1–v3 records decode with
`scope = ""`. The decode MUST advance `off` after `last_reinforced` even when
the scope is present — `last_reinforced` was the last field of v3; without the
advance, v4 reads the scope from the wrong offset (real bug, fixed).

### MDM1 v4 → v5 (v1.1.4 item 10)

Adds `entities` (declared 1-hop entity strings). v1–v4 records decode with an
empty list. Same discipline: the scope was the last field of v4; every field
advances `off`.

### MDM1 v5 → v6 (v1.1.6 item 2)

Adds `content_type` (stable type label: `text`/`json`/`code`/`embedding`/
`binary`, `None` = not declared). v1–v5 records decode with `None`. NMD1 and
TKLV/TKCK are untouched — the label lives only in `sys/meta/`, travels on the
MDR1 (via `meta_for_import`), and never reinterpretes old bytes.

### MDM1 v6 → v7 (v1.1.14 — era do modelo)

Adiciona `scope_dims` (`ScopeDims{user,agent,app,run}`) e `model_id` (ADR-0007).
Migração **explícita**: registros v1–v6 decodificam com `scope_dims` vazio e
`model_id = ""` — nenhum byte antigo é reinterpretado. Ver ADR-0007 para a
política de era (`mixed_models`, `era_report`, custo de re-embed).

**Correção de rótulo (fonte única: a tabela acima).** O bump do wire do MDM1
v7 é **v1.1.14** — é o que dizem os comentários do codec
(`memory_doc.rs`, "v1.1.14 MDM1 v7") e o que importa para decodificar bytes. O
CHANGELOG rotula a *política de era completa* no v1.1.15 (e uma seção chegou a
escrever `META_VERSION = 7 (v1.1.15)`); nenhum byte mudou entre 1.1.14 e
1.1.15 — a divergência é de rótulo, não de formato.

### MDM1 v7 → v8 (1.2.2 — triagem s413)

Adiciona `authority` (`u8`: `0` = learned, `255` = HITL-approved) como **último
byte** do bloco meta — 1 byte, sem comprimento. Migração **explícita**: v1–v7
decodificam com `authority = 0` (learned). NMD1 e TKLV/TKCK intocados.

**Não é forward-compatível.** O v8 é aditivo para quem lê v1–v7, mas um leitor
**v7 recusa** uma meta v8 (`MemoryMeta::decode` valida a faixa de versões): o
vendored do OS em 1.1.16 (v7) não lê metas escritas pelo standalone atual. O
NMD1 segue byte-idêntico — a divergência é só do side-table da meta (ver
`docs/interop-os.md`).

O campo entrou no wire no **1.2.2** (o `set_authority`/`authority_of` público só
chegou no 1.4.4, junto do `explain full`); por isso o consumidor só consegue
*ler* o campo antes do 1.4.4, e o `merge_remote` já desempata por `authority`
antes do timestamp (`MergeVerdict::RejectedByAuthority`). O golden
`golden_mdm1_v8_layout_pins_authority_as_last_field` pina a versão 8 no byte 4 e
o `authority` como último byte.

### Pre-v0.6 records → identity (lazy)

Records written before provenance existed return `meta: None` until re-put
or `set_importance`/`set_confidence`. On replication, identity is derived
from the AUTHOR's clock (never `self.node_id`).

### Records written before the prefix-key guard (P1-7, 2026-08-13)

ART does not support prefix keys; `engine::put`/`associate` now reject a
prefix-key with `SgdbError::Invalid` BEFORE writing. Data written earlier
under a prefix-key layout is unreachable by `get` — keys must use
fixed-width suffixes (e.g. `d{02}`, `k1`/`k2`). No bytes are corrupted;
only re-keying recovers the entries.

## Versioning a format (checklist)

1. Decide compat: backward-decodable (MINOR) vs breaking (MAJOR) — see
   `VERSIONING.md`.
2. Bump the format's version marker in the codec.
3. Update the golden tests **in the same commit** (`golden_nmd1_bytes`,
   `golden_record_bytes`, `golden_mdm1_v8_layout_pins_authority_as_last_field`,
   or add a new one) plus decode tests for **every** earlier version (v1…vN
   intermediate decodes — every field must advance `off`, see the MDM1 v4/v5/v6
   bugfixes), and add the new row to §"MDM1 — evolução dos campos".
4. Update `docs/api.md` §Format versioning and this file.
5. If the OS shares the format, coordinate the bump in `neural-os-core` —
   never ship a divergent layout (bughunt parity is a contract).