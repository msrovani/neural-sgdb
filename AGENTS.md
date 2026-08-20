# AGENTS.md — neural-sgdb

Guide for AI agents (OpenCode, Cursor, Windsurf, Claude Code) working in this
repo. **Read `codemap.md` (atlas), `docs/api.md` (contract) and
`docs/architecture/` (v1.1.6 — Memory Model, Lifecycle, Retrieval,
Distributed, Storage, Cognitive API) and
`docs/implementation-status.md` before editing code.**

## Post-P2 hardening state (2026-08-13)

P0 (committed in `feat(v1.0): P0 hardening…`): docs aligned to v1.0.0, clippy
zero-warnings (13 files), CI gates (clippy/doc/no-default — **fmt deliberately
NOT gated**), MCP paginate overflow fix, arbitration empty-scores fix,
FileStorage truncation sweep test, SAFETY comments on all 9 unsafe sites,
differential SIMD test.

P1 (committed in `feat(v1.0): P1 hardening…`): wire encode safety
(`try_encode` — no silent truncation), centralized `limits.rs`, deterministic
LCG property tests (P1-4), honest benchmarks + `BENCHMARKS.md`, scan
pagination + RAG size caps, ART prefix-key rejection (`has_prefix_conflict`).

P2 (committed `feat(v1.0): …P2-*…`): CRDT convergence in random topologies
(P2-1), governance docs + ADRs 0001–0006 (P2-2), `health()`/`validate()` +
signed-transport reference flow (P2-3), central wire-codec fuzz harness
`src/wire_fuzz.rs` over all 8 wire types (P2-4), layered multi-AI telepathy
mesh (P2-5). MCP server now exposes `health`/`validate` tools; new p2p
examples: `mesh_simulation`, `signed_peer`. Matriz: **193+1 / 239+1 / 148+1**,
no_std gate ok. `.freebuff/` is tool state — gitignored, never commit.

## Post-audit v1.1.2 (guinea-pig plan, 2026-08-14)

The fixes for what the guinea pig hit using the DB for real (each committed
with a regression test, hot test 49/49 exit 0):

- **`resolve_known_key` (sgdb.rs)**: raw keys resolve to the existing
  canonical storage key by deterministic layer priority (L4 semantic first).
  Side-table reads/writes (meta/set_importance/confidence/reinforce/
  add_parents/forget/explain/transfer_to/merge_memories/supersede/state/
  validity/delete/export_record) no longer silently miss with the right key
  and wrong form. `ensure_meta` errors carry a canonical-key hint.
- **`recall_weighted` uses DOC importance** (not layer): score =
  `w_imp·(1−provenance.importance)`; records without meta fall back to the
  layer default (`layer_importance`). The name now tells the truth.
- **`associate_checked`** validates both sides exist (ghost → `Err`, no
  orphan `sys/rel/`); raw `associate` keeps the no-validation design.
- **`Embedder` trait (`src/embedder.rs`)** + `DemoEmbedder` + `demo_embed`
  (moved from mcp_server): `no_std`-safe zero-dep seam for a real model. MCP
  accepts agent-supplied `embedding` in `remember`/`recall`/`rag_context`,
  falling back to `NEURAL_SGDB_EMBEDDER` (default demo trigram). Contract:
  whoever supplies embeddings uses the SAME model on write and query (4-dim
  agent vector ≠ 256-dim demo — they don't cross-match, by design).
- `sqrt_f32` is now `pub(crate)` (sgdb.rs) — Newton, reused by embedder
  (regra 3 no_std: no `f32::sqrt` in core).

## Post-audit v1.1.3 (co-author ergonomics, 2026-08-14)

The things that would annoy the agent USING the DB (each committed with a
regression test; hot test 60/60 exit 0; matrix 206+1 / 252+1 / 159+1):

- **`recall` is loud on dimension mismatch (S1)**: query dims matching NONE
  of the indexed embeddings → `SgdbError::Invalid` (message references
  `indexed_embedding_dims()`), not silent hamming garbage — the P4 contract
  ("same model on write and query") is enforced loudly. Text-payload L4/L5
  (no bitvec) don't feed the detection. New accessor
  `Sgdb::indexed_embedding_dims()`. Wire-dims live in
  `Engine::indexed_dims: BTreeSet<usize>` (built in `index_doc`, cleared on
  rebuild).
- **`examples/embedder_http.rs` (S2)**: the `Embedder` trait plugged into a
  real HTTP endpoint (raw HTTP/1.1 + existing `serde_json` dev-dep, zero new
  deps). `HttpEmbedder` + self-contained mock embedding server; proves the
  same-model contract and the S1 guard end-to-end. 4/4 PASS.
- **`recall` companion texts in batch (S3)**: `Engine::get_texts_batch` reads
  all `/L2/` companions in one deduplicated pass (payload only, no
  `attach_meta`) — was N×(NMD1 + meta) reads per recall. Contract parity:
  missing companion → empty `Hit.text`.
- **Proactive BQ reclamation (S4)**: the BQ flat is append-only — physical
  `delete` left orphan ids in the index forever (harmless but inflating the
  candidate pool). `BqFlatIndex::retain` recompacts; `Sgdb::delete` fires
  `reclaim_bq_orphans(DEFAULT_BQ_ORPHAN_THRESHOLD=64)` on the spot (bounded
  churn — only recompacts when the payout justifies O(N)); `threshold=0`
  always recompacts.
- **MCP `recall` pagination is lazy (S5)**: the server computes only
  `off+size+1` hits per page (the `+1` sentinel makes a full page report a
  `nextCursor`) instead of a hard-coded top-100 with an artificial ceiling.
  Deterministic top-k ⇒ each lazy page slices the same ranking. Hot test
  phase 4c exercises the real handler (raw `rpc` — `tool()` only returns
  `content[0].text`, not the top-level `nextCursor`).

## Post-audit v1.1.4 (memory landscape, items 1–10)

Ported from the memory-landscape benchmark (mem0/mempalace/Zep/Letta/
Supermemory/cognee — see `docs/memory-landscape.md`). Matrix: **210+1 /
252+1 → 256+1 / 159+1 → 162+1**, clippy/no_std/doc gates green, hot test
exit 0 (23 MCP tools).

- **ADD-only é contrato oficial** (item 1): o BQ é append-only — novos fatos
  acumulam e confrontam o pool via ranking determinístico, nunca overwrite
  silencioso. Resolução de conflito é questão de retrieval-time
  (`recall_weighted`/`supersede`/`resolve_conflict`), não mutação de write.
- **`remember_episodic` (item 2, mempalace)**: guarda o par user/response
  CRU em L2 timestamped (`md/L2/<ts>/u` e `/a`), sem extração/resumo — o
  antídoto à perda de contexto da extração. Devolve as keys. MCP:
  `remember_episodic`.
- **`feedback(key, positive, amount)` (item 3, cognee improve)**: re-pondera
  importance E confidence pelo resultado real (sobe/desce ambas, clamp
  [0,1], non-finite rejeitado, não ticka relógio). MCP: `feedback`.
- **`diary(node_id, limit)` (item 4, mempalace)**: episódicos L2 do agente,
  recentes primeiro (keys `ts/{:016x}` sortable — reverter). MCP: `diary`.
- **`profile(node_id, limit)` (item 5, supermemory)**: fatos L3/L4/L5 do
  agente por importância desc; texto via companion L2 quando o payload é
  embedding. MCP: `profile`.
- **`expire_old(now)` (item 6, supermemory)**: varre `sys/validity/` e marca
  `Invalidated` as janelas fechadas em `now`; idempotente; recall default
  (active-only) ignora; história via recall_historical. MCP: `expire_old`.
- **Scoping multi-agente (item 7, mem0 multi-tenancy)**: `MemoryMeta.scope:
  String` = **MDM1 v4** (migração explícita — v1/v2/v3 decodificam com
  `scope=""`; NMD1/TKLV intocados). O filtro de scope roda DENTRO do pool de
  candidatos do `recall_impl`: memória escopada nunca compete por vagas de
  outro scope; o recall global (sem scope) NÃO vaza de scopes (null-scoping
  implícito mem0). APIs: `set_scope`/`scope_of`/`recall_scoped(_historical)`.
  MCP `remember(scope=)`/`recall(scope=)`. **BUG FIX**: decode MDM1 não
  avançava `off` após `last_reinforced` (v3 era último campo; o v4 `scope`
  lia do offset errado) — disciplina "todo flag avança off".
- **Modos de retrieval (item 8, cognee `search_type`)**: MCP `recall` ganha
  `mode` = `semantic` (default, BQ+FP32) | `lexical` (BM25 sobre textos
  L2/L3, SEM embedding) | `hybrid` (semântico + lexical não-duplicados).
  Core: `recall_lexical_scoped(_historical)`/`recall_hybrid_scoped`. O path
  lexical/hybrid agora honra o mesmo filtro de scope do recall (vazava
  antes). **`Engine::effective_scope(sk)`**: o scope do companion `/L2/`
  vem do primário `/L4/`/`/L5/`/`/L3/` do mesmo id (a meta do companion não
  carrega scope). Usar `effective_scope`, NÃO `doc.meta.scope`, em filtros
  de recall.
- **Retrieval temporal com intenção (item 9, mem0/Graphiti bi-temporal)**:
  `recall_temporal(query, k, at, w_sem, w_time)` re-ranqueia o pool
  semântico pela proximidade ao instante `at` — memórias VÁLIDAS em `at`
  (janela `from ≤ at < until`) sobem, as que não vigoravam descem, sem
  janela usa recência relativa a `at`. Responde "quando mudou X?" /
  "qual era o estado em T?". Escopado: `recall_temporal_scoped`. MCP:
  `recall_temporal` (`at` obrigatório).
- **Entidades 1-hop (item 10, Graphiti/cognee — modo barato)**: entidades são
  **metadado explícito fornecido pela camada superior** —
  `MemoryMeta.entities: Vec<String>` = **MDM1 v5** (migração explícita:
  v1–v4 decodificam com lista vazia; NMD1/TKLV intocados). O core **NUNCA
  extrai entidade de texto** (mesmo contrato do `Embedder`: quem fornece usa
  as MESMAS strings na escrita e na busca — 1-hop só casa strings idênticas).
  Índice derivado `Engine::entity_index` (entidade → storage keys),
  reconstruído do `sys/meta/` no rebuild, mantido por `persist_meta`/
  `write_meta`, limpo em `delete` (`remove_entities`). APIs:
  `set_entities`/`entities_of`/`recall_entities`/`_historical`/`_scoped`/
  `_scoped_historical` (rank por overlap desc → importância desc → key asc;
  `dist` = fração de entidades não cobertas). MCP `remember(entities=)` +
  tool `recall_entities`. **BUG FIX (bis)**: decode MDM1 não avançava `off`
  após o SCOPE (v4 era último campo; o v5 `entities` lia do offset errado) —
  disciplina "todo campo avança off", não só flags.
- **Testes/duração**: regressões por item (10 novos + scoped + temporal +
  modes + scope/entities reopen+rebuild). Hot test MCP cobre `recall_temporal`,
  os modos e `recall_entities`; tools = 22 (itens 2–6 adicionaram
  remember_episodic/feedback/diary/profile/expire_old; item 10 adicionou
  recall_entities).

## Post-audit v1.1.6 (hits TIPADOS para o consumidor máquina, 2026-08-19)

O retorno de recall é lido por OUTRA inteligência (não por humano) — e o canal
carrega dados que NÃO são palavras humanas (JSON de intenção máquina→máquina,
embeddings da era, código, binários). Antes, tudo passava por
`String::from_utf8_lossy` e o consumidor não sabia nem QUE datum era nem COMO
parseá-lo. Cada item com regressão; matrix **229+1 / 181+1 / 275+1**, gates
verdes, hot test 90/0 exit 0.

- **`src/ctype.rs` (no_std-safe)**: `ContentType` = Text | Json | Code |
  Embedding(dim) | Binary — HINT derivado na LEITURA (nunca persistido; o
  writer pode declarar via seam, mesmo contrato de `entities`/`Embedder`);
  `RecallPath` = Semantic | Lexical | Entities (o campo `dist` tem escala
  DIFERENTE por path — cosseno 0..1 vs BM25 normalizado; em `hybrid` o
  consumidor precisa saber qual é qual); `detect_content_type` (JSON =
  `{…}`/`[…]` delimitado; código = keyword (`fn `/`impl `/`return `/`=> `…) +
  UM segundo sinal estrutural (chave/semicolon/arrow/outra keyword) ou
  braces≥2 + semis/arrows — um `-> ` isolado em prosa ("L5 -> md/L2") NÃO vira
  code, o custo de rotular prosa como code (consumidor com menos contexto pode
  tentar parsear o texto) é maior que o de rotular code como text (verbatim);
  não-UTF8 → Binary), `embedding_dim_of` (len%4==0 e ≥4 — mesma regra do
  `index_doc`/S1).
- **`Hit` (v1.1.6)**: + `path`, `content_type`, `score` (bruto, ranking
  auditável), `matched_terms` (grounding BM25 — o "porquê" do casamento),
  `validity: Option<(u64,u64)>` (janela bi-temporal por hit), `rel: Option<
  String>` (companion `/L2/` → key do primário `/L4|L5|L3/`). `HitProvenance`
  + `last_reinforced`/`scope`/`entities`. **`Hit` NÃO é tipo wire** (MDR1 é)
  → sem quebra de formato. `recall_weighted`/`recall_temporal` CARREGAM o Hit
  (novos campos fluem de graça).
- **Projeção prosa só para Text/Json/Code**: Embedding/Binary → `text`
  vazio; o consumidor vê `type=Embedding(4)` e sabe que o datum é o payload
  binário (era ADR-0007), nunca `from_utf8_lossy`.
- **BUGFIX companion L5 (bughunt #13)**: o lookup do texto companion fazia
  `sk.replacen("/L4/","/L2/",1)` — no-op para keys `md/L5/` → o batch lia o
  PRÓPRIO doc L5 e projetava floats como prosa lossy (o bug exato que o
  pedido apontou). Agora `companion_key()` mapeia L4 E L5 → `md/L2/<id>`;
  sem companion, o tipo cai para o payload (Embedding/Binary) e o texto fica
  vazio (batch com texto vazio NÃO sobrescreve o tipo).
- **`LexicalIndex::search`** retorna `(key, score, matched_terms)` — termos
  da query que casaram, deduplicados por doc.
- **`Sgdb::primary_of`**: `md/L2/<id>` → primário existente (`/L4/`→`/L5/`→
  `/L3/`) para follow-ups (`explain`/`supersede` miram o primário).
- **`Hit.payload_type` (v1.1.6 item 3)**: o datum REAL do primário — para um
  hit semântico L4/L5 (com ou sem companion) é `Embedding(dim)` do payload; o
  consumidor re-usa o vetor com o MESMO modelo (era ADR-0007), mesmo quando a
  projeção `content_type` é Text (companion). Para um companion `/L2/` via
  lexical/entities, `primary_of` agora devolve `Option<(String, ContentType)>`
  (tipo do payload do primário) — 3 sites de construção (recall_impl,
  recall_lexical_impl, recall_entities_impl) preenchem `payload_type`.
- **`rag_context_reranked` (v1.1.6 item 4 — lição P1/P5)**: o gargalo é
  ESCOLHER o que entra no prompt. `Sgdb::rag_context_reranked(emb, query_text,
  k)` = pool AMPLIADO (`recall_oversampled` oversample 8, top-4k) + rerank por
  ancoragem lexical (tokens do query — `lexical::tokenize`, agora `pub(crate)`
  — presentes no texto do hit, substring lowercased) → score desc → dist asc.
  Linha expõe `anchors=N` (o "porquê" é auditável); mesmo teto de bytes do
  `rag_context_limited`. MCP `rag_context` ganhou `rerank=true` (só no mode
  semantic default).
- **MCP `fmt_hit`** (unificado p/ recall todos os modes + recall_temporal +
  recall_entities): `- {key} | {text} (d=..) [state=.. imp=.. conf=.. src=..
  path=.. type=.. terms=.. rel=.. valid=..]` — invariantes preservadas
  (prefixo `- {key} | ` = paginação `split(" | ")`; sufixo abre ` [state=`).
  `payload=..` só aparece quando difere de `type` (datum real ≠ projeção).
  `rag_context` ganhou `mode` (semantic/lexical/hybrid) devolvendo hits
  tipados no lexical/hybrid. Nenhum tool NOVO (22 permanece).
- **MCP `format=json`** (v1.1.6 item 1): recall, recall_temporal,
  recall_entities e rag_context aceitam `format=json` → hits ESTRUTURADOS
  (`[{key,text,dist,score,path,type,dim,matched_terms,validity,rel,provenance}]`)
  com strings ESTÁVEIS (`path= semantic|lexical|entities`, `type=
  text|json|code|embedding(bin)`) — o consumidor máquina parseia sem depender
  da projeção prosa nem do `Debug`. Default continua a prosa (invariantes).
  Em `rag_context` semantic + `format=json` usa `recall` (hits) em vez do
  core `rag_context` (string). Hot test 86/0 (2 novos: recall json + rag json).
- **Seam de WRITE — `set_content_type` (v1.1.6 item 2)**: `MemoryMeta.
  content_type: Option<String>` = **MDM1 v6** (migração explícita: v1–v5
  decodificam com `None`; NMD1/TKLV intocados). Quem fornece declara o rótulo
  ESTÁVEL (`text`/`json`/`code`/`embedding`/`binary` — `stable_label`/
  `parse_stable_label` em `ctype.rs`); o consumidor deixa de depender do
  detector heurístico (a saga type=Code era adivinhação). Rótulo inválido →
  `SgdbError::Invalid` na escrita. A declaração PROPAGA para o companion
  `/L2/` do primário (L4/L5) — semântico e lexical tipam igual. **declared
  wins** nos 3 sites de construção do Hit (`resolve_content_type` em sgdb.rs):
  Embedding declarado absorve a dim do payload; Embedding/Binary declarado
  NUNCA rende prosa (`renders_prose` guarda `Hit.text`). MCP `remember(type=)`.
  `content_type_of(key)` lê a declaração. `meta_for_import` carrega a
  declaração na replicação (o tipo viaja no MDR1).
- **`examples/two_ai_protocol.rs` (v1.1.6 item 5)**: o contrato COMPLETO
  máquina→máquina, ponta a ponta (16 auto-checks): IA-A (writer) grava JSON de
  intenção, JSON NÚMERO `"42"` (o detector diria Text — o seam remove a
  adivinhação), binário não-UTF8 (L3 + entidade `datum/checksum`) e vetor da
  era — todos DECLARADOS via `set_content_type`; IA-B (reader) consome os hits
  TIPADOS: `content_type` (declarado vence), `payload_type` (Embedding(dim) do
  primário), `rel` (companion → primário), `matched_terms`; parseia JSON
  verbatim, segue `rel=` e re-lê o payload do primário, consome o binário cru
  pela key (bytes idênticos, nunca `from_utf8_lossy`). Determinístico
  (InMemory, sem LLM, embedding LCG do exemplo). Exit 0 sse 16/16.

## Agent decision protocol (v1.1.4, exemplo `agent_protocol.rs`)

O core NÃO decide — ele dá o MATERIAL da decisão. O exemplo
`examples/agent_protocol.rs` codifica a disciplina de uso da camada superior
(itens 2–6, 12 auto-checks): **ontologia de entidades** (strings canônicas
`kind/name` — mesmas na escrita e na busca; 1-hop só casa strings idênticas),
**fato estruturado** (`Fact {subject, predicate, object}` + entities + scope;
verbatim via `remember_episodic`), **evidência ponderada por provenance**
(`recall_weighted` com w_imp forte puxa memória confiável antes de legada),
**ciclo de vida** (`learn_fact` supersede o antigo, feedback positivo, 
`expire_old` periódico) e **protocolo de duas passadas** (`gather_evidence`
coleta SEM escrever; passada 2 registra o aprendizado). Regra de ouro do
protocolo: memória de outro scope NUNCA vira evidência (filtro em todos os
recalls).

## Agent decision protocol v2 (P1–P7, exemplo `memory_arena_eval.rs`)

Protocolo v2 = o que a pesquisa de 2026 (SmartSearch 2603.15599, MemoryArena
2602.16313, survey "Memory for Autonomous LLM Agents" 2603.07670, FSFM
2604.20300) diz sobre o USO da memória. `agent_protocol.rs` agora tem **23
auto-checks** (itens 2–6 + P1–P6):

- **P1/P5 — rerank gate + verbatim**: o gargalo não é achar, é ESCOLHER o que
  entra no prompt (SmartSearch: recall 98.6% mas só 22.5% da evidência dourada
  sobrevive ao truncamento sem rerank). `rerank_gate` = pool HÍBRIDO
  (`recall_oversampled` semântico ∪ `recall_lexical`) + rerank por ancoragem
  lexical (tokens do query no texto do hit) ANTES de compilar. Fato EXATO fica
  VERBATIM em L2 (`remember_episodic`): o BQ só indexa L4/L5 — a via lexical
  recupera (verbatim > abstração, achado MemoryArena).
- **P2 — write-path filter**: `remember_fact_checked` prova pelo 1-hop de
  entidades se `subject predicate` já existe; objeto igual → DEDUP (sem version
  bump, sem churn de manutenção), mudou → escreve (version bump, identidade
  estável) — o estágio de "memory management" do unified framework.
- **P3 — reflection grounding**: `store_reflection` cita ≥1 evidência
  episódica (`DerivedFrom` do hit + `Supports` da evidência — trilha
  auditável); `recheck_for_contradiction` busca ATIVAMENTE evidência contra a
  crença e marca `Contradicts` (anti erro auto-reforçado).
- **P4 — esquecimento + bi-temporal**: `open_session` roda `expire_old` na
  abertura (FSFM/Ebbinghaus); "qual era o estado em T?" vira
  `recall_temporal` (janela que cobre `at` = penalty 0; não cobre = penalty 1).
- **P6 — checkpoint multi-sessão**: `open_session` carrega as restrições
  LATENTES do scope via `recall_scoped` (o ambiente não as reestateia —
  MemoryArena); recall global não vaza de scopes.

**P7 — `memory_arena_eval.rs`**: avaliação da UTILIDADE (não do recall) da
memória, estilo MemoryArena: loop memória–agente–ambiente com subtarefas
INTERDEPENDENTES em múltiplas sessões, medindo SR (success rate binário) e sPS
(soft progress). Config A (naive hoarder: global, semântico puro, append com
timestamp) vs Config B (protocolo v2). Seção 1: quiz de recall estático —
AMBOS saturam 3/3 (memorização não distingue). Seção 2: 3 tarefas agênticas —
B 3/3, A 0/3 (shopping/restrição escopada, formal/verbatim exato,
lifecycle/estado corrente). Determinístico (InMemory, sem LLM), exit 0 sse
quiz empata E SR(B) > SR(A) E sPS(B) > sPS(A).

## Repository Map

A full codemap is available at `codemap.md` in the project root.

Before working on any task, read `codemap.md` to understand:
- Project architecture and entry points
- Directory responsibilities and design patterns
- Data flow and integration points between modules

For deep work on a specific folder, also read that folder's `codemap.md`
(`src/codemap.md`, `examples/codemap.md`).

## What it is

Memory database for AI agents (**memories, not data**): documents with a
cognitive layer L0–L7, semantic recall BQ + FP32 rescore (no FAISS/HNSW), O(k)
ART index, pluggable storage, optional CRDT sync, MCP server. Extracted from
neural-os-core as a community project: **zero deps, `no_std` + `std`, MIT OR
Apache-2.0**. OS interop via byte-identical NMD1 and TKLV formats.

## Development rules

1. **Zero dependencies in the lib** — only `alloc` (no_std) / `std`. Examples
   may use dev-deps (`serde_json`). Do not add deps to `[dependencies]`.
2. **`no_std` is a contract** — `cargo check --no-default-features --target
   x86_64-unknown-none` must ALWAYS pass. `deny(warnings)` in no_std elevates
   dead-code to error: use explicit `#[allow(dead_code)]` on port-parity.
3. **`f32::sqrt` does NOT exist in core** for that target — use `sqrt_f32`
   (Newton, in `sgdb.rs`) or `libm` (not in this crate).
4. **ART does not support prefix keys** — keys where one is a prefix of another
   break silently; use fixed-width suffixes. **Hardened (P1-7)**: `ArtIndex::
   has_prefix_conflict` + guards in `engine::put`/`engine::associate` reject a
   prefix-key with `SgdbError::Invalid` BEFORE writing (no more silent loss).
5. **Formats are contracts** — NMD1 (`memory_doc.rs`) and TKLV (`tickv.rs`) are
   byte-identical to the OS; do NOT change encode/decode/layout without
   updating the OS.
6. **Seams, not globals** — clock via `now: u64`, SIMD via `cpu_caps()`/
   `set_cpu_caps()`, log via `sgdb_log!`. No global engine statics.
7. **Verification** — `cargo test` (157+1 default, 192+1 `--features p2p`,
   114+1 `--no-default-features`) and `cargo check` (std + no_std) before
   committing. Clippy/rustdoc run with `-D warnings` (P0-5/P0-6/P0-10);
   `cargo fmt` is NOT a gate (repo is not rustfmt-clean — 223 diffs).

## Quick API

```rust
use neural_sgdb::{Sgdb, InMemory, FileStorage, Storage, SgdbError};
use neural_sgdb::{MemoryLayer, MemoryDoc, ArtIndex, BqFlatIndex};

// open (InMemory for tests; FileStorage to persist; TickvFile for OS format)
let mut db = Sgdb::open(FileStorage::open("mem.db")?)?;

// memories
db.remember_exchange("user", "response")?;             // L1 + L2
db.remember_semantic("k", "text", &emb)?;              // L4 BQ (emb: &[f32])
db.remember_semantic_with("k2", "text", &emb, RememberOptions {
    scope: Some("user/ana"), entities: &["pref/theme"], content_type: Some("text"),
})?;
db.set_default_scope(Some("project/foo".into()));
let _ = db.recall_empty_hint("", "semantic");          // hint se recall global vazio
db.remember_fact("fact", now)?;                        // L3 timestamped
db.checkpoint()?; db.prune_working_ram()?;             // flush L0/L1 RAM
db.delete("md/L4/k1")?;                     // deleção FÍSICA (tombstone + índices)

// recall (requires caller-supplied embeddings)
let hits: Vec<Hit> = db.recall(&query_emb, 5)?;        // coarse BQ + FP32 rescore
let ctx = db.rag_context(&query_emb, 3)?;              // string ready for the prompt
let facts = db.scan_prefix("md/L3/")?;                 // ART prefix scan
```

## Testing the examples

```bash
cargo run --release --example bench        # benchmarks (ART/BQ/recall vs FP32)
cargo run --release --example mcp_server   # MCP server for AI agents
cargo run --release --example mcp_client   # HOT TEST: drives mcp_server like an IDE (77 checks)
cargo run --release --example agent_protocol  # DECISION PROTOCOL (itens 2–6 + P1–P6): como o agente USA o DB
cargo run --release --example two_ai_protocol # PROTOCOLO MÁQUINA→MÁQUINA (v1.1.6 itens 1–5): IA-A grava datum declarado, IA-B lê tipado
cargo run --release --example memory_arena_eval # MEMORY-ARENA EVAL (P7): utilidade da memória em tarefas interdependentes
cargo run --release --example mesh_simulation --features p2p  # layered AI telepathy mesh
cargo run --release --example signed_peer --features p2p      # signed-transport seam flow
```

## Agent self-memory (hot test, v1.1)

The project agent is a test subject: `.opencode/opencode.json` registers the
MCP server as a local server (`NEURAL_SGDB_DB=.nsgdb/memory.db`, gitignored),
giving the agent `mcp__neural-sgdb__*` tools (23: remember/remember_episodic/
recall/rag_context/recall_temporal/recall_entities/feedback/diary/profile/
expire_old/explain/reinforce/forget/associate/related_to/contradicts/
supersede/conflicts/resolve_conflict/merge_memories/health/validate/era_report). Audit
log in
`docs/hot_test.md`. Lessons learned (2026-08-13): `remember` returns the FULL
storage key (`md/L4/...`), always use it for follow-up (`explain`/`reinforce`
on the raw `mcp/...` key fails — was fixed); recall hits print `h.key | text`;
use the SAME words in queries as in the stored text (demo_embed is a trigram
hash, not a semantic model). Restart opencode after changing the config.

## Running tests

```bash
cargo test                                 # 229+1 tests (InMemory/FileStorage/TickvFile)
cargo test --features p2p                  # 275+1 (includes CRDT sync + mesh harness)
cargo test --no-default-features           # 181+1 (no_std core, host test harness)
cargo check --no-default-features --target x86_64-unknown-none   # no_std gate
cargo clippy --all-targets --all-features -- -D warnings          # lint gate (P0-5)
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps                   # doc gate (P0-6/P0-10)
```

## Repo-specific gotchas

- **no_std test matrix**: `cargo test --no-default-features` (host) é o gate de
  TESTE no_std (compila os testes do lib sem `std`); `cargo check
  --no-default-features --target x86_64-unknown-none` gate o lib. NÃO rode
  `--tests`/`--all-targets` no target bare-metal: dev-deps (`serde_json` →
  `memchr`) não compilam para ele — falha é do toolchain, não do crate.
  Exemplos que precisam de arquivo têm `required-features = ["file-storage"]`
  (bench, stress) — sem isso `cargo test --no-default-features` quebra.

- **MCP server** (`examples/mcp_server.rs`): stdout JSON-RPC ONLY (logs →
  stderr), one message per `\n` line, `2025-11-25` handshake, do not gate tools
  on `notifications/initialized` (Claude Code sends tools/list first), echo the
  id verbatim, `-32601` for unknown methods (modern-client fallback). The
  `demo_embed` embedding is a trigram hash — NOT a real semantic model. Now
  exposes read-only `health`/`validate`/`era_report` tools (P2-3 + ADR-0007);
  **instalação e troubleshooting**: [`docs/MCP.md`](docs/MCP.md) (launchers
  release, Cursor Windows, smoke test, embedder HTTP);
  [`docs/MCP-RELOAD.md`](docs/MCP-RELOAD.md) (reload vs rebuild, `.nsgdb/bin`).
- **Wire-codec fuzz harness** (`src/wire_fuzz.rs`, P2-4): the single LCG
  never-panic/roundtrip/truncation gate over ALL 8 wire types — add a new
  wire type there (plus its per-module `prop_tests`), and keep the matrix
  (229+1 / 181+1 / 275+1) green. `SignedEnvelope::decode` returns
  `Option<(Self, usize)>` (no magic byte — corrupt via field lengths, not
  byte 0).
- **TickvFile** (`src/tickv.rs`): 512-aligned records, tombstone `vlen=0` or
  `TKL\0`. **`scan_volume` MUST skip in-place tombstones (`hdr[3]==0`) before
  CRC** — otherwise OS-written deletes resurrect (bughunt #1 CRÍTICO, fixed),
  **and skip the `sys/tickv_ckpt` record** (checkpoint = metadata, never a
  live key). `checkpoint()` writes the TKCK record as the LAST record;
  `open()` fast-mounts via `try_mount_from_ckpt` (FNV-1a index check, per-entry
  CRC + `TKL V` stale check, ckpt-must-be-last) with full `scan_volume`
  fallback; `compact()` rewrites live set + ckpt + atomic rename. Fast-mount
  only wins under churn (tombstones) — all-live is parity.
- **CRDT** (`src/crdt.rs`): rate-limit uses `Option<u64>` (the 0 sentinel fails
  on first sync at now=0); `UdpTransport` is an unauthenticated demo — use a
  signed transport in production (`SignedEnvelope` é o formato de envelope
  autenticável p/ transportes assinados; o core não implementa crypto).
  `set_cpu_caps` must rearm `SELECTED` (bughunt #9).
- **Storage CRC** (`src/storage.rs`): FileStorage CRC covers **key‖val**, not
  just the key — bit rot in values must be detected (bughunt #2). Append uses
  a **persistent lazy handle** (perf ~38x): `compact()` must drop it BEFORE the
  atomic rename and reopen lazy, or writes after rename hit the old inode/file
  object (data loss). Bounds-check oversized keys/vals at write time (recovery
  would truncate the file otherwise).
- **Recall** (`src/sgdb.rs`): sort by the raw u32 score (FP32 0..10000 vs ham
  0..64 share the OS ordering space); `sk.replacen("/L4/", "/L2/", 1)` for the
  companion-text lookup — a key containing `/L4/` must not be corrupted
  (bughunt #3/#6). Use `recall_oversampled(query, k, oversample)` when the
  coarse BQ filter collides (low dims): raise the candidate pool, don't lower
  `k`. `Hit.dist` fallback hamming is normalized to 0..1 (bughunt #11).
- **clamp** (`src/sgdb.rs`): truncate at a char boundary — `&s[..max]` panics
  mid multi-byte char (bughunt #7).
- **`Sgdb::delete`** (`src/sgdb.rs`/`engine.rs`): deleção FÍSICA (tombstone +
  side-tables `sys/state|validity` + ART/lexical/id→sk). O BQ é append-only
  flat — entradas órfãs ficam inertes: o recall pula candidatos cujo doc
  sumiu (`Ok(None) | Err(_) => continue`), então nunca ressuscita memória
  deletada; rebuild/compactação reclama o espaço.
- **ART** (`src/art.rs`): `delete` now reclaims nodes (no leaf tombstone) —
  `delete_rec` returns `Option<Box<Node>>` (None = empty subtree) and shrinks
  256→48→16→4 when `n` drops. Match on `*node` (Box doesn't auto-deref in
  patterns); rebuild the box instead of returning the moved `node`.
- **Hamming dispatch** (`src/hamming_dispatch.rs`): `ensure_selected` uses
  `load`+`store`, NOT `SELECTED.swap(true)` (locked RMW on every `hamming` was
  the hot-path bottleneck); benign double-select race, `set_cpu_caps` still
  rearms (bughunt #9).
- **Bench baseline** (`examples/bench.rs`): recall@k must compare against true
  FP32 cosine over the original f32 vectors, never hamming over the same
  quantized bits (tautological, bughunt #4), and must use **correlated cluster
  data** — pure noise measures a meaningless 0%. Sign-BQ separates the cluster,
  not the exact member (dense clusters → hamming ties → id tie-break wins).
- **Features** (Cargo.toml): `std`, `file-storage`, `simd-runtime`, `p2p`
  (opt-in). Default = `["std","file-storage","simd-runtime"]`. no_std gate:
  `cargo check --no-default-features --target x86_64-unknown-none`.
- **BQ recall-time extras (2026)**: `MihIndex` (multi-index hashing sobre os
  bitvecs existentes — candidatos sub-lineares, `candidates()`/`top_k` com
  probes); `quantize_f32_centered`/`top_k_f32_centered` (query re-centrada pela
  média — bitvecs armazenados intactos); `recall()` usa auto-oversample por
  dimensionalidade (1 word→16, 2-4→8, senão 4); `recall_weighted` =
  `w_sem·dist + w_rec·recência(/ts/hex) + w_imp·importância(doc, penalty 1−imp)`
— v1.1.2 P2.
- **Model = era invariant (ADR-0007)**: o S1 checa DIMENSÃO, não identidade do
  modelo — trocar para outro modelo de MESMA dim degrada silenciosamente (sem
  cross-match). E o BQ trava `words_per_vec` no primeiro insert (bughunt #11):
  **escrever dim diferente num BQ vivo TRUNCA silenciosamente o vetor novo**.
  Troca de modelo = era: base nova por era, OU migração explícita (re-embed do
  texto preservado em `/L2/` + put cru no mesmo id — overwrite preserva
  memory_id — + `rebuild_indices()` para resetar a largura do BQ). Custos
  medidos em `examples/era_migration_bench.rs` (BENCHMARKS.md):
  rewrite ~72µs/doc, rebuild ~50ms/4k docs. `recall_lexical`/`recall_entities`
  são a rede embedding-free do passado; `recall_temporal` NÃO é (re-ranqueia o
  pool semântico). `compact()` reclama os blobs antigos (FileStorage append-only).
- **Write-side era guard (ADR-0007, v1.1.5)**: `remember_semantic` agora é
  LOUD no WRITE também — dim fora de `indexed_dims` num corpus vivo →
  `SgdbError::Invalid` (mensagem cita era + `era_report()`, nada é escrito);
  a primeira escrita de um DB vazio DEFINE a era (base nova por era continua
  livre). Migração deliberada = put cru (`MemoryDoc` + `quantize_f32`) +
  `rebuild_indices()` — o caminho guarded NÃO serve. `Sgdb::era_report()`
  (MCP `era_report`, read-only) reporta dims indexadas, contagem por dim,
  largura do BQ, cobertura `/L2/` e o CUSTO ESTIMADO (fórmula BENCHMARKS §Era
  aplicada ao total real: ~86µs/doc db-side; o custo do MODELO é externo — a
  LLM multiplica `docs_to_reembed`×`text_bytes` pelo próprio modelo). Veredito:
  `empty`/`ok`/`mixed_dims`. O core NÃO decide — reporta.
- **Lexical dual-path** (`src/lexical.rs`): índice invertido BM25-style sobre
  textos L2/L3 (alloc-only, no_std). `recall_lexical`/`recall_hybrid` no Sgdb.
  **`f32::ln` não existe no core bare-metal** → `ln_f32` (ponteiro: expoente
  IEEE + série no mantissa, precisão ~1e-5 — suficiente p/ ranking). Custo
  ~6µs/put (L2/L3 tokenizados no `index_doc`).
- **TickvFile** (`src/tickv.rs`): `put`/`delete` agora **invalidam o record
  antigo in-place** (`magic[3]='V'→0`, TKL\0) antes do append (parity OS) —
  `scan_volume` pula sem CRC. `compact()` reescreve live set + ckpt.
- **Validade temporal** (`sys/validity/`, engine/sgdb): `set_validity`/
  `invalidate`/`validity_at`/`recall_at` — **invalidar-não-deletar** (Zep/
  Graphiti); side-table 16B `from|until u64le`, NMD1 intacto.
- **Identidade/proveniência (v0.6)** (`sys/meta/`, engine/sgdb): `MemoryMeta`
  (memory_id 32-hex, source, confidence, importance, created_tick,
  parent_ids, clock_overflow) é side-table — o NMD1 **não mudou** (decisão de
  formato: sem version bump; `docs/api.md` §Format decision). Regra de
  identidade: **memory_id é estável por chave** — overwrite preserva
  memory_id/source/created; doc replicado com `meta` preserva a identidade
  do criador; re-criação pós-delete ganha id NOVO (watermark do contador
  próprio, reconstruído no rebuild de docs 72B + metas `sys/meta/`).
  Registros pré-v0.6 → `meta: None` até re-put/`set_importance`.
- **Clock dinâmico (v0.6)**: `VectorClock` = 8 nós fixos + `overflow`
  (bounded 248). O NMD1 serializa SÓ 72B — overflow persiste em
  `sys/meta/` e é re-fundido no `get` (`attach_meta`). `encode`/`decode` do
  clock **não mudaram** (golden test intacto); quem mudar semântica precisa
  cobrir fixos + overflow (eq/happens_before/merge usam `iter_nodes`).
- **`Hit.provenance` (v0.6)**: recall* expõe `Option<HitProvenance>`
  (memory_id/layer/state/source/confidence/importance/created/parents).
  `set_importance`/`set_confidence`: fora de 0..1 é clampado, não-finita é
  `SgdbError::Invalid` (variante nova — atualizar matches exaustivos).
- **CRDT delta** (`src/crdt.rs`): `record_change` acumula `pending` deltas;
  `sync` envia só o não-visto pelo peer via `send_delta` (trait default cai p/
  `send_crdt`); `pending_deltas()` mede. Wire = protocol-interno (OK mudar).
- **Replicação v0.6** (`memory_doc.rs`/`engine.rs`/`sgdb.rs`/`crdt.rs`):
  `MemoryRecord` (doc NMD1 + state + validade + meta, wire `MDR1`) é a
  UNIDADE de replicação — fecha a contradição #2 (side-tables viajam).
  `import_record` **não ticka o relógio local** (receptor nunca vira autor)
  e deriva identidade do AUTOR do relógio p/ docs pré-v0.6 (nunca
  `self.node_id`). `merge_remote` (p2p): mesmo clock+payload+bitvec com
  estado/validade iguais → Duplicate; conteúdo igual mas side-metadata novo →
  reimporta (Applied) para propagar supersede/validade; clocks concorrentes →
  **Conflict, nunca sobrescreve**. `MemoryDelta`/`MemorySnapshot` agora
  carregam `Vec<MemoryRecord>` (codecs `MDLT`/`MSNP` bounds-checked) —
  substituem os stubs `docs: Vec<Vec<u8>>` (quebra de API documentada).
- **MergePolicy (v0.6)** (`crdt.rs`): tabela camada→política explícita
  (L0/L1/L6 não aceitam remoto → `MergeVerdict::Rejected`).
- **Hardening de versões (v0.6)**: `apply_remote_version` ignora `v==0`
  (heartbeat de relay não cria conflito fantasma) e **não adota** versão de
  peer em `local_version` (senão um nó fresh re-broadcasta versão alheia como
  autoria). `missing_after(peer)` devolve a faixa causal faltante (watermark
  por nó — versões são contíguas por construção).
- **Mesh harness** (`crdt.rs` tests, feature p2p): `Mesh` com arestas
  direcionais (partição = sem aresta), `round()` = TX→RX→pull via
  `merge_remote`; use `split_at_mut` para pares (i,j) (borrow checker não vê
  `i != j`). Cenários: triângulo, partition/rejoin preservando concorrentes,
  duplicata/atraso idempotente, nó novo alcançando tudo.
- **Anti-entropy v0.7** (`crdt.rs`/`engine.rs`): o pull direcionado precisa
  do clock **pré-ronda** do destino — medir a faixa faltante com o
  `known_clock()` corrente (já atualizado pelos anúncios da própria ronda)
  "esconde" a lacuna e nada é puxado (bug real: `applied=0` no mesh).
  `pull_delta` puxa `known+1..=v` por nó (anúncio anuncia o MÁXIMO; peer
  tardio precisa da série inteira).
- **Convergência mesh (P2-1)**: o que CONVERGE é o conteúdo causal
  (byte-idêntico por storage key: NMD1 + clock + state + validade + meta) e
  nenhuma-versão-perdida (autor preserva o próprio valor). **`node_versions`
  do CRDT é conhecimento de GOSSIP** — parcial em topologias direcionais, NÃO
  converge necessariamente (testes de clique comparam, topologias aleatórias
  não). **`ConflictRecord` é evidência LOCAL do merge** (onde o merge
  concorrente aconteceu) — não é a unidade MDR1 de replicação, então também
  não converge entre nós. Testar convergência = comparar `export_record`
  encode por key (exceto chaves em conflito, que preservam versões
  distintas por design) + `db.conflicts()` não-vazio + conteúdo do autor.
- **Um write lógico = uma versão causal** (v0.7): `remember_semantic` grava
  L4+L2 com puts separados e cada put ticka o relógio do doc — sem
  `put_companion` (mesmo contador, sem tick), o contador por-put diverge da
  versão do CRDT e `keys_for_clock(node, v)` perde o companion. Mudar o
  número de docs por write lógico exige revisar esse acoplamento.
- **Estado CRDT durável (P0-11)**: `CrdtState::decode` usa `Result` — `?`
  sobre `Option` (`.get()`) não compila nessa assinatura (`ok_or` antes);
  truncamento em `bytes.len()` não é truncamento. `restore` recusa node_id
  alheio.
- **L6 relations (v0.8)** (`engine.rs`/`sgdb.rs`): persistidas em
  `sys/rel/<kind>/<a>#<b>` + índice ART forward (`rel/…`) e reverse
  (`rev/…`) — AMBOS derivados (rebuild reindexa de `sys/rel/`; delete chama
  `remove_relations_for`). `#` é separador reservado (associate rejeita).
  `related_to` devolve o OUTRO lado da aresta (a--causes-->b visto de b = a).
- **AUDIT set_state (1.1)** (`engine.rs`): `set_state` com estado ≠ Active
  recusa chave sem doc (`Invalid`) — antes criava side-table `sys/state/`
  órfã (validate flagga "side-table targets missing doc"; mesma família do
  bughunt do hot-test via MCP). `Active` é remove-only e inócuo: supersede
  marca new→Active antes do new existir. `associate` NÃO valida existência
  (design: relação é afirmada pela camada superior). `merge_memories`/
  `transfer_to` já validavam via `export_record`. Teste: `set_state_rejects_
  ghost_key_no_orphan_side_table`; exemplo `examples/audit.rs` (battery 1:
  attack).
- **AUDIT validate BQ (1.1)** (`sgdb.rs`): o check de contagem do BQ contava
  só `md/L4/`, mas o `put_inner` indexa L4 **E** L5 (bitvec ou payload ≥4B
  reinterpretado como f32) — um doc L5 legítimo com embedding quebrava o
  `validate()` com falso positivo. Regra replicada: decode e conta docs
  `md/L4/`+`md/L5/` com `bitvec.is_some() || payload.len() >= 4`. Teste:
  `validate_accepts_l5_procedural_embedding`. `invalidate(key, now)` NÃO
  muda o estado (é validade até `now`; `until <= from` APAGA a marcação —
  usar now > from). `recall_weighted` pondera a importância do DOC
  (`Hit.provenance.importance`, penalty `1−imp`); sem meta, cai para a
  default da camada (`layer_importance`: L4=0.0, L5=0.2) — v1.1.2 P2.
- **Recall active-only (v0.8)**: o filtro de estado roda DENTRO do
  `recall_impl`/`recall_lexical_impl` (antes do ranking); estado é POR DOC —
  marcar `md/L4/k` não marca o companion `md/L2/k` (testes devem marcar
  ambos se quiserem sumir dos dois índices). `recall_historical` opta-in.
- **Lifecycle (v0.8)** (`src/lifecycle.rs`): `tick(db, now)` é determinístico
  e idempotente por construção (fonte só promove se `Active`); promoção
  grava `parent_ids` (via `add_parents`, `ensure_meta` cria meta p/ registros
  pré-v0.6) + relação `derived_from`. L3→L4 cria L4 SEM bitvec (embedding é
  da camada superior — nunca gerar embedding no core). no_std: `vec!` e
  `to_string` precisam de `use alloc::vec;` / `alloc::string::ToString` nos
  testes.
- **`MemoryRecord::decode`**: cuidado com flags — cada flag (vflag/metaflag)
  avança `off` mesmo no ramo 0 (bug real: metaflag=0 não avançava e o NMD1
  começava no byte errado).
- **ART** (`src/art.rs`): `scan_prefix_stats` expõe nós visitados (pruning de
  range — só desce em filhos que casam o prefixo). `MCP` (example): resources
  `memory://{layer}/{key}`, `nextCursor` paginação opaca, annotations
  `readOnlyHint`/`destructiveHint`/`idempotentHint`.
- **MDM1 decode (v0.9 bug fix)**: o branch `ver >= 2` do `version_id` não
  avançava `off` (`off += vid_len` faltava) — inofensivo no v2 (último
  campo), mas o v3 `last_reinforced` expôs o bug. Correção: `off += vid_len`
  dentro do branch. Sempre testar decode com versões intermediárias (v1,
  v2) e com v3.
- **Conflict model (v0.9)** (`src/conflict.rs`): `ConflictRecord.records` são
  `Vec<Vec<u8>>` (MDR1 paralelos a `candidates`) — re-merge upserta (id
  determinístico FNV-1a sobre subject+candidates ordenados, nunca duplica).
  `resolve_conflict` importa o winner via evidência e sobrescreve
  `version_id` no slot (decisão explícita ≠ overwrite implícito, que
  preserva identidade local). `dismiss_conflict` remove só o marcador;
  história permanece via `sys/version/`.
- **Reinforce (v0.9)**: `importance += delta` clampada a [0,1],
  `last_reinforced = engine.own_counter()`. Não ticka relógio — metadado
  local. Decode MDM1 v1/v2 sem `last_reinforced` → 0 (migração explícita,
  nunca reinterpreta bytes antigos).
- **`merge_remote` Conflict branch (v0.9)**: paraleliza (vid, MDR1) dos
  candidatos, ordena por vid, deduplica; nós fonte únicos e ordenados;
  `CrdtMemorySync::apply_remote_version` NÃO atualiza `local_version` do
  nó relay (evita poluição de broadcast).
- **`MemoryRecord` encode/decode**: cada flag (vflag/metaflag) avança `off`
  MESMO no ramo 0 — o bug real do `MemoryRecord` (v0.8, `metaflag=0` não
  avançava) foi corrigido; aplicar a mesma discipline em codecs novos.
- **Format contracts**: NMD1/TKLV byte-identical to the OS — golden tests
  (`golden_nmd1_bytes`, `golden_record_bytes`, `fnv1a64_known_vector`) pin the
  layout; change them in the same commit as any format change.
