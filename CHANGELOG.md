# Changelog — neural-sgdb

All notable changes to this project. Format based on
[Keep a Changelog](https://keepachangelog.com/), versions follow
[SemVer](https://semver.org/).

## [1.1.29] — 2026-09-25 (fast-mount IDX1, ADR-0009 §3)

- **Fast-mount do índice derivado (IDX1)**: novo wire type (10º do fuzz
  harness) em `src/idx_snapshot.rs` — snapshot do estado derivado (art_keys
  sem ids — NEXT_ID é de processo, entity_index, indexed_dims,
  corpus_counts — somas f64 FORA, não-associativas — e são recomputadas no
  mount; bug do hot test: corpus_sums vazio → drift fantasma no `validate`),
  `bq_entries` (sk→bitvec, pareado por storage key) e lexical COMPLETO
  (postings/doc_len/n_docs), com `fingerprint` como oráculo do mount.
  `Sgdb::open_with_snapshot(node_id, backend)` monta SEM ler docs; qualquer
  divergência/corrupção/ausência → full rebuild (mesma garantia). `open()`
  default inalterado.
- **MCP seam**: `NEURAL_SGDB_INDEX_SNAPSHOT=off|auto|always` (default off);
  auto/always → `open_with_snapshot`; persist do snapshot no
  `curate op=audit_checkpoint` ("+ snapshot idx persistido" na resposta).
- **Bench** (`examples/bench_index_snapshot.rs`, BENCHMARKS.md §Fast-mount
  IDX1): ~1.2×@800 docs, ~1.5×@8k (102–109 ms → 65–75 ms); persist do idx
  12–14 ms @ 8k docs. Teto MAX_VLEN (1 MiB): ~7k writes estouram o snapshot
  único — paginação é próximo passo se houver demanda.
- **RaBitQ A/B: REJEITADO nos dois critérios** (`src/rabitq.rs` mantido como
  registro; `examples/bench_rabitq_ab.rs` reproduzível): clusters densos
  −13..−17 pp recall@5 vs ADC-lite em todos os oversamples; custo ~25–45×
  pior (~1.3 ms vs 27–60 µs/query). ADC-lite permanece o caminho oficial.
- **ADR-0018**: budget allocation per view (Jev-Mem Eq. 13–14) DEFERRED —
  reativação só com multi-hop real + evidência medida.
- **Auditoria dos projetos base** (ROADMAP): tickv = port byte-exato do
  TickvLite (nada a portar); noproto congelado 0.9.x (NMD1 é layout próprio);
  art 2025–26 ataca concorrência/NVM que não temos. Todos 💤.
- Hot test: fase 9b (fast-mount IDX1) — **139/0**.

## [1.1.28] — 2026-09-23 (linguagem de máquina, ADR-0017)

- **Vocabulário ÚNICO prosa/JSON**: `state_label`/`path_label`/`layer_label`
  tabelas únicas em `ctype.rs`; o `{:?}` do Rust SAIU do wire (D1/D8). Prosa
  agora diz `state=active`/`path=lexical`/`type=text` — idêntico ao JSON.
- **`dist: null` no lexical** (D2): campo distância constante 0.0 enganava.
- **Tool `decide` (5º tool listado)** — 𝒥(S,𝒬) do Jev-Mem: N perguntas com
  espaços fechados → respostas tipadas em 1 round trip (suficiência de
  evidência, relação temporal, vigência, relevância de candidatos).
- **`recall_candidates`** (Movimento 3, Eq. 8/23): prefetch com sinais
  DECOMPOSTOS (`sim_vec`, `lex_overlap`, `shared_entities`, `recency`, `rrf`).
- **validate com gêmeo tipado** (D3): `structuredContent{healthy,issues[]}`.
- **k=0 é erro tipado** (D6).
- **D4/D5 fechados**: onboarding e cold_start.steps viram `{step,text}`
  tipados (ordinal fora da string); `unseen_scopes` virou tupla
  `{label,count}` (não mais `"scope(count)"` achatada).
- **D9 — calibração exposta**: `set_tie_margin(Some(m))/tie_margin_of()` —
  a margem de empate do state-first deixou de ser constante interna; o host
  calibra ao próprio corpus (`None` = default 50; `0` = state-first só em
  score exato). Teste prova que a calibração muda o ranking.
  Matrix **347+1 / 393+1 / 291+1**; hot test **135/0** (+16 asserções);
  contrato MCP → **1.1.28**.
- **v1.1.28.1 — gaps do Jev-Mem fechados**: `temporal_relation` com o
  vocabulário COMPLETO de 7 valores (before/after/same_time por timestamp;
  during/contains/overlaps por janelas de validade bi-temporal; aceita
  `a_key/b_key` e resolve `created_tick` via `created_tick_of`); stop de
  `evidence_sufficient` ENRIQUECIDO com `c_d` (contradições não-resolvidas
  de `conflicts()`) e `m_d` (`required_keys` ausentes) — Eq. 21 completa.
  Os 4 gaps de `docs/jev-mem-adoption.md` estão fechados (P2 segue
  deliberadamente registrado).

## [1.1.27] — 2026-09-23 (scores de tipo sobrepostos, ADR-0016)

- **`TypeScores` (ADR-0016)**: os quatro eixos do Jev-Mem (arXiv 2609.23986) —
  episódico/semântico/procedural/preferência — derivados **na leitura** de
  `layer` + `entities`, nunca persistidos (NMD1/MDM1 intocados). Sobrepostos,
  não exclusivos: L3 pontua episodic=0.7 E semantic=0.4. Preferência via
  convenção `pref/*` e L7. `Hit.type_scores: Option<TypeScores>`; MCP
  `format=json` expõe `type_scores` com vocabulary estável de `fields()`
  (nunca o `Debug` do Rust). Contrato MCP → **1.1.27** (novo campo visível).
- Testes: derivação completa (sobreposição, pref, L7, L0 zero, vocabulary) +
  fluxo end-to-end no recall lexical; teste de mutação prova que morre sem a
  regra. Matrix **344+1 / 390+1 / 288+1**; hot test **119/0**.

## [1.1.26] — 2026-09-22 (descoberta de escopo + anunciado == servido)

Sem mudança de formato (NMD1/TKLV intocados, MDM1 sem bump). `MCP_CONTRACT_VERSION`
→ **1.1.26**: o VALOR de dois campos visíveis mudou (`global_memory_count`,
`scope_labels`) e um parâmetro que o schema anunciava desde o v1.1.14 passou a
fazer alguma coisa. ADR-0015.

### Fixed
- **Ponto cego de escopo: a memória gravada não era alcançável por NENHUMA rota
  documentada.** Descoberto USANDO o DB (flush do harness contra o banco real),
  não lendo código. `curate(op=commit_run, scope_run="release-x")` — o caminho
  natural do ADR-0010 — grava `ScopeDims{user:"",run:"release-x"}`; como o
  legado espelha `user` (ADR-0013), `scope == ""` e o `scope_distribution`
  classificava com esse campo. Medido: `global_count = 1` (contado como GLOBAL),
  ausente de `scope_labels`/`scopes_to_probe`, `recall(scope=…)` → 0 hits,
  `recall` global → 0 hits (null-scoping correto), e a única rota
  (`scope_distribution_dims`) **não exposta por nenhuma superfície**. Agora
  global = `scope == ""` **e** dims globais; a classificação roda em um scan
  único (`scan_scope_metas`); `Sgdb::scope_probes` devolve as duas rotas com
  procedência (`legacy` vs `dims_only` — as labels são ambíguas por forma, um
  `scope` legado pode conter `/`); e `nsgdb://session` publica
  `scopes_to_probe_dims` com descritores prontos para o `recall`.
- **`recall` ignorava `scope_user/scope_agent/scope_app/scope_run`, que o schema
  ANUNCIA desde o v1.1.14.** `recall(scope_run="x")` respondia igual a
  `recall()`: 0 hits, `isError:false`, mais um hint mandando usar
  `recall(scope=…)` — que também dava 0. Pior, o sub-modo `entities` **já honrava**
  dims (roteado para `recall_entities`), então dois usos do mesmo tool se
  comportavam diferente. Mesma família do `view=index` do v1.1.23, invertida: o
  modelo lê o schema, não o código. Agora o caminho principal roteia para
  `recall_lexical_dims`/`recall_scoped_dims` (dims vencem o `scope` legado, que é
  menos específico), a resposta ecoa o `scope_dims` efetivo, e onde o core não
  tem rota (`hybrid` RRF, `temporal`) a chamada é **recusada com erro acionável**
  em vez de responder o pool global (que vazaria escopo).
- **A invariante do ADR-0013 quebrava na escrita com AMBOS os escopos.**
  `remember(scope="proj/x", scope_run="r1")` gravava `scope="proj/x"` com
  `dims.user == ""`: o `finish_remember` promovia o legado para `dims.user`
  apenas quando as dims estavam TODAS vazias, então o `set_scope_dims` seguinte
  clobberava o write-through do `set_scope`. Como `effective_scope_dims` prefere
  dims não-globais, a dimensão `user` era **perdida**: quem filtrasse por
  `scope_user` não achava a memória e quem filtrasse por `run` a achava sob
  outro user. A promoção agora preenche um `user` vazio sempre que `scope` é
  não-vazio. Teste com mutação provada.

### Changed
- **`scope_distribution()` e `scope_distribution_dims()` passam a ser projeções
  de uma classificação única** (`scan_scope_metas`), e o rótulo de dims vem de
  `ScopeDims::label()` — a mesma regra estava copiada em três funções e a
  formatação em duas.
- **`scopes_to_probe` não é mais truncado.** Era derivado de
  `health.scope_labels`, que faz `truncate(8)` por EXIBIÇÃO — o 9º escopo mais
  populoso era indescobrível ao cold-start. Descoberta não pode ser limitada
  por uma decisão de formatação.

### Testes
- 9 asserções novas no hot test (fase 7): `scope_run` alcança o doc gravado,
  run errado não vaza, global não vaza, `hybrid`+dims recusa, `global_memory_count`
  não se move, `scopes_to_probe_dims` publicado, e `scope`+`scope_run` juntos
  concordam. Hot test **119/0**.
- 3 testes de lib novos (um com mutação provada). O exemplo
  `scope_blind_spot_probe` foi promovido a teste de regressão e removido.
- Matrix **342+1 / 388+1 / 286+1** (default / p2p / no_std); clippy/rustdoc
  `-D warnings` e no_std bare-metal verdes; `agent_protocol` 25/0,
  `two_ai_protocol` 16/0, `audit` 25/0.

## [1.1.25] — 2026-09-22 (payload_type honra a camada)

Sem mudança de formato (NMD1/TKLV intocados, MDM1 sem bump). `MCP_CONTRACT_VERSION`
→ **1.1.25**: o VALOR de um campo visível ao consumidor mudou (`Hit.payload_type`
no `format=json`), e dois builds diferentes não podem se chamar 1.1.24.

### Fixed
- **`payload_type` declarava `Embedding(dim)` para prosa L3.** O campo promete "o
  datum REAL do payload"; a regra em uso era a do `index_doc`
  (`len % 4 == 0 && len >= 4`), que **qualquer texto de tamanho múltiplo de 4**
  satisfaz — 25% deles. Um `remember(text="dezesseis bytes!")` (16 B) respondia
  `payload_type = Embedding(4)`, e um consumidor máquina que confiasse no campo
  tentaria reusar um vetor que não existe — exatamente o erro que os hits
  tipados (v1.1.6) existem para evitar. Agora só **L4/L5** (as camadas que o BQ
  indexa) rendem `Embedding`; fora delas o payload passa pelo detector normal
  (Text/Json/Code/Binary). Descoberto **usando** o DB (flush do harness contra o
  banco real), não lendo código.
- A decisão — que estava duplicada em **quatro** call sites (recall semântico,
  `recall_impl_dims`, recall de entidades e `primary_of`, este último com um
  branch explícito para L3) — virou uma função nomeada em `src/ctype.rs`:
  `payload_content_type(key, payload, has_bitvec)` + `key_carries_embedding(key)`.
  `embedding_dim_of` ganhou a precondição de camada na doc.

### Testes
- `payload_type_honors_the_layer_not_the_byte_count` (ctype): L3/L2 de prosa de
  16 B → Text; o MESMO payload em L4 → `Embedding(4)`; L5 com bitvec →
  `Embedding(4)`; L3 não-UTF8 continua Binary; L4 com payload curto não inventa
  vetor.
- `l3_prose_is_never_typed_as_embedding_in_any_path` (sgdb): cobre os caminhos de
  entidades, lexical, `primary_of` (companion L2 com primário L3) e confirma que
  o semântico L4 segue reportando o vetor. **Verificado com mutação**: removida a
  guarda de camada, o teste falha com a asserção exata; restaurada, passa.
- Matrix **339+1 / 385+1 / 283+1** (default / p2p / no_std); clippy/rustdoc
  `-D warnings` e no_std bare-metal verdes; hot test **110/0**.

## [1.1.24] — 2026-09-22 (unificação de escopo + ledger de negativos)

Release **3/3** do plano de melhorias (itens 4 e 7) — os dois únicos itens que
**mudam comportamento**, por isso por último e abertos com characterization
test. Sem mudança de formato (NMD1/TKLV intocados, MDM1 sem bump).
`MCP_CONTRACT_VERSION` → **1.1.24** (4 aliases novos na superfície anunciada).

### Added
- **Ledger de negativos (item 7, [ADR-0014](docs/adr/0014-negative-ledger.md))**
  — "o que já verifiquei e NÃO existe". Side-table
  `sys/negative/<fnv1a64(scope‖0x1f‖query):016x>` (wire de valor `NDG1`,
  **não** é wire type; NMD1/TKLV intactos), com `src/negative.rs`
  (`AbsenceEntry`, `normalize_query`, `negative_key`). APIs:
  `note_absence` (reforça em vez de duplicar), `forget_absence`,
  `recall_absences` (recentes primeiro, null-scoping respeitado),
  `absence_count`, `prune_absences` (retenção do host; `0` desliga) e
  `recall_with_ledger` — probe lexical + ledger numa chamada, com
  **self-healing**: ao achar memória, a ausência registrada é removida.
  Identidade normalizada pelos tokens do BM25 (caixa/pontuação não fragmentam;
  paráfrase e acento ainda fragmentam — documentado). Escopado: a ausência de um
  tenant nunca vira evidência do outro. MCP: `recall_ledger`,
  `recall_absences`, `note_absence`, `forget_absence` (JSON **próprio**, o shape
  de `format=json` dos hits fica intacto).

### Changed
- **`ScopeDims` é AUTORITATIVO; o `scope` legado é o espelho de `user`
  (item 4, [ADR-0013](docs/adr/0013-scope-dims-is-authoritative.md)).**
  O read-side já unificava (o decode promove o legado para `user` com dims
  vazias; `effective_scope_dims` faz o mesmo nos filtros) — o desacordo morava
  nos **bytes**: `set_scope` gravava só o campo legado, então o `sys/meta/`
  persistido tinha as 4 dims vazias e `encode(decode(raw)) != raw`. Quem lesse
  `scope_dims` como campo (outra implementação, um peer, o OS) via escopo
  global. Agora `set_scope` faz **write-through** nos dois campos, o storage
  fica canônico e o invariante `scope == scope_dims.user` vale nos dois
  sentidos. A promoção no decode **permanece** (registros pré-v7 e payloads de
  peer seguem corretos) — é canonicalização de escrita, não reinterpretação de
  bytes.
- **`validate()` §6 novo**: sinaliza `legacy scope disagrees with scope_dims.user`
  em qualquer `sys/meta/` de primário com os dois campos preenchidos e
  divergentes. O caminho de API não consegue mais produzir esse estado; uma meta
  montada à mão (`put` com `doc.meta`) ou importada de um escritor divergente
  agora é **observável**. `scope_dims_of` documentado como o acessor
  autoritativo; `scope_of` como sua projeção `user`.
- **Caracterization test antes de tocar no comportamento**: o teste pinou o
  estado v1.1.23 (dims vazias no storage, `encode∘decode != raw`, distribuição
  `user/ana///`), foi verificado verde **antes** da mudança e atualizado junto,
  com o "antes" medido preservado no cabeçalho do teste e no ADR-0013.

### Testes
- Matriz de testes: **337+1 / 383+1 / 281+1** (default / p2p / no_std);
  hot test MCP **110/0** (fase nova "ledger de negativos", 6 asserções);
  protocolos `agent_protocol` 25/0, `two_ai_protocol` 16/0, `audit` 25/0,
  `memory_arena_eval` PASS. 7 testes novos no lib (1 de escopo +
  3 de ledger + 3 do codec em `src/negative.rs`).

## [1.1.23] — 2026-09-21 (revisão de documentação + fix do schema anunciado)

Sem mudança de formato (NMD1/TKLV) e **sem mudança de comportamento**. Bump de
contrato porque a superfície MCP **visível** mudou (o `enum` anunciado).

### Fixed
- **`health(view=index)` era indescobrível (bug de contrato real).** O handler do
  v1.1.21 servia o view e o hot test o exercitava, mas `tools/list` anunciava o
  `enum` **sem** `index` — e o modelo só lê o schema. A feature existia e estava
  invisível. Corrigido no `enum` + na descrição da tool, com guard no hot test:
  **o enum anunciado deve listar TODO view que o handler serve** (asserção nova,
  falha se divergirem). É o caso mais claro do problema que este repo já tratou
  antes: prosa/superfície divergindo do código sem nada quebrar.

### Changed (auditoria de documentação)
- **Versões paradas em `1.1.20`/`1.1.17`** corrigidas em `README.md`,
  `VERSIONING.md`, `ROADMAP.md`, `CLAUDE.md`, `docs/api.md`,
  `docs/architecture/*` (status dos 6 docs) e `docs/harness-prompts.md` — a
  linha **v1.1.21 não tinha atualizado README/VERSIONING/api/architecture**, e
  os números de matriz de teste tinham ficado dois releases atrás.
- **`docs/api.md`**: a superfície aditiva estava rotulada `v1.1.2–v1.1.17`;
  agora cobre até `v1.1.22` e documenta de fato o `index_fingerprint`,
  o contrato de custo de `open`, `recall_adaptive`/`AdaptiveRecall`/`RecallProbe`.
  O placeholder `pub struct RecallProbe;` que eu tinha deixado no bloco de
  `recall` foi substituído pela definição real.
- **`docs/architecture/03-retrieval-architecture.md`**: seção nova sobre quanto
  esforço um recall merece (ADR-0012), incluindo a medição que mostra que o
  threshold default escala quase sempre.
- **`docs/architecture/06-cognitive-api.md`**: observabilidade atualizada
  (custo de `open`, §5 do `validate`, `index_fingerprint`, `recall_adaptive`) e
  o contrato MCP corrigido (dizia `1.1.10`, e listava `health(view=validate)` —
  `validate` é tool/alias, não view).
- **Números de teste** alinhados ao estado corrente nos 4 lugares onde apareciam
  (`AGENTS.md`, `CLAUDE.md`, `README.md`, `ROADMAP.md`): **330+1 / 376+1 /
  274+1**, hot test **103/0**.
- **`MIGRATIONS.md` — o registro de formato estava MENTINDO**: dizia `MDM1 v6`
  quando o código tem `META_VERSION = 7` (v1.1.15, `scope_dims` + `model_id`) —
  contradizendo o próprio `docs/interop-os.md`. Faltavam também a entrada da
  migração `v6 → v7`, três side-tables no registro (`sys/audit/` v1.1.10,
  `sys/ttl/` e `sys/event/` v1.1.15) e o wire type `AUD1` na tabela de
  formatos. Num arquivo cuja única função é ser autoritativo sobre versão de
  formato, isso é o defeito mais alto da auditoria depois do schema do MCP.
- **`src/wire_fuzz.rs` dizia "8 wire types" e listava 8** — o `AUD1` (v1.1.10)
  entrou no harness e a enumeração do module-doc não acompanhou (o teste
  decodifica 9). Corrigido no doc do módulo e nos 4 lugares que repetiam o
  número (`AGENTS.md`, `docs/api.md`, `src/codemap.md`, `ROADMAP.md`).

## [1.1.22] — 2026-09-21 (math consolidado + recall adaptativo)

Sem mudança de formato (NMD1/TKLV). **`MCP_CONTRACT_VERSION` → 1.1.22**
(superfície MCP inalterada — o pin marca a build do server). Release 2 de 3 do
plano de melhorias (itens 5 e 8).

### Added
- **`Sgdb::recall_adaptive` / `recall_adaptive_scoped` + `AdaptiveRecall` +
  `RecallProbe` (ADR-0012, item 8)** — o esforço de recall passa a ser
  consequência da AMBIGUIDADE da fronteira do top-k, não de uma constante.
  Degraus `1 → 4 → 8 → 16` (cortados no teto `max_oversample`); em cada degrau
  o core pede `k+1` hits (o sentinela de fronteira) e para quando
  `|score(k) − score(k+1)| > SCORE_TIE_MARGIN`, medido no **score cru u32** e
  não no bucket `score/(MARGIN+1)` do state-first — buckets têm bordas
  arbitrárias e dois docs a 0.0026 de cosseno caíam em buckets diferentes,
  parecendo "decididos" (pego pelo teste que o próprio item exigia).
  **`RecallProbe`** distingue **"o store acabou"** de **"o pool acabou antes
  dos docs válidos"** (`survivors <= k` só é decisivo se `!saturated()`): sem
  isso, um `scope` cujos docs ficam fundo na ordem do BQ devolveria top-k
  truncado em silêncio. Opt-in por desenho — escalar adiciona candidatos e
  PODE mudar a ordem, então o `recall` default fica intacto
  (`max_oversample = 1` degrada para o comportamento clássico).
  `recall_impl` virou wrapper fino sobre `recall_impl_probe` (mesma passada,
  dois contadores de observabilidade a mais). **Veredito MEDIDO** (2 regimes,
  `examples/bench.rs`): em clusters densos 44% de recall@5 (vs 40% do dual
  16× fixo) a 338.6 candidatos/query contra ~160 de uma passada única;
  62% das queries de um corpus espalhado ainda escalam até o teto. Ou seja:
  **o threshold default (`SCORE_TIE_MARGIN`) faz "ambíguo" ser o caso comum**
  — o valor entregue é o INSTRUMENTO (`oversample_used` / `escalations` /
  `boundary_decisive` / `probe`), não uma promessa de ganho. Os 8 testes pinam
  o mecanismo: recuperação do melhor fora do pool barato, no-op quando
  decisivo, convergência ao cap, determinismo, escopo, entradas degeneradas e
  ausência de regressão no `recall` default.

### Changed
- **Math consolidado em `src/math.rs` (item 5)** — `sqrt_f32`/`ln_f32`/
  `exp_f32` saíram de `sgdb.rs`/`lexical.rs` para um módulo único (o `sgdb.rs`
  re-exporta `pub(crate)`), **movidos sem tocar na aritmética**: o teste
  `bm25_ranking_is_frozen_across_math_move` congela ordem E scores do BM25,
  que é onde `ln_f32` entra (IDF e TF). `Embedder`/decay/rescore seguem nos
  mesmos call sites.

### Fixed
- **Doc mentirosa do `ln_f32`** — o comentário prometia "precisão ~1e-5"; o
  erro relativo MEDIDO é **5.9e-2** (x=1.85), três ordens de grandeza pior.
  Não é bug (o BM25 só precisa de ORDEM consistente, e o ranking está
  congelado em teste), mas a documentação agora carrega o número real e os
  testes comparam contra `std` sob bound de regressão (`worst < 1e-1`).
- **Gate `--no-default-features` quebrado pela mudança de math**: os testes de
  `math.rs` usam `println!` + `f32::ln`/`exp` (que NÃO existem no core
  bare-metal) e o módulo de testes do `lexical.rs` usava `vec!` sem import —
  ambos passaram a ser explicitamente host-only (`#[cfg(all(test, feature =
  "std"))]` / `use alloc::vec`). Matrix no_std de volta a verde.

## [1.1.21] — 2026-09-21 (integridade do índice derivado + medição de open)

Sem mudança de formato (NMD1/TKLV). **`MCP_CONTRACT_VERSION` → 1.1.21**.
Release 1 de 3 do plano de melhorias (itens 1, 2, 3, 6): tudo aditivo, nada
muda o comportamento do recall.

### Added
- **`Sgdb::index_fingerprint()` + ADR-0011** — oráculo canônico do estado
  derivado. Invariante central: **`fp(open) == fp(rebuild_indices())`**, mais
  ordem de inserção irrelevante, deleção simétrica, equivalência
  `export → import` e estabilidade entre reopens. **Três exclusões
  deliberadas**: ids do ART/BQ (`NEXT_ID` é global de PROCESSO — o mesmo
  corpus dá ids diferentes a cada open, então hashear id não seria estável
  entre processos: o fingerprint resolve `id → storage_key` e hasheia a
  chave), órfãos do BQ (inertes por design; hashear `bq.len()` quebrou a
  simetria de deleção — pego pelo teste) e floats. MCP: `health(view=index)`.
- **`Sgdb::validate()` §5 — invariante de `corpus_mean` (ADR-0011 §float)**: a
  soma mantida incrementalmente é comparada com a recomputada do storage;
  **counts por igualdade exata** (divergência é bug duro), **somas com
  tolerância relativa `1e-9`** — a adição `f64` não é associativa e o rebuild
  acumula noutra ordem, logo igualdade exata daria falso positivo em
  produção. `Engine::corpus_sums_drift` / `recompute_corpus_sums` reusam a
  MESMA acumulação (`corpus_add`) e a MESMA leitura de payload
  (`payload_floats_truncated`, extraído do ramo "sem bitvec" do `index_doc`)
  — comparar dois critérios diferentes seria inútil por construção.
- **Métricas de `open` (ADR-0009 §4 — contrato de medição)**:
  `open_rebuild_ms_last` / `open_rebuild_ms_max` / `opens`, expostos em
  `Metrics::snapshot`, `HealthReport` e `health(view=index)`. Mede o rebuild
  de índices que domina o cold start (`no_std` reporta `0`: não existe
  `Instant` no core do alvo — ausência declarada, não número falso; nenhum
  seam global de relógio foi inventado). `opens` é contador de PROCESSO
  (host conta o histórico entre restarts, como o ADR atribui).
- **Superfície de alias como TABELA** (`ALIAS_SURFACE`, 34 nomes) + **erro
  enriquecido**: tool desconhecida continua sendo erro `-32602`, mas com
  `data.did_you_mean` (prefixo/substring, senão distância de edição ≤3, máx. 3
  sugestões, determinístico) + `listed_tools` + `alias_count`. A contagem
  documentada era **23** (rework v1.1.8) e estava velha — cresceu com as ops
  cognitivas (v1.1.10) e o harness (v1.1.19); agora é pinada por teste.

### Notas
- Sem mudança de comportamento de recall; nenhuma tool nova LISTADA (segue 4).
- `BENCHMARKS.md`: seção "Open cost" com o custo medido `open` vs rebuild
  (12800 docs → 156 ms total, 119 ms de rebuild = **76%**).
- Verificado: lib **315+1**, p2p **361+1**, no_std **265+1**; clippy e rustdoc
  `-D warnings` verdes; bare-metal ok; hot test **102/0** (2 asserções novas).

## [1.1.20] — 2026-09-18 (null-scoping ScopeDims + harness tests)

Sem mudança de formato (NMD1/TKLV). **MCP_CONTRACT_VERSION → 1.1.20**.

- **Null-scoping + `ScopeDims`:** `allows_scope_filter` — recall global
  (`scope=None`) exige `effective_scope` vazio **e** `dims.is_global()`.
  Memórias só com `scope_run`/`scope_user`/… deixam de vazar no lexical/
  semântico/entities default (ADR-0010 / doutrina).
- **`recall_*_dims`:** pool sem filtro legado (`enforce_legacy_scope=false`);
  filtro multi-dim aplica-se depois (evita pool vazio pós-fix).
- **`consolidate_recurrences`:** L3 consolidado herda `ScopeDims` do grupo
  (não vaza global).
- Harness: +8 testes (isolamento A≠B, idempotência, TTL, companion L4,
  anti-pattern dedup, null-scoping, consolidate scoped).
- `scripts/mcp-install.ps1`: rename-then-copy se o `.exe` estiver locked.
- Docs: [`docs/harness-prompts.md`](docs/harness-prompts.md) — prompts
  globais/auditoria corrigidos (sem `Deprecated`; anti-pattern via entities;
  `commit_run` shipped); self-program + doctrine + skill alinhados;
  [`docs/interop-os.md`](docs/interop-os.md) — paridade NMD1 com neural-os-core.
- Verificado: lib **292/0**, p2p **338/0**, no_std **244/0**; hot test
  **100/0**; bare-metal ok. Embedder host default permanece unset/`none`.
- **Gate de clippy restaurado (2026-09-21):** o código do ADR-0010 entrou com
  4 erros de lint — `derivable_impls` / `match` usado como teste de igualdade /
  `manual_contains` em `src/harness.rs` e `redundant_closure` em
  `examples/mcp_server.rs` — e `clippy --all-targets --all-features -D warnings`
  **não fechava no `main`**. Correções semanticamente neutras: derive `Default`
  em `CommitRunPlan<'a>` (mais geral que o impl manual `'static`), `if`
  colapsado, `contains`, `map_err(mcp_actionable_error)`. Sem mudança de
  formato (NMD1/TKLV) nem de contrato MCP — `MCP_CONTRACT_VERSION` permanece
  **1.1.20**. Ver [`docs/hot_test.md`](docs/hot_test.md) §2026-09-21.

## [1.1.19] — 2026-09-18 (ADR-0010 harness: commit_run)

Sem mudança de formato (NMD1/TKLV). **MCP_CONTRACT_VERSION → 1.1.19**.

- **`src/harness.rs`**: `commit_run`, `deprecate_run`,
  `consolidate_recurrences_scoped`, `remember_episodic_scoped`;
  entity canónica `mom/anti-pattern`.
- MCP `curate` ops `commit_run` / `deprecate_run`; episodic honour
  `scope_*`; `recall_entities` aceita `scope_run` via `ScopeFilter`.
- Docs: ADR-0010 marked implemented; agent-self-program MOM table;
  architecture lifecycle/cognitive API.
- Tests: 6 harness unit + hot-test fase harness.

## [1.1.18] — 2026-09-16 (telepathy 2-DB + host harvest + MCP 1.1.18)

Sem mudança de formato (NMD1/TKLV). **MCP_CONTRACT_VERSION → 1.1.18**.

- **`examples/telepathy_two_db`**: dois `FileStorage` distintos + CRDT +
  recall cruzado + supersede/validade + **reopen** do disco. Features
  `p2p` + `file-storage`.
- **Verificado (host harvest):** lib **278/0**, p2p **324/0**, no_std **230/0**;
  bare-metal OK; `agent_protocol` **25/0**; hot test **96/0**.
- Docs: `telepathy-pt.md` / `telepathy.md` §5; `scripts/telepathy-demo.ps1`
  corre InMemory + 2-DB.
- **Agent self-program (qualquer LLM/IDE):**
  [`docs/agent-self-program.md`](docs/agent-self-program.md) — checklist de
  auto-config, cold-start, pacote de memória, mapa de capacidade, snippets
  user-rule/system-prompt; skill `.cursor/skills/nsgdb-full-usage/`; ponteiros
  em `doctrine.md`, `MCP.md`, `README`, `.cursor/rules/nsgdb-agent.mdc`.
- **Host harvest 1–4:** ontologia MOM (`mom/*`); surprise→reinforce em
  `agent_protocol`; paging host `page_context`; `Sgdb::staleness_report` +
  `health(view=staleness)` (read-only). MCP contract pin **1.1.18**.

## [1.1.17] — 2026-09-16 (recall quality: ADC-lite dual-path + state-first)

Sem mudança de formato (NMD1/TKLV intactos), sem mudança de contrato MCP
(tipos/rotas idênticos); APIs novas aditivas. **MCP_CONTRACT_VERSION → 1.1.17**.

- **ADC-lite dual-path** (`sign(q − mean)`): engine mantém a média dos
  embeddings L4/L5 por dim (`corpus_sums`: insert soma, delete/overwrite
  subtrai, rebuild reconstrói); recall une top-k legado + top-k centrado e o
  rescore FP32 decide. Bitvecs intactos (era/`words_per_vec`/S1 intactos).
  Medido (`examples/bench.rs`, mesma harness): recall@5 22→24 / 22→25 /
  24→28 / 30→34 / 35→40% (oversample 1/2/4/8/16×). APIs:
  `quantize_f32_minus_mean`, `top_k_f32_minus_mean`, `bq_top_k_f32_dual`,
  `Sgdb::corpus_mean`.
- **State-first**: ordenação final `(grupo de score, created_tick desc,
  score, key)` — margem `SCORE_TIE_MARGIN=50` (≈0.005 cosseno); versão
  corrente vence empates, conteúdo distinto continua dominando. Vale para
  `recall_impl` e `recall_impl_dims`. Teste `recall_tie_break_by_key`
  atualizado para o novo contrato.
- **13 testes de validação rec2+rec3** (edge cases, interação, regressão):
  dual-path: corpus vazio, single doc, query=mean, dims mismatch, dedup,
  mean preservada em rebuild (FileStorage). State-first: same tick→key,
  content dominates, superseded filtered, deterministic. Interação:
  dual-path honra state-first. Paridade recall/hybrid_rrf. Regressão:
  1-dim e weighted. Matrix: **275+1 / 321+1 / 227+1**, gates verdes.

## [1.1.16] — 2026-09-10 (AI-user audit: hardening + write-amplification fix)

Avaliação agêntica completa (gates + stress/bench + protocolos + simulação de
loop de agente). Duas correções com regressão; sem quebra de API (PATCH):

- **no_std gate restaurado**: `wasm_backend` sem entrada `[[example]]` era
  auto-buildado em `cargo test --no-default-features` e falhava
  (`SgdbError: std::error::Error`) — `required-features = ["std"]`. Matriz:
  **257+1 / 303+1 / 209+1**.
- **Bomba de write-amplification no lineage (crítico p/ agentes)**:
  `persist_meta` empilhava 1 parent por overwrite do mesmo slot e re-encodava
  o meta inteiro — `remember_exchange` (last_user/last_asst, o caminho mais
  quente do agente) crescia O(turns²): meta 68 KB aos 2k turnos, ~13 GB de
  lixo aos 20k (OOM no stress), 22.5 ms/op. Janela `MAX_PARENT_IDS` (64,
  `limits.rs`) mantém os ancestrais MAIS RECENTES; histórico completo
  permanece em `sys/version/`. Pós-fix: meta 2.3 KB flat, **68.6 µs/op
  (35×)**, 45 MiB em vez de 1.6 GiB no mesmo workload. Teste:
  `overwrite_lineage_window_bounded`.
- **Novo harness**: `examples/agent_sim.rs` — loop de agente real (latência
  P50/P99 por turno, exact vs paraphrase, isolamento de scope). Paraphrase
  MISSA no lexical (BM25, by design). Medição de gap (roadmap, não bug):
  `Sgdb::open` rebuilda índices linearmente (~16 ms/500 docs) — falta
  snapshot de índice no estilo fast-mount do TKCK.
- **MCP**: `MCP_CONTRACT_VERSION` → 1.1.16; pin do `mcp_client` no mesmo
  commit (disciplina do AGENTS.md — hot test 95/0).

## [Unreleased]

- **Docs ADR hygiene:** ADR-0007 updated — `model_id` / `mixed_models` are
  **shipped** (MDM1 v7), not “optional future”; architecture `01-memory-model`
  documents MDM1 v7 + side-tables ttl/event/audit; ADR-0008 consequence line
  aligned; index ADR-0007 row refreshed.
- **ADR-0009** — Index snapshot on open; metrics-gated auto-adapt (design
  Accepted; implementação ROADMAP Next). Ver
  [`docs/adr/0009-index-snapshot-auto-adapt.md`](docs/adr/0009-index-snapshot-auto-adapt.md).
- **Parked (browser):** extensão / Chrome Web Store **não** são produto nem
  gate de release. Auto-captura em `extension/background.js` comentada.
  Foco: NSGDB **agêntico** — MCP 4 tools, doutrina (`nsgdb://doctrine`),
  `agent_protocol` / `two_ai_protocol` / `memory_arena_eval`.
- **Hot test pin:** `examples/mcp_client.rs` `serverInfo.version` deve
  igualar `MCP_CONTRACT_VERSION` (**1.1.17**). Bump sem pin → 94/1.
- **Agent ops (MCP):** `nsgdb://session` ganha `cold_start` (scopes_to_probe,
  single_writer, telepathy_when, embedder_host); onboarding multi-agente;
  regra `.cursor/rules/nsgdb-agent.mdc`; `docs/MCP.md` §cold-start;
  `scripts/telepathy-demo.ps1`. Fix example `p2p_telepathy`: key `rev_of_m1`
  (ADR-0005 — `m1b`/`m1_rev` eram prefixos de `m1`). Snapshot de índices →
  **ADR-0009** / ROADMAP Next.

## [1.1.15] — 2026-09-08 (leva 2: TTL/GC + timeline + ANN + WASM)

MDM1 v7 (v1.1.14) + governança/temporal/escala:

- **MDM1 v7**: `ScopeDims{user,agent,app,run}` + `model_id` (ADR-0007 fecha
  misma-dim-mesmo-modelo: veredito `mixed_models`); legado `scope` mapeia
  `user`; decode v1-7.
- **Retrieval**: `recall_hybrid_rrf(_scoped)` (RRF k=60), `Reranker` seam +
  `LexicalAnchorReranker` + `recall_reranked`, `recall_scoped_dims/
  lexical_dims/entities_dims`, `recall_ann_ivf` (IVF-Flat) + `ann::HnswLite`.
- **Temporal/GC**: `sys/ttl/` (`set_ttl/ttl_of/expire_ttl`), `sys/event/`
  (`set_event/event_of/close_event/recall_timeline`), `GcConfig/GcReport/
  collect_garbage`; `validate` cobre `sys/ttl|event`; `delete` limpa.
- **WASM**: `SnapshotStorage::to_bytes/from_bytes` + `src/wasm_storage.rs`
  + `examples/wasm_backend.rs`; `nsgdb-embed` 384 local (`LOCAL_MODEL_ID`).
- **MCP**: `remember/recall(scope_user/agent/app/run, model_id)`,
  `hybrid=RRF`, `curate=set_ttl|expire_ttl|set_event|close_event|timeline|
  gc|recall_ann`; contract `1.1.15`.
- Gates: 256 lib / 302 p2p / 208 no_std +1 doc-test, no_std target ok,
  clippy lib `-D warnings`, doc `-D warnings`, `wasm_backend` OK.

## [1.1.14] — 2026-09-08 (MDM1 v7 base, RRF, rerank, embedder)

## [1.1.13] — 2026-08-21 (chrome web store obrigatório)

Docs: `VERSIONING.md` passo 6 torna obrigatório republicar `extension/` na Chrome Web Store a cada nova versão `v*`; `extension/README.md` com sync `manifest.json` `version` = `Cargo.toml`.

## [1.1.12] — 2026-08-21 (security hardening 11→1 ordem inversa de custo)

Hardening baseado nas premissas (no_std zero deps, NMD1/TKLV byte-idêntico, sem crypto no core) em ordem inversa de custo (do BAIXO ao ALTO): 11 remember_episodic docs, 10 lexical cap 1024 termos (DoS), 9 `MemoryDoc::try_encode()` com `MAX_KLEN`/`MAX_VLEN`, 8 `LocalEmbedder::new()->Result` Err se `0`/`>MAX_EMBEDDING_DIM` (era clamp silencioso), 7 `backfill_helper` sem duplicar `md/L4/md/L4/`, 6 `write_side_bytes` host-only docs, 5 MCP `-32601` já tinha, 4 `WasmStorage::put` com `validate_ws_key`, 3 `engine.put_inner` choke central (fecha import/merge_memories/remember_episodic), 2 `associate` valida `a`/`b` completo, 1 `merge_memories target` via 3. Gates 243/289/195+1 clippy/no_std/doc hot 95/0. Rebased com `3cc3010` fix simd.

## [1.1.11] — 2026-08-21 (host governance + micro-ganhos core)

Stable **1.1.10** guardada como tag `v1.1.10` (BENCH_STABLE_1.1.10.txt).
Bench baseline em `BENCHMARKS.md` §Environment 2026-08-13 + nova captura.

**Host (sugestões sem quebrar contratos):**
- `examples/host_scheduler.rs` — daemon que governa: `expire_old` + `decay_importance` (Ebbinghaus) + `consolidate_recurrences` + `audit_checkpoint`/`audit_verify` + `health`/`validate`. 5/5 checks PASS.
- `examples/backfill_helper.rs` — migra L3 lexical → L4 semântico via re-embed do texto preservado em `md/L2/<id>` + `rebuild_indices()` (caminho explícito de era migration, não `remember_semantic` guarded).
- `Storage::put_many` + `FileStorage::put_batch` — batch `write_all` único para `remember_exchange` (L1+L2) e scheduler; `Flushed` por lote, não por record.

**Core micro-ganhos (sem quebrar NMD1/TKLV/MDM1, no_std, zero deps):**
- **MG1** `sgdb.rs:recall_weighted_full` — `select_nth_unstable_by` + truncate para top-k (`O(N)+O(k log k)` vs `O(N log N)`), heap já existia em `BqFlatIndex::top_k`.
- **MG2** `lexical.rs:search` — dedup de query termos + `search_fast` sem `matched_terms` (evita `Vec<String>` por hit no rerank interno).
- **MG3** `hamming_dispatch.rs` — `#[inline(always)]` em `hamming`/`hamming_1024`/`ensure_selected`/`path_name` + `active_kernel` (hot path).
- **MG4** `storage.rs` — `FileStorage::put_batch` + `Storage::put_many` trait (default loop, FileStorage sobrescreve).

Gates: 243+1 / 289+1 / 195+1, clippy `-D warnings`, no_std `x86_64-unknown-none`, `cargo doc -D warnings`, hot test 95/0.

### Added (host layer — does **not** bump crate SemVer)
- **`connectors/`** — adapters claw — `mcp_server` without touching `src/`,
  NMD1, or TKLV. Shared Python MCP/stdio client (handshake `2025-11-25`,
  cooperative `.connector.lock`), Hermes `MemoryProvider` (tools + bounded
  auto-recall; auto-capture OFF by default), OpenClaw TypeScript skeleton,
  contract tests (`python -m unittest discover -s connectors/tests -v`, 4/4).
  Docs: [`connectors/README.md`](connectors/README.md). Crate remains **1.1.9**.

## [1.1.10] — 2026-08-20 (cognitive metadata: decay, consolidation, audit)

Crate now **1.1.10**. Five future-horizon items landed (each with a lib
regression + hot-test coverage; matrix **243+1 / 289+1 / 195+1** green,
clippy `-D warnings`, `no_std` bare-metal, `cargo doc -D warnings`, hot test
**95/0** exit 0).

### Added
- **Item 1 — Ebbinghaus decay** `Sgdb::decay_importance(now, &DecayConfig)`:
  importância decai exponencialmente com a idade (desde `last_reinforced` ou
  `created_tick`) em direção ao `floor`, sem nunca subir; abaixo de
  `decay_state_at` o estado vira `Decayed` (recall active-only ignora,
  história preservada). Idempotente para o mesmo `now` (relógio = input).
  `exp_f32` (IEEE + série, no_std-safe) junto de `sqrt_f32`/`ln_f32`.
- **Item 2 — Consolidação por recorrência** `Sgdb::consolidate_recurrences`:
  episódicos L2 (`md/L2/<ts>/…`) com MESMO texto normalizado (BM25 tokenize)
  repetido `min_repeats`+ → fato L3 determinístico
  (`md/L3/consolidated/<fnv1a>`) VERBATIM do episódio mais antigo, com
  `parent_ids` (version_ids) + relação `derived_from`. Dedup idempotente
  (mesmo payload = no-op, sem bump de versão); churn bounded por `max_new`.
- **Item 3 — Breakdown de score + ponderação** `Sgdb::recall_weighted_full`:
  pesos por sinal — semântico, recência, importância, **confiança** e
  **fonte** — e `Hit.score_breakdown` expõe cada sinal em penalidade + o
  total (o consumidor vê o "porquê" do ranking). `trust: &[(node_id, 0..1)]`
  penaliza fonte não confiável (p2p); fora do mapa = 0.5 neutro.
  `recall_weighted` legado delega (w_conf=w_src=0 → comportamento idêntico).
- **Item 5 — Auditoria hash-chain + rollback** `src/audit.rs` (wire `AUD1`):
  `Sgdb::audit_checkpoint(now)` anexa elo `sys/audit/<seq:016x>` com
  `prev_hash` (FNV-1a do elo anterior), `digest` (FNV-1a do estado ordenado
  docs+side-tables) e SNAPSHOT das side-tables cognitivas;
  `Sgdb::audit_verify()` → `AuditReport` (chain_intact + digest_matches_last
  — tamper-evidence sem cripto, ADR-0006); `Sgdb::rollback_to(seq)` restaura
  `sys/meta/`+`sys/state/`+`sys/validity/` e remove metas de memórias criadas
  após o checkpoint (payloads ficam — ADD-only; undo de conteúdo é o DAG
  causal). Ledger cobre o `wire_fuzz` (nunca-panic/roundtrip/truncation/magic).
- **Item 6 — Write-path hardening** `validate_written`: rejeita chave/
  scope/entidade com path traversal (`..`/`.`), NUL/control chars, separador
  reservado `#` (sys/rel/) ou acima de `MAX_KLEN` — em `remember_semantic`,
  `remember_text_with`, `set_scope`, `set_entities` e `Sgdb::put`. Nada é
  gravado (MPBench/MemAudit lesson).
- MCP `curate` (ferramentas listadas continuam **4**) ganhou ops
  `decay`/`consolidate`/`audit_checkpoint`/`audit_verify`/`rollback_to`
  (schema + dispatch; hot test fase 6b).

## [1.1.9] — 2026-08-20 (ADR-0008: lexical-first MCP)

### Changed
- **MCP default recall is lexical** when the caller does not pass `embedding`.
  `mode=semantic` / `hybrid` require a vector or explicit
  `NEURAL_SGDB_EMBEDDER=demo`.
- **`remember(text=)` without a vector writes L3** (`remember_text_with`) —
  does not open a fake BQ era. L4 only with `embedding=` or demo embedder.
- **ADR-0008 Accepted** (was Proposed): embeddings are a host-side era.

### Added
- MCP resource **`nsgdb://session`** — cold-start JSON (health + tensions).
- **`health(view=tensions)`** — conflicts, superseded keys, unseen scopes.
- Core **`Sgdb::remember_text_with`**.

### Fixed
- Launchers and `.cursor/mcp.json` no longer set `NEURAL_SGDB_EMBEDDER=demo`
  by default (that made host embedder “explicit” and kept L4 writes).

### Notes
- Hot test: **84/0**. Lib tests **235+**.
- MCP `tools/list`: 4 tools; 23 legacy names remain `tools/call` aliases.

## [1.1.8] — 2026-08-20 (agent doctrine + 4 MCP tools)

### Added
- **`docs/doctrine.md` / `src/doctrine.rs`**: compact protocol for the LLM above
  (memories not RAG, null-scoping, ADD-only, two-pass, typed hits, era).
- **`Sgdb::ensure_doctrine(emb)`**: idempotent seed at `md/L4/nsgdb/doctrine`,
  `scope=nsgdb/doctrine`, entities `doc/protocol`+`nsgdb/usage`. Not called from
  `open` (tests stay clean). MCP calls it after embed.
- **MCP surface 4 tools**: `remember` / `recall` / `health` / `curate`.
  Dispatch by args (`user+response`, `entities`, `at`, `rag`, `view=era|validate`,
  `curate.op`). The previous 23 names remain valid aliases on `tools/call`.

### Fixed
- **Windows Cursor launchers**: PowerShell 5.1 rejects `>&2` (parse error → MCP
  never starts) and treats native `cargo` stderr as terminating under
  `$ErrorActionPreference = Stop`. `scripts/mcp-server.ps1` / `mcp-install.ps1`
  now use `[Console]::Error.WriteLine` and `Continue` around cargo.

### Changed
- **ADR-0008 Accepted** (decision record). MCP default flip shipped in **1.1.9**.

## [1.1.7] — 2026-08-20 (agent ergonomics — core + MCP)

### Added (core)
- **`ScopeDistribution`**, **`RememberOptions`**, **`RememberOutcome`**, **`Sgdb::remember_semantic_with`**
  — write L4+L2 + scope/entities/content_type numa operação; retorno estruturado p/ SDK/agentes.
- **`Sgdb::scope_distribution`**, **`Sgdb::recall_empty_hint`** — diagnóstico multi-tenant quando recall
  vazio (null-scoping mem0).
- **`Sgdb::set_default_scope` / `resolve_scope_param`** — escopo default por instância (host/MCP).
- **`HealthReport`**: `global_memory_count`, `scoped_memory_count`, `scope_labels`, `indexed_embedding_dims`.
- **`embedder::DEMO_EMBED_DIM` / `DEMO_EMBED_NOTE`** — contrato explícito do demo trigram (256-dim, não semântico).
- **`build.rs`** — `NEURAL_SGDB_BUILD_GIT` para diagnóstico runtime.

### Added (MCP / ops)
- Install fixo: `scripts/mcp-install.{ps1,sh}` → `.nsgdb/bin/mcp_server`; launchers com auto-build.
- `scripts/mcp-smoke.ps1`, CI job `mcp-windows`, `docs/MCP-RELOAD.md`.
- MCP: `structuredContent` em health/remember/recall; hints de escopo; `NEURAL_SGDB_DEFAULT_SCOPE`.

## [1.1.6] — 2026-08-20 (hits TIPADOS + ergonomia MCP)

O retorno de recall é lido por OUTRA inteligência (não por humano) — e o canal
carrega dados que NÃO são palavras humanas (JSON de intenção máquina→máquina,
embeddings da era, código, binários). Antes, tudo passava por
`String::from_utf8_lossy` e o consumidor não sabia nem QUE datum era nem COMO
parseá-lo. Cada item com regressão; matrix **229+1 / 181+1 / 275+1**, gates
verdes, hot test 90/0 exit 0.

### Added
- **`src/ctype.rs` (no_std-safe)**: `ContentType` = Text | Json | Code |
  Embedding(dim) | Binary — HINT derivado na LEITURA (nunca persistido; o
  writer pode declarar via seam, mesmo contrato de `entities`/`Embedder`);
  `RecallPath` = Semantic | Lexical | Entities (o campo `dist` tem escala
  DIFERENTE por path — cosseno 0..1 vs BM25 normalizado; em `hybrid` o
  consumidor precisa saber qual é qual); `detect_content_type` (JSON =
  `{…}`/`[…]` delimitado; código = keyword + um segundo sinal estrutural;
  não-UTF8 → Binary), `embedding_dim_of` (len%4==0 e ≥4 — mesma regra do
  `index_doc`/S1), `stable_label`/`parse_stable_label` (rótulos estáveis do
  seam de write), `renders_prose` (Embedding/Binary NUNCA viram prosa).
- **`Hit` (v1.1.6)**: + `path`, `content_type`, `score` (bruto, ranking
  auditável), `matched_terms` (grounding BM25), `validity: Option<(u64,u64)>`
  (janela bi-temporal por hit), `rel: Option<String>` (companion `/L2/` → key
  do primário `/L4|L5|L3/`), `payload_type` (datum REAL do primário). `Hit`
  NÃO é tipo wire (MDR1 é) → sem quebra de formato. `recall_weighted`/
  `recall_temporal` CARREGAM o Hit (campos novos fluem de graça).
- **Projeção prosa só para Text/Json/Code**: Embedding/Binary → `text` vazio;
  o consumidor vê `type=Embedding(4)` e sabe que o datum é o payload binário
  (era ADR-0007), nunca `from_utf8_lossy`.
- **Seam de WRITE — `set_content_type` (item 2)**: `MemoryMeta.content_type:
  Option<String>` = **MDM1 v6** (migração explícita: v1–v5 decodificam com
  `None`; NMD1/TKLV intocados). Quem fornece declara o rótulo ESTÁVEL
  (`text`/`json`/`code`/`embedding`/`binary`); rótulo inválido →
  `SgdbError::Invalid` na escrita. A declaração PROPAGA para o companion
  `/L2/` do primário (L4/L5); **declared wins** nos 3 sites de construção do
  Hit (`resolve_content_type`): Embedding declarado absorve a dim do payload;
  Embedding/Binary declarado NUNCA rende prosa. `content_type_of(key)` lê a
  declaração; `meta_for_import` carrega na replicação (o tipo viaja no MDR1).
- **`rag_context_reranked` (item 4 — lição P1/P5)**: o gargalo é ESCOLHER o
  que entra no prompt. Pool AMPLIADO (`recall_oversampled` oversample 8,
  top-4k) + rerank por ancoragem lexical (tokens do query no texto do hit) →
  score desc → dist asc. Linha expõe `anchors=N`; mesmo teto de bytes.
- **`examples/two_ai_protocol.rs` (item 5)**: contrato COMPLETO máquina→
  máquina, ponta a ponta (16 auto-checks): IA-A grava JSON de intenção, JSON
  NÚMERO `"42"` (o detector diria Text — o seam remove a adivinhação), binário
  não-UTF8 e vetor da era — todos DECLARADOS; IA-B consome os hits TIPADOS
  (`content_type` declarado vence, `payload_type`, `rel`, `matched_terms`),
  parseia JSON verbatim, segue `rel=` e re-lê o payload do primário, consome o
  binário cru pela key (nunca `from_utf8_lossy`). Determinístico (InMemory,
  sem LLM, embedding LCG do exemplo). Exit 0 sse 16/16.
- **MCP ergonomics (instalação Cursor/Windows)**: [`docs/MCP.md`](docs/MCP.md)
  (guia completo), [`.cursor/mcp.json`](.cursor/mcp.json) +
  [`scripts/mcp-server.ps1`](scripts/mcp-server.ps1) /
  [`scripts/mcp-server.sh`](scripts/mcp-server.sh) (launchers stdio via binário
  release — não dependem de `cargo` no PATH do IDE), [`scripts/mcp-smoke.sh`](
  scripts/mcp-smoke.sh) (23 tools + `era_report` + schemas + `health`
  onboarding), step de smoke no CI, [`CONTRIBUTING.md`](CONTRIBUTING.md)
  (identidade Git + gates locais).
- **MCP `health` onboarding**: JSON com `db_path`, `embedder`, dims indexadas,
  `mcp_tool_count`, `mcp_contract_version`, passos iniciais e link para
  embedder HTTP (`examples/embedder_http.rs`).
- **`mcp_actionable_error`**: erros de dim/era apontam `era_report`; erros de
  embedding citam contrato same-model-on-write-and-query.

### Changed
- **MCP `remember(type=)`** (item 2): declara o tipo no write (schema enum).
- **MCP `format=json`** (item 1): recall/recall_temporal/recall_entities/
  rag_context devolvem hits ESTRUTURADOS
  (`[{key,text,dist,score,path,type,dim,matched_terms,validity,rel,
  provenance}]`) com strings ESTÁVEIS (`path= semantic|lexical|entities`,
  `type= text|json|code|embedding(bin)`). Default continua a prosa
  (invariantes de paginação preservadas).
- **MCP `rag_context`**: + `rerank=true` (item 4) e + `mode`
  (semantic/lexical/hybrid) devolvendo hits tipados no lexical/hybrid.
- **MCP `fmt_hit`** unificado p/ recall (todos os modes) + temporal +
  entities; `payload=..` só aparece quando difere de `type` (datum real ≠
  projeção).
- **`Cargo.toml` → 1.1.6**; `serverInfo.version` → 1.1.6; README checklist
  pós-clone; docs de versão alinhados (`VERSIONING.md`, `docs/api.md`).

### Fixed
- **BUGFIX companion L5 (bughunt #13)**: o lookup do texto companion fazia
  `sk.replacen("/L4/","/L2/",1)` — no-op para keys `md/L5/` → o batch lia o
  PRÓPRIO doc L5 e projetava floats como prosa lossy. Agora `companion_key()`
  mapeia L4 E L5 → `md/L2/<id>`; sem companion, o tipo cai para o payload e o
  texto fica vazio (batch com texto vazio NÃO sobrescreve o tipo).
- **`LexicalIndex::search`** agora retorna `(key, score, matched_terms)` —
  termos da query que casaram, deduplicados por doc.

## [Unreleased] — v1.1.5 (era guard + era_report)

The write side of ADR-0007 hardened (guinea-pig path: the agent hits S1 on
query; now the WRITE is loud too and the DB tells the managing LLM exactly
what to do + how much it costs).

### Added
- **Write-side era guard (ADR-0007)**: `remember_semantic` rejects an
  embedding whose dimension is outside `indexed_dims` on a LIVE corpus with
  `SgdbError::Invalid` (message cites the era + `era_report()`; NOTHING is
  written — the BQ width-lock would truncate the new vector silently,
  bughunt #11). The first write of an EMPTY DB defines the era (a fresh DB
  per era remains the free path). Deliberate migration uses the raw path
  (`db.put(MemoryDoc)` + `quantize_f32` + `rebuild_indices()`) — the guarded
  API deliberately does NOT serve it.
- **`Sgdb::era_report()`/`era_report_lines()`** (`src/era.rs`, no_std-safe):
  read-only diagnostic that reports the corpus era state — indexed dims,
  per-dim doc counts (L4+L5 embedding-declared, same rule as `index_doc`),
  BQ width lock, `/L2/` companion coverage + text bytes (migration
  viability), verdict `empty`/`ok`/`mixed_dims`, the ADR plan, and the
  ESTIMATED db-side migration cost (`estimate_era_migration`: the measured
  formula from BENCHMARKS.md §Era applied to the real totals, ~86 µs/doc).
  The MODEL-side cost is external: the report hands over
  `docs_to_reembed`×`text_bytes` for the LLM to multiply by its own model's
  throughput/price. The core does not decide — it reports.
- **MCP `era_report` tool**: read-only (tool 23); the manager LLM calls it
  after an S1/write-guard error to decide migrate/keep/new-base.

### Changed
- S1 query error message now points at `era_report()`.

## [Unreleased] — v1.1.4 (memory landscape, item 1–10)

Ideas ported from the memory-landscape benchmark (mem0/mempalace/Zep/Letta/
Supermemory/cognee — see `docs/memory-landscape.md`).

### Changed
- **ADD-only is now an official contract** (item 1, from mem0): the BQ is
  append-only — new facts accumulate and confront the existing pool via
  deterministic ranking, never by silent overwrite. Conflict resolution is a
  retrieval-time concern (`recall_weighted`/`supersede`/`resolve_conflict`),
  not a write-time mutation. This was the behavior all along; it is now
  documented so callers can rely on it (see AGENTS.md §ADD-only).

### Added
- **`Sgdb::set_scope`/`scope_of` + recall scoping** (item 7, from mem0
  multi-tenancy): `MemoryMeta.scope: String` (MDM1 v4 — migration is
  explicit; v1/v2/v3 records decode with `scope = ""`). The scope filter runs
  INSIDE the candidate pool of `recall_impl`: a scoped memory never competes
  for top-k slots of another scope, and the unscoped global recall does not
  leak scoped memories (mem0's implicit null-scoping). New surface:
  `recall_scoped`/`recall_scoped_historical` + `Sgdb::set_scope`/`scope_of`.
  MCP `remember` gained `scope=`, `recall` gained `scope=`.
- **Retrieval modes** (item 8, from cognee `search_type`): `recall` in MCP
  now accepts `mode` = `semantic` (default, BQ+FP32), `lexical` (BM25 over
  L2/L3 texts, no embedding required) or `hybrid` (semantic first, then
  non-duplicated lexical). New core surface:
  `recall_lexical_scoped(_historical)`/`recall_hybrid_scoped`; the lexical
  and hybrid paths now honor the same scope filter as `recall` (they leaked
  scoped memories before). Companion `/L2/` docs resolve their effective
  scope from the primary `/L4/`/`/L5/`/`/L3/` doc (`Engine::effective_scope`).
- **Temporal-intent retrieval** (item 9, from mem0/Graphiti bi-temporal):
  `Sgdb::recall_temporal(query, k, at, w_sem, w_time)` re-ranks the
  semantic pool by proximity to the intent instant `at` — memories VALID at
  `at` (validity window covers it) rank first, memories that did not apply
  drop out, windowless memories fall back to recency relative to `at`.
  Answers "quando mudou X?" / "qual era o estado em T?". Scoped variant:
  `recall_temporal_scoped`. MCP tool `recall_temporal` (with `at`).
- **1-hop entity recall** (item 10, low-cost/low-risk port): named entities
  as an explicit, caller-supplied metadata seam — `MemoryMeta.entities:
  Vec<String>` (MDM1 v5 — migration is explicit; v1–v4 records decode with an
  empty list). The core NEVER extracts entities from text (same contract as
  `Embedder`: whoever supplies them uses the SAME strings on write and query).
  Derived `entity_index` (entity → storage keys, rebuilt from `sys/meta/` on
  open, maintained by `persist_meta`/`write_meta`, cleaned on `delete`).
  New surface: `Sgdb::set_entities`/`entities_of`/`recall_entities`
  (`_historical`/`_scoped`/`_scoped_historical`), ranked by overlap desc →
  importance desc → key asc. MCP `remember` gained `entities=`, new tool
  `recall_entities` (with `scope`/`historical`).
- **Agent decision protocol v2 (P1–P6)** (`examples/agent_protocol.rs`, now 23
  self-checks): the research-backed discipline for USING memory (SmartSearch
  2603.15599, MemoryArena 2602.16313, "Memory for Autonomous LLM Agents"
  2603.07670, FSFM 2604.20300). P1/P5 — `rerank_gate`: hybrid pool
  (semantic BQ ∪ lexical) re-ranked by lexical grounding BEFORE compiling the
  prompt (the "compilation bottleneck": recall 98.6% but only ~22% of gold
  evidence survives truncation without reranking); exact facts stay VERBATIM
  in L2 (`remember_episodic`) because the BQ only indexes L4/L5 — the lexical
  path recovers them (verbatim > abstraction, MemoryArena). P2 —
  `remember_fact_checked`: write-path filter that proves via the 1-hop entity
  index whether `subject predicate` already exists — identical object → DEDUP
  (no version bump, no maintenance churn), changed object → write (version
  bump, stable identity). P3 — `store_reflection` grounds every lesson in ≥1
  episodic evidence (`DerivedFrom` + `Supports`, auditable trail) and
  `recheck_for_contradiction` actively searches for evidence AGAINST the
  belief (`Contradicts` — anti self-reinforcing-error). P4 — `open_session`
  runs `expire_old` on open (FSFM/Ebbinghaus) and `recall_temporal` answers
  "qual era o estado em T?" (window covering `at` = penalty 0). P6 —
  `open_session` loads the scope's LATENT constraints via `recall_scoped`
  (the environment does not restate them — MemoryArena); global recall never
  leaks scoped memory.
- **`examples/memory_arena_eval.rs` (P7)**: a MemoryArena-style evaluation of
  memory UTILITY (not recall): a memory–agent–environment loop over
  interdependent multi-session subtasks measuring SR (binary success rate)
  and sPS (soft progress). Config A (naive hoarder: global, semantic-only,
  append with timestamps) vs Config B (protocol v2). Section 1 — static
  recall quiz: BOTH saturate 3/3 (memorization alone does not discriminate,
  the LoCoMo effect). Section 2 — three agentic tasks: B 3/3, A 0/3 (scoped
  shopping constraint, exact verbatim intermediate value, current state after
  lifecycle update). Deterministic (InMemory, no LLM); exit 0 iff the quiz
  ties AND SR(B) > SR(A) AND sPS(B) > sPS(A).
- **Model eras + era migration (ADR-0007)** (`examples/era_migration_bench.rs`):
  the embedding model is an **era invariant per corpus** — the S1 guard checks
  DIMENSIONS, not model identity, so same-dim model swaps are silently
  undetectable, and `BqFlatIndex` locks `words_per_vec` on the first insert
  (bughunt #11) — writing a different-dim embedding into a live BQ silently
  truncates it. The ADR codifies the switch policy (new DB file per era, or an
  explicit re-embed migration) and the benchmark measures the migration cost
  (N=2000, FileStorage): payload rewrite ~72 µs/doc, `rebuild_indices()` ~50 ms,
  scan/text-read negligible, all invariants asserted (width-lock trap
  reproduced, `memory_id` stable across overwrite, 40/40 resurrected recalls,
  era-OLD queries now fail loudly via S1, lexical still recovers old text,
  `validate()` clean).

### Fixed
- **MDM1 v4 decode**: the `last_reinforced` branch (v3) never advanced `off`
  after reading the u64 — harmless while it was the last field, but the v4
  `scope` field read from the wrong offset. Same discipline as the
  `metaflag=0` bug: every flag branch must advance `off` (regression:
  `meta_roundtrip` with non-empty scope).
- **MDM1 v4 decode (bis)**: the `scope` branch read `off..off+slen` but never
  advanced `off` past the scope bytes — harmless while v4 was the last field,
  but the v5 `entities` field read from the wrong offset. Same discipline:
  every field branch advances `off` (regression: `meta_roundtrip` with
  entities).
- **no_std gate**: `profile` used `std::cmp::Ordering` (pre-existing, item 5)
  — now `core::cmp::Ordering`.
- **Scope durability** (item 7 hardening): regression
  `scope_persists_across_reopen_and_rebuild` proves `MemoryMeta.scope` (MDM1
  v4, `sys/meta/`) survives disk reopen AND index rebuild — the filter never
  "forgets" tenants after a restart.
- **Agent decision protocol** (`examples/agent_protocol.rs`): the discipline
  of using the DB for decisions, encoded as reusable functions + 12
  self-checks. Ontology of entities (canonical `kind/name` strings, same on
  write and query), structured `Fact {subject, predicate, object}` +
  verbatim via `remember_episodic`, provenance-weighted evidence
  (`recall_weighted` with strong w_imp pulls the trustworthy memory ahead of
  the legacy one), memory lifecycle (`learn_fact` supersedes the old fact,
  positive feedback, periodic `expire_old`) and a two-pass protocol
  (`gather_evidence` collects without writing; pass 2 records the learning).
  Golden rule: a memory from another scope is NEVER evidence (filtered in
  every recall). Exit code 0 iff all checks pass.

## [Unreleased] — v1.1.3 (WIP)

Co-author ergonomics (S1–S5): the things that would annoy me as an agent
using this DB for real.

### Added
- **`Sgdb::indexed_embedding_dims()`** (S1): dimensionalities (nº of f32) of
  the embeddings currently indexed in the BQ — derived from L4/L5 payloads,
  rebuilt on remount.
- **`examples/embedder_http.rs`** (S2): the `Embedder` trait plugged into a
  real HTTP endpoint, end-to-end, with zero new deps (raw HTTP/1.1 via
  `std::net` + the existing `serde_json` dev-dep). Ships a self-contained mock
  embedding server (thread-local) plus an `HttpEmbedder` that POSTs text and
  parses the JSON response — the stand-in for BGE/OpenAI/ONNX behind an HTTP
  gateway. Proves the same-model contract (write+query through HTTP, 8 dims)
  and the S1 guard (a 256-dim demo query against an 8-dim corpus → `Invalid`).
  `cargo run --release --example embedder_http` → 4/4 PASS, exit 0.
- **Proactive BQ reclamation** (S4): `Sgdb::reclaim_bq_orphans(threshold)` +
  `BqFlatIndex::retain`. The BQ flat is append-only — physical `delete`
  left orphan ids in the index (harmless — recall skips them — but they
  inflated the candidate pool). `Sgdb::delete` now recompacts on the spot
  once orphans cross `DEFAULT_BQ_ORPHAN_THRESHOLD` (64) in `limits.rs`;
  `threshold = 0` always recompacts. Regression:
  `reclaim_bq_orphans_recompacts_after_delete`.

### Changed
- **`recall`/`recall_historical`/`recall_weighted` no longer return hamming
  noise on dimension mismatch** (S1): if the query dims match NONE of the
  indexed embeddings (e.g. 4-dim agent vector vs 256-dim demo), the recall
  returns `SgdbError::Invalid` with an actionable message instead of silently
  returning garbage — the P4 contract ("same model on write and query") is now
  enforced loudly. Text-payload L4/L5 docs (no bitvec) don't feed the
  detection (they're noise, not a dimension). Regressions:
  `recall_dim_mismatch_is_loud_not_silent`,
  `recall_dim_mismatch_survives_rebuild`.
- **`recall` companion texts load in batch** (S3): `Hit.text` was filled with
  one `get_by_storage_key` per hit — N×(doc NMD1 + `sys/meta/`) reads, even
  though the companion text needs no meta. New `Engine::get_texts_batch`
  reads all companion keys in one deduplicated pass (payload only, no
  `attach_meta`). Same contract, fewer reads. Regression:
  `recall_companion_texts_batch_parity`.
- **MCP `recall` pagination is lazy** (S5): the server fetched a hard-coded
  top-100 and paginated over it — fixed cost on every page plus an
  artificial 100-hit ceiling. It now computes only `off+size+1` hits for the
  requested page (the `+1` is a sentinel that makes a full page report a
  `nextCursor`). Deterministic top-k (score, key) means each lazy page slices
  the same ranking as the full fetch. Hot test phase 4c (60/60) exercises the
  real handler: page 1 = 2 hits + `nextCursor`, page 2 follows it with no
  repeats. Regressions: `lazy_recall_pages_match_full_topk` (server),
  hot-test phase 4c (client).
- Matrix: 197+1 / 243+1 / 151+1 tests (default / p2p / no-default).
- Hot test: 60/60 exit 0 (new phase 4c).

## [1.1.2] — 2026-08-14

Guinea-pig plan (v1.1 P1–P4): the fixes for what irritated me using it.

### Added
- **`Sgdb::associate_checked`** (P3): defensive variant of `associate` that
  validates BOTH sides exist (ghost key → `Err`, no orphan `sys/rel/`). The
  raw `associate` keeps the design (relation affirmed by the upper layer).
  Regression: `associate_checked_rejects_ghost_keys_no_orphan_relation`.
- **`neural_sgdb::Embedder` trait + `DemoEmbedder` + `demo_embed`**
  (P4, `src/embedder.rs`): `no_std`-safe, zero-dep seam for plugging a real
  embedding model (BGE/OpenAI/ONNX) without touching the core. The MCP server
  now accepts agent-supplied `embedding` in `remember`/`recall`/`rag_context`
  payloads (real-model path), falling back to the configured embedder
  (`NEURAL_SGDB_EMBEDDER`, default demo trigram). Hot test: +4 checks
  (embedding do agente grava/busca, contrato dimensional, caminho demo) —
  **49/49 exit 0**.

### Changed
- **`Sgdb::resolve_known_key`** (P1): raw keys now resolve to the existing
  canonical storage key by deterministic layer priority (L4 semantic first) —
  `meta`/`set_importance`/`set_confidence`/`reinforce`/`add_parents`/`forget`/
  `explain`/`transfer_to`/`merge_memories`/`supersede`/`get_state`/
  `set_state`/`memory_id`/`version_of`/`lineage`/`set_validity`/`validity_at`/
  `invalidate`/`delete`/`export_record` no longer silently miss with the right
  key and wrong form. `ensure_meta` errors now carry the canonical-key hint.
  Regression: `resolve_known_key_finds_layer_for_raw_key`.
- **`recall_weighted` now uses DOC importance** (P2): the score uses
  `Hit.provenance.importance` (from `set_importance`/`reinforce`) converted to
  the penalty space (`1 − imp`); records without meta fall back to the layer
  default. The name now tells the truth. Regression:
  `recall_weighted_uses_doc_importance_not_layer`.
- Matrix: 193+1 / 239+1 / 148+1 tests (default / p2p / no-default).

## [1.1.1] — 2026-08-14

### Added
- **`examples/audit.rs`** — the Cognitive QA audit, the mission test
  ("returns MEMORIES, not data"): 3 batteries, 59 assertions, exit 0.
  - Battery 1 (ATTACK, 23): hostile embeddings (NaN/Inf/empty), malformed
    keys (`/`, `#`, `sys/`, ART prefix-collision), invalid states, side-table
    overwrite, ghost-key relations.
  - Battery 2 (CORRUPTION, 11): deterministic bit-flip inside FileStorage
    L4 records → recovery truncates at the first invalid record; corrupted
    docs NEVER resurrect with mangled bytes; tail truncation + physical
    mid-file cut reopen cleanly; `rebuild_indices` reconciles.
  - Battery 3 (FIDELITY, 25): recall returns text + `HitProvenance` (not
    bytes); `forget` archives (history via `recall_historical`/`get`/
    `explain`, default recall ignores); `supersede` builds a DAG (lineage +
    `parent_ids`); validity window gates `recall_at`; `invalidate` = validity
    until `now` (not a state); `recall_weighted` ranks by LAYER importance
    (L4 penalty 0.0 vs L5 0.2).
- **Two more real bugs found & fixed by the audit**:
  1. `engine::set_state` accepted ghost keys for state ≠ Active, creating
     orphan `sys/state/` side-tables (validate flagged them) — same family
     as the hot-test MCP bughunt. Now rejects no-doc keys with `Invalid`
     BEFORE writing; `Active` stays remove-only (supersede marks new→Active
     before it exists). Regression: `set_state_rejects_ghost_key_no_orphan_
     side_table`.
  2. `validate()` BQ count only counted `md/L4/` but `put_inner` indexes
     L4 **and** L5 (bitvec or payload ≥4B reinterpreted as f32) — a legit
     L5 procedural embedding broke validate() with a false positive. Rule
     replicated exactly. Regression: `validate_accepts_l5_procedural_
     embedding`.

### Changed
- Matrix: 186+1 / 232+1 / 141+1 tests (default / p2p / no-default).

## [1.1.0] — 2026-08-13

### Added
- **`Sgdb::health()` / `Sgdb::validate()`** (P2-3): `ready()` (hardcoded
  `true`) replaced by observable `HealthReport` + aggregated integrity checks
  (see P2-3 entry below). New public types `HealthReport`, `ValidateIssue`.
- **MCP `health`/`validate` tools** (`examples/mcp_server.rs`).
- **`examples/signed_peer.rs`** — signed-transport reference flow.
- **`examples/mesh_simulation.rs`** — layered multi-AI telepathy (P2-5).
- **`examples/mcp_client.rs`** + **`docs/hot_test.md`** — the Hot SGDB Test:
  the agent as guinea pig driving the MCP server like an IDE (45 assertions,
  10 phases, cross-process persistence). Found and fixed two real bugs:
  (1) `remember` returned the raw `mcp/...` key that `resolve_storage_key`
  mapped to `md/mcp/...` (nonexistent) — side-table orphans were silently
  created (caught by `validate`!); now returns the full `md/L4/...` key and
  `recall` prints `h.key | text`. (2) `demo_embed` was position-dependent
  (seed mutated per n-gram window), so the same trigram at different offsets
  landed on different bins and keyword recall failed (`d≈1.0`); now
  position-independent.

### Changed
- `MemoryDelta`/`MemorySnapshot` now carry `Vec<MemoryRecord>` (v0.6
  breaking change already documented; kept here for release history).
- Matrix: 182+1 / 228+1 / 139+1 tests.

### P2 — Governance & security hardening (2026-08-13)
- **CRDT convergence in random topologies (P2-1)**: LCG-generated directed
  meshes (ring spine + random edges) converge to byte-identical content
  records; partitions/rejoins preserve concurrent writes with no lost
  versions. Documented semantics: `node_versions` is gossip knowledge (not
  converged state); `ConflictRecord` is LOCAL merge evidence (not a
  replicated unit) — content converges, authors keep their own values.
  API unchanged (tests only).
- **Governance docs (P2-2)**: new `SECURITY.md`, `VERSIONING.md`,
  `MIGRATIONS.md`, `ROADMAP.md` and `docs/adr/` (index + template + ADRs
  0001–0006) capturing the zero-deps/no_std contract, BQ-vs-FAISS, side-table
  metadata, byte-contract formats, ART prefix-key rejection and the
  no-crypto-in-core trust seam.
- **Health/validate + signed-transport reference (P2-3)**: `ready()` (hardcoded
  `true`, low value) replaced by `Sgdb::health()` (observable `HealthReport`:
  backend, node_id, storage probe, doc/BQ/RAM counts, open conflicts) and
  `Sgdb::validate()` (aggregated integrity checks: storage `md/` walk + NMD1
  decode + ART/BQ index cross-check + side-table orphan detection — returns
  every issue, empty = healthy). Both `no_std`-safe. Crypto stays OUT of the
  core: a reference signed-transport flow test (`trust.rs`, p2p) proves the
  `Signer`/`TrustStore`/`SignedEnvelope` seam end-to-end (sign → envelope →
  verify → reject tampered payload → reject untrusted peer) — the host plugs
  real Ed25519/HMAC at the boundary (ADR-0006).
- **Layered multi-AI telepathy simulation (P2-5)**: honest deterministic
  stubs (no LLM/real embeddings) demonstrate the memory substrate: 8 agents
  in 5 cognitive layers (L1 surface → L5 consolidation/identity) on a
  directed mesh; an "external AI" writes at L1, each layer answers via its
  own recall, telepathy (anti-entropy) propagates L1→L5, and a deep layer
  recovers by semantic recall the exact memory that entered at L1 (seed
  102 → "deploy quebrou a CI"), consolidates it, and re-converges to
  byte-identical content with fixed point. New p2p test
  `layered_ai_telepathy_mesh` + runnable `examples/mesh_simulation.rs`
  (`--features p2p`). API unchanged (tests + example only).
- **Central wire-codec fuzz harness (P2-4)**: `src/wire_fuzz.rs` closes the
  "fuzz-tested" promise from `docs/api.md` with ONE deterministic LCG harness
  (zero deps) over all 8 wire types — NMD1 (`MemoryDoc`), MDR1
  (`MemoryRecord`), MDM1 (`MemoryMeta`), CFL1 (`ConflictRecord`), MDLT
  (`MemoryDelta`), MSNP (`MemorySnapshot`), `SignedEnvelope`, `CrdtState`
  (last 4 p2p-gated). Properties per type: never-panic on random bytes
  (lengths 0..128 ×8 rounds), `decode∘encode` roundtrip (500 LCG samples),
  truncation safety on every prefix of a valid encoding, and
  corrupt-magic/version → `Err`, never panic. Post-audit regression pass:
  full matrix green (bughunt oracle #1–#11 re-verified).
- **MCP health/validate tools (P2-3 surface)**: `examples/mcp_server.rs` now
  exposes 2 read-only tools — `health` (observable `HealthReport`: backend,
  node_id, storage probe, doc/BQ/RAM counts, open conflicts) and `validate`
  (integrity issues aggregated from `Sgdb::validate`; empty = healthy) — so
  agents monitoring the DB get immediate observability over MCP.
- **Signed-transport example (ADR-0006)**: `examples/signed_peer.rs` (p2p)
  is the documentary "where to plug real crypto" runnable — proves the full
  reference flow end-to-end with the `HmacFnvSigner` demo: sign payload →
  `SignedEnvelope` → verify → reject tampered payload → reject untrusted peer
  via `TrustStore`, while `CrdtMemorySync` stays crypto-free. Production swap:
  implement `trust::Signer` with Ed25519/HMAC at the transport boundary.

### P0 — Hardening & tooling (delivered 2026-08-13)
- **Docs aligned to v1.0.0**: README, docs/api.md, CHANGELOG, MCP
  ServerInfo, tickv module docs, storage architecture — test counts, tool
  count (13), checkpoint/GC claims now match the code (P0-1/2/3).
- **Clippy zero-warnings** across all targets/features (P0-5): 13 files
  cleaned (needless loops, manual find, doc-list indentation, `?` blocks,
  `is_empty`, type aliases, struct-update Default, SIMD loops). New gate:
  `cargo clippy --all-targets --all-features -- -D warnings`.
- **CI gates** (P0-4): added `cargo test --no-default-features`, clippy
  `-D warnings`, and `cargo doc --no-deps` (RUSTDOCFLAGS `-D warnings`) to
  `.github/workflows/ci.yml`. `cargo fmt` intentionally NOT gated (repo not
  rustfmt-clean).
- **MCP paginate overflow** (P0-8): `paginate` clamps `pageSize` to
  `MAX_PAGE_SIZE=1000` and uses saturating offset arithmetic — hostile
  `pageSize` can no longer panic the server. Regression test added.
- **Arbitration empty-scores panic** (P0-9): `HeuristicArbitration` returns
  `Escalate` when a conflict has records but no candidates (empty `scores`)
  instead of panicking on `scores[0]`. Regression test added.
- **FileStorage recovery sweep test** (P0-7): open() recovery truncation
  verified deterministic for every cut offset (preserves complete records,
  truncates to `valid_end`, never panics).
- **SAFETY comments + differential SIMD test** (P0-10): all 9 unsafe sites
  documented (hamming_dispatch.rs ×7, art.rs ×2); new
  `differential_scalar_vs_all_kernels` proves scalar == AVX2 == AVX-512 over
  17 lengths + unequal-length pairs. `hamming_scalar`/BQ have no remaining
  clippy/lint issues.
- Matriz: default **157+1**, p2p **192+1**, no-default **114+1**,
  `x86_64-unknown-none` ok, clippy `-D warnings` ok, doc `-D warnings` ok.

## [1.0.0] — 2026-08-13

### v1.0 — Arbitration + trust seam + observability (Phase 16/28/32)
- **Arbitration layer** (`src/arbitration.rs`) — pluggable policy, NO LLM in
  core: `ArbitrationPolicy` trait (prefer/invalidate/merge/escalate by
  confidence/importance/recency) + `Arbitrator::arbitrate(db, conflict)`
  returning a structured `ArbitrationDecision` (winner, action, reasons).
  Deterministic, evidence-driven — the cognitive layer stays a consumer, the
  core never decides semantic truth.
- **Trust seam** (`src/trust.rs`, Phase 23/28) — `Peer` (node_id + identity +
  auth status + trust level + capabilities), bounded `TrustStore` (upsert,
  revoke, set_trust, trusted_peers), `Signer` trait with `HmacFnvSigner`
  DEMO (keyed FNV-1a, explicitly non-cryptographic — production hosts plug a
  real Ed25519/HMAC at the transport boundary; the core stays clean).
- **Observability** (`src/metrics.rs`, Phase 32) — structured counters wired
  into Sgdb: `memory_writes`, `recalls`, `lifecycle_transitions`,
  `conflicts_detected/resolved`, `replication_sent/received/rejected/stale/
  duplicate`, `clock_changes`, `storage_recoveries`, `index_rebuilds`;
  `db.metrics().snapshot()` for monitoring/diffing. `LifecycleReport` now
  carries `transitions`.
- Testes (+8 default, +4 no_std): `trust.rs` (bounded upsert, revoke,
  signer tamper/determinism), `metrics.rs` (snapshot), `sgdb.rs` (writes/
  recalls/lifecycle counted), `arbitration.rs` (4 policies under `p2p`).
  Matriz: default **155+1**, p2p **189+1**, no-default-features **113+1**,
  `x86_64-unknown-none` ok.

### v0.9 — Conflict model + reinforce + cognitive API + MCP surface (Phase 14/15/17/23)
- **First-class conflict model** (`src/conflict.rs`, `ConflictRecord`, `ConflictStatus`) — deterministic `conflict_id` (FNV-1a 128 sobre subject+candidates ordenados), re-merge upserta (nunca duplica). Persistido em `sys/conflict/<id>` com evidência completa: `records: Vec<Vec<u8>>` (MDR1 dos candidatos paralelos a `candidates`) — a resolução NÃO depende de re-buscar o nó remoto (item 14: conflict preservation).
- **`Sgdb::merge_remote` grava conflito** — branch CONCORRENTE: paraleliza (vid, MDR1) dos candidatos, ordena, deduplica; nós fonte únicos; upsert idempotente.
- **`resolve_conflict(cid, winner_vid)`** — decisão EXPLÍCITA da camada superior: importa o record do vencedor (via evidência), vira versão corrente do slot, perdedores viram `parent_ids`; conflito marcado `Resolved` (idempotente). Nenhuma decisão semântica no core (item 15/21).
- **`conflicts()` / `conflict(id)` / `dismiss_conflict(id)`** — enumeração e limpeza explícita.
- **`reinforce(key, delta)`** — `importance += delta` (clamp [0,1]), `last_reinforced = own_counter`. MDM1 v3 (decode retrocompatível v1/v2). Não ticka relógio — metadado cognitivo local.
- **`forget(key)`** — ARQUIVA (preserva história; recall default ignora).
- **`explain(key)` → `MemoryExplanation`** — machine-readable: state/layer/importance/confidence/source/version_id/parents/validity/children/last_reinforced (roadmap §17).
- **`transfer_to(key, target_layer)`** — move camada com linhagem: parent_ids + relation `derived_from`; fonte `Archived` (nada deletado).
- **`merge_memories(a, b, target)`** — C nasce com `parent_ids=[A,B]`, payload concatenado, importance/confidence = max. Fontes intactas (roadmap §16).
- **`engine.add_parents` / `engine.scan_versions` / `engine.own_counter`** — helpers internos.
- **`engine` conflict side-tables** — `put_conflict`, `get_conflict`, `list_conflicts`, `delete_conflict` (sys/conflict/).
- **MCP server** (`examples/mcp_server.rs`) expõe 11 tools cognitivas: `explain`, `reinforce`, `forget`, `associate`, `related_to`, `contradicts`, `supersede`, `conflicts`, `resolve_conflict`, `merge_memories` + `recall` com proveniência por hit (state/imp/conf/src). ServerInfo v0.9.0.
- **Bug fix**: MDM1 decode — `off += vid_len` ausente no branch `ver >= 2` (latente desde v0.7; o field v3 `last_reinforced` expôs).
- Testes (+10 default, +3 p2p, +2 no_std): `conflict.rs` (roundtrip open/resolved, determinismo de id, fuzz decode), `sgdb.rs` (reinforce clamping, persistência across reopen, concurrent merge cria conflito, resolve importa vencedor + preserva loser + idempotente, dismiss remove registro, merge_memories com parents, forget arqueia, explain expõe lineage, transfer_to move layer, `v0.9` tests). Matriz: default **147+1**, p2p **177+1**, no-default-features **105+1**, `x86_64-unknown-none` ok.

### v0.8 — L6 associations + provenance-aware recall + lifecycle engine (Phase 8/9/15/16)
- **L6 associative memory** (`RelationKind` + `associate`/`related_to`/
  `causes`/`supports`/`contradicts`/`derived_from`) — topologia cognitiva
  memória-NATIVA: side-table `sys/rel/<kind>/<a>#<b>` (storage = fonte da
  verdade) + índice ART forward/reverse (derivado, reconstruído no rebuild,
  removido no delete — memória morta não mantém topologia). Relações
  sobrevivem a reopen; `#` é separador reservado (rejeitado na entrada).
  NENHUMA inferência: a camada superior afirma, o SGDB armazena.
- **Provenance-aware recall (P0-9b)** — o recall default agora devolve só
  memórias **ATIVAS**: `Superseded`/`Archived`/`Decayed`/`Invalidated` são
  filtradas ANTES do ranking (não consomem vagas do top-k) — memória
  superseded nunca se finge de ativa (item 13). `recall_historical` e
  `recall_lexical_historical` incluem as inativas com `provenance.state`
  exposto (histórico explícito). `recall_at` continua compondo validade.
- **`MemoryLifecycle` determinístico** (`src/lifecycle.rs`, P0-8) —
  `tick(db, now)` sem relógio de parede oculto nem thread; `LifecycleConfig`
  + `LifecycleReport` estruturado (observabilidade). Transições: L1→L2
  commit (origem Archived), L2→L3 promoção por importância+idade, L3→L4
  semanticização heurística (L4 nasce SEM bitvec — embeddings são da camada
  superior; o core nunca gera representação semântica), L4→L5 NUNCA
  automática (HITL), decay configurável (Decayed, nunca delete) e archive
  de superseded envelhecido. Toda promoção registra `parent_ids` + relação
  L6 `derived_from` (DAG + topologia). Idempotente: fonte só promove se
  `Active`.
- **`Sgdb::add_parents`** — anexa `parent_ids` à meta (linhagem; base da
  fusão do v0.9).
- Testes (+10 lib, +5 no_std): relações (direções, determinismo, reopen,
  delete limpa topologia, derived_from, chave `#` rejeitada), recall
  active/histórico (semântico + lexical), lifecycle (commit idempotente,
  promoção por idade, semanticização com linhagem, decay sem delete,
  archive, determinismo, contador explícito). Matriz: default **136+1**,
  p2p **163+1**, no-default-features **95+1**, `x86_64-unknown-none` ok.

### v0.7 — Causal DAG + anti-entropy (Phase 3/6)
- **Anti-entropy de verdade (P0-7, Phase 6)** — o mesh não faz mais
  diff/pull doc-a-doc: cada ronda **anuncia o clock completo** (próprio +
  relayado — `CrdtMemorySync::announce`) e cada nó puxa SÓ a **faixa causal
  faltante** do peer (`known+1..=v` por nó), localizada pelo novo índice
  `(node, counter) → storage keys` (`AiosDatabaseEngine::clock_index`,
  `keys_for_clock`; derivado, reconstruído no rebuild, removido no delete).
  Versões e docs atravessam nós intermediários (gossip/relay); entrega
  duplicada/atrasada/fora-de-ordem é idempotente.
- **Estado de replicação DURÁVEL (P0-11)** — `CrdtState` (node_id +
  contadores + versões conhecidas, wire "CRDT" bounds-checked) com
  `state()`/`restore()`; restore recusa identidade alheia (nunca adota
  node_id de outro nó); persistível via escape hatch `Sgdb::read_side_bytes`/
  `write_side_bytes` (`sys/…`). Um nó reiniciado não regride o relógio nem
  re-anuncia versões antigas como novas.
- **Um write lógico = uma versão causal** — `remember_semantic` grava o
  companion de texto L2 sob o MESMO contador do L4 (`put_companion`): antes,
  cada put tickava o relógio e o contador do doc divergia da versão do CRDT
  (o pull direcionado perdia docs). `CrdtMemorySync::node_id()`/`known_clock`
  públicos.
- Testes (+5 p2p, +4 default/+5 no_std pelas novas APIs no core):
  `crdt_state_roundtrip_and_restore` (truncamento em todo ponto, restore
  recusa node_id alheio), `restart_preserves_clock_no_regression`,
  `versions_relay_through_intermediate_node` (gossip A→C→B sem aresta
  A→B), `directed_pull_fetches_full_version_range` (peer entra depois de 3
  escritas e puxa 1..=v). Matriz: default **126+1**, p2p **153+1**,
  no-default-features **86+1**, `x86_64-unknown-none` ok.

### v0.7 — Causal DAG (per-version identity) (Phase 3)
- **Identidade POR VERSÃO** (`MemoryMeta.version_id`, wire **MDM1 v2** com
  decode retrocompatível de v1 — migração explícita: version_id = memory_id):
  `memory_id` continua sendo a identidade estável do SLOT (layer,key);
  `version_id` identifica a VERSÃO corrente do DAG causal. Cada put local que
  muda o slot avança `version_id` e registra a versão anterior em
  `parent_ids` (linhagem causal).
- **Índice reverso `sys/version/<version_id>`** → (storage key + meta DA
  PRÓPRIA versão) — base de lineage consultável; derivado (escrito no
  persist_meta/ensure_meta, reconstruído no rebuild, removido no delete).
- **`Sgdb::version_of(key)`** e **`Sgdb::lineage(key) -> Vec<LineageEntry>`**
  — caminha o DAG para trás (parent mais recente, guarda de ciclos);
  `LineageEntry` expõe version_id/memory_id/storage_key/source/created_tick/
  parents (ramos de merge exploráveis pelo caller). `supersede` agora linka a
  VERSÃO corrente (não só o slot); `HitProvenance` ganhou `version_id`.
- Testes (+6 lib): overwrite cria versão nova com slot estável, lineage em
  mesma chave (multi-version), lineage cruzando chaves via supersede,
  version_id viaja na replicação e muda em overwrite local do receptor,
  persistência cross-reopen (índice reconstruído), MDM1 v1→v2 migration +
  fuzz truncado. Matriz: default **126+1**, p2p **149+1**, no-default-
  features **86+1**, `x86_64-unknown-none` ok.

### v0.6 — Memory identity + provenance + dynamic VectorClock + delta replication + layer-aware merge policy (Phase 1–5)
- **`MemoryRecord`** (`memory_doc.rs`) — memória como UNIDADE de replicação:
  doc NMD1 + estado lógico + janela de validade + meta, serializados juntos
  (wire "MDR1", bounds-checked, nunca panics). Fecha a contradição #2: o
  antigo diff/pull doc-a-doc descartava `sys/state/`/`sys/validity/` — agora
  o side-metadata viaja com o doc.
- **`AiosDatabaseEngine::export_record/import_record`** (`engine.rs`) —
  exporta/importa o record completo. Import NÃO ticka o relógio local (o
  receptor nunca vira "escritor" de memória alheia — sem inflação causal) e
  deriva identidade determinística do AUTOR do relógio para registros
  pré-v0.6 (nunca reivindica autoria local).
- **`Sgdb::export_record/import_record`** públicos + **`Sgdb::merge_remote`**
  (p2p) — merge de record remoto sob a política da camada: Applied/Stale/
  Duplicate/Conflict/Rejected; concorrentes NUNCA sobrescritas (conflito
  exposto, camada superior decide).
- **`MergePolicy`** (`crdt.rs`) — tabela explícita camada → política
  (L0 LocalOnly, L1 LocalWorking, L2/L3 MultiValueRegister, L4
  CausalLwwWithHistory, L5/L7 ControlledLww, L6 Reserved) + veredicto
  `Rejected`. Consultada em `apply_remote_version_with_policy` e no
  `merge_remote` — a regra LWW universal morreu (item 8).
- **`MemoryDelta`/`MemorySnapshot` reais** (`crdt.rs`) — agora carregam
  `records: Vec<MemoryRecord>` (não só NMD1) com codecs "MDLT"/"MSNP"
  bounds-checked; **`CrdtMemorySync::missing_after(peer)`** computa a faixa
  causal faltante (o que pedir num protocolo de delta). A substituição dos
  stubs `docs: Vec<Vec<u8>>` é quebra de API documentada (tipos marcados
  como "futuro/NÃO implementado" desde v0.3).
- **Hardening de versões** (`crdt.rs`): versão 0 (nó sem escritas) é
  ignorada — antes, um relay que só publica heartbeat virava "concorrente"
  de todos (conflito fantasma); `local_version` NUNCA adota versão de peer
  (adotar fazia um nó fresh re-broadcastar versão alheia como autoria).
- **Harness de 3 nós + partition/rejoin** (`crdt.rs` tests) — malha com
  arestas, duplicatas, atraso e partições: convergência em triângulo,
  A∥B com C-relay → reconexão preserva AMBAS as escritas concorrentes,
  entrega duplicada/atrasada idempotente, nó novo (restart) alcança tudo,
  sync repetido é ponto-fixo. Testes de propriedade: merge associativo,
  `missing_after`, política exaustiva, codecs malformados (fuzz LCG).
- **`examples/p2p_telepathy.rs`** — pull agora via `export_record` +
  `merge_remote`; cenário novo: A supersede + marca validade, B vê
  estado/validade/lineage replicados.
- Matriz: default **120+1**, p2p **143+1**, no-default-features **81+1**,
  `x86_64-unknown-none` ok.
- **`VectorClock` dinâmico** (`memory_doc.rs`) — fast path de 8 nós em arrays
  fixos + `overflow` para nós além do 8º; `set_counter` = registro dinâmico
  de nós (política bounded: máx. 248 no overflow); `happens_before`/
  `concurrent`/`merge`/igualdade consideram fixos + overflow. O NMD1
  continua 72B fixos byte-idênticos ao OS (o overflow persiste em
  `sys/meta/` e é re-fundido no `get`). Testes: >8 nós, concorrência via
  overflow, merge monotônico/comutativo/idempotente, política bounded.
- **`MemoryMeta`** (`memory_doc.rs`) — identidade + proveniência compactas
  (wire "MDM1", bounds-checked, nunca panics): `memory_id` (32 hex),
  `source`, `confidence` [0..1], `importance` [0..1], `created_tick`,
  `parent_ids` (DAG causal), `clock_overflow`. Persistido em side-table
  `sys/meta/` — o NMD1 NÃO muda (decisão de formato documentada em
  `docs/api.md`).
- **`generate_memory_id`** — FNV-1a 128 bits sobre (node_id, created_tick,
  layer, key), determinístico, independente de node_id, nunca re-derivado.
  Watermark do contador próprio reconstruído no recovery (docs 72B + metas)
  garante ids monotônicos através de restarts — re-criação pós-delete não
  colide.
- **Identidade estável** (`engine.rs`) — overwrite = mesma memória: a meta
  existente vence (memory_id/source/created nunca mudam); doc replicado com
  `meta` preserva a identidade do criador; `Sgdb::put(doc)` escreve a meta
  junto (base do fechamento do gap de replicação).
- **Provenance no recall** (`sgdb.rs`) — `Hit.provenance`
  (`HitProvenance`: memory_id, layer, state, source, confidence, importance,
  created_tick, parent_ids) em `recall*` e `recall_lexical`; memórias
  superseded não se fingem de ativas (Phase 9 parcial).
- **API pública nova**: `Sgdb::memory_id/meta/set_importance/set_confidence`;
  `MemoryState::Decayed` (o estado existe; o motor de decay é fase posterior);
  `SgdbError::Invalid` (contrato: não-finita é rejeitada, fora de 0..1 é
  clampada). `supersede` agora registra `new.parent_ids += [old.memory_id]`.
- **Migração pré-v0.6**: registros antigos retornam `meta: None` até o
  próximo put/`set_importance` (que atribui identidade determinística) —
  nunca reinterpreta bytes antigos.
- Testes (+18 lib): identidade estável/colisão, meta viaja na replicação,
  contrato importance/confidence, parents no supersede, proveniência no
  recall, persistência cross-reopen (identidade + overflow do clock),
  Decayed roundtrip. Matriz: default **113+1**, p2p **125+1**,
  no-default-features **74+1**, `x86_64-unknown-none` ok.

### Maturation v0.2 (robustez, determinismo, durabilidade)
- **`Sgdb::delete`** (`sgdb.rs`/`engine.rs`) — deleção FÍSICA (tombstone +
  side-tables `sys/state|validity` + ART/lexical/id→sk), idempotente, distinta
  do estado lógico (invalidar-não-deletar). O BQ (índice flat append-only) fica
  com entradas inertes: o recall pula candidatos sem doc vivo — memória
  deletada nunca ressuscita. Testes: consistência de índices, persistência
  cross-reopen, re-add pós-delete.
- **`SignedEnvelope`** (`crdt.rs`) — envelope de transporte autenticável
  (payload + node_id + auth opaco), wire length-prefixed bounds-checked (nunca
  panics em input malformado). Fronteira de segurança explícita: o core não
  implementa crypto; `UdpTransport` segue DEMO não autenticado.
- **Recall hardening** (`sgdb.rs`) — candidatos cujo doc sumiu/corrompeu são
  pulados no `recall_oversampled` (antes caíam em fallback hamming). Estágios
  do pipeline documentados: candidate generation (BQ) → filtragem → rerank
  FP32 → finalização determinística.
- **Teste de estágios de retrieval** — caso onde hamming e cosseno FP32
  discordam (A hamming 7/cos 0.869 vs B hamming 0/cos 0.243): prova que o
  rerank reordena e que o resultado é determinístico.
- **`cargo test --no-default-features` agora passa** (58+1) — exemplos
  `bench`/`stress` ganharam `required-features = ["file-storage"]`;
  imports `alloc::vec`/`alloc::format` nos testes no_std; `unused_mut` e
  variáveis mortas limpos (deny(warnings) no no_std).
- Docs atualizadas: `docs/api.md` (API `delete` + fronteira de segurança
  CRDT), `AGENTS.md`/`README.md` (contagens de teste reais).

### Telepathy (p2p memory exchange demo)
- **`examples/p2p_telepathy.rs`** — two `Sgdb` instances exchange memories via
  CRDT version sync + diff pull (`Sgdb::get` → `Sgdb::put`): A and B converge
  with no central server. `required-features = ["p2p"]`. Demo shows concurrent
  writes being preserved (`CONFLITO` verdict), never LWW-discarded.
- **`Sgdb::put(doc)`** — public restore/import primitive (indexes any
  `MemoryDoc`), the replication hook for the pull side.

### Interface for use (study / AI-assisted IDEs)
- `MihIndex` and `LexicalIndex` now re-exported at the crate root; new
  read-only `Sgdb::bq()` accessor (feeds `MihIndex::build`).
- Crate doc is a runnable **quick tour** (doctest) covering remember/recall/
  weighted/lexical/validity/MIH — `cargo doc --open`.
- README refreshed to v0.5 (92 tests, recall variants, checkpoint/GC storage,
  MCP resources/pagination, honest bench numbers).
- `docs/api.md` "Target public API" extended with the v0.5 surface
  (`recall_oversampled`/`recall_weighted`/`recall_lexical`/`recall_hybrid`,
  validity window, `MihIndex`/`LexicalIndex`).
- Doctest exposed a real bug in `MihIndex::build` (blocks > vector width
  panicked on `vec[lo..hi]`) — fixed with a bounds guard + `break`.

## [0.5.0] — 2026-08-10

### Added (pesquisa de ponta 2026 — 10 itens, cada um com teste de medição)
- **#1 `MihIndex`** (`bq.rs`) — Multi-Index Hashing (Norouzi) sobre os bitvecs
  existentes: candidatos ∝ N/2^(bits/bloco) em vez de O(N), match exato sempre
  recuperado, ranking por hamming completo. Teste: pool < N/8 e top-1 = exato.
- **#2 ART range-scan pruning** (`art.rs`) — `scan_prefix` só desce em filhos
  que podem casar com o prefixo (poda por byte de borda + `path_matches`).
  `scan_prefix_stats` mede nós visitados: scan estreito visita < 1/3 da árvore.
- **#3 `recall_weighted`** (`sgdb.rs`) — `score = w_sem·dist + w_rec·recência
  + w_imp·importância` (padrão Mem0/MemGPT); recência do `/ts/<hex>` da key,
  importância da camada `md/LX/`. Teste: recente vence com w_rec alto; L4
  vence L5 com w_imp alto.
- **#4 `quantize_f32_centered` / `top_k_f32_centered`** (`bq.rs`) — query
  re-centrada pela própria média; bitvecs armazenados intactos. Teste: query
  com offset (+5) recupera o exato que o `sign(x)>0` perde.
- **#5 auto-oversample por dimensionalidade** (`sgdb.rs`) — `recall` usa
  ov=16 (1 word) / 8 (2-4) / 4 (≥5); BQ degrada abaixo de ~768 dims. Teste:
  recupera ~285/286 do exato em 16-dim vs o pool fixo antigo.
- **#6 invalidação in-place no `TickvFile::put`/`delete`** (`tickv.rs`) —
  `magic[3]` do record anterior vira 0 (`TKL\0`) antes do append (parity OS);
  dead-space detectável, GC-ready. Teste: bytes `TKL\0` no offset antigo.
- **#7 path lexical contextual** (`lexical.rs` + engine + `recall_lexical`/
  `recall_hybrid`) — índice invertido BM25-style (alloc-only, no_std) sobre
  textos L2/L3; recupera termos que o BQ perde (dual-path Anthropic). Teste:
  "ordenacao" acha só o doc certo; híbrido não duplica. Custo ~6µs/put.
- **#8 MCP resources + paginação + annotations** (`examples/mcp_server.rs`) —
  `resources/list`/`resources/read` (`memory://{layer}/{key}`), `nextCursor`
  opaco em recall/resources, `readOnlyHint`/`destructiveHint`/`idempotentHint`.
  Testes: parse de URI + paginação por cursor.
- **#9 janela de validade** (`engine`/`sgdb`) — side-table `sys/validity/`
  (`from ≤ now < until`), **invalidar-não-deletar** (Zep/Graphiti);
  `set_validity`/`invalidate`/`validity_at`/`recall_at`. Teste: persiste no
  reopen e `recall_at` filtra inválidos.
- **#10 delta CRDT** (`crdt.rs`) — `record_change` registra deltas; `sync`
  envia SÓ o não-visto pelo peer (`send_delta`, default cai p/ `send_crdt`);
  `pending_deltas()` mede. Teste: peer convergido até v2 recebe só v3.

## [0.4.0] — 2026-08-10

### Fixed (bughunt #11)
- **FileStorage oversized write = silent tail data loss**: `put` with a value
  > `MAX_VLEN` (or key > `MAX_KLEN`) was accepted, but `open()` recovery
  rejects it and **truncates the file** — every record written after it was
  silently destroyed. `append` now bounds-checks before writing (parity with
  `TickvFile`); oversized puts fail with `Err`.
- **`put(k, &[])` inconsistent with reopen** (both `FileStorage` and
  `TickvFile`): empty value writes a tombstone on disk (vlen `u32::MAX` /
  `0`) but kept `k → []` in the in-memory map — `get(k)` returned `Some([])`
  in-session and `None` after reopen. Empty value now behaves as delete at
  both read points.
- **`BqFlatIndex::insert_1024` broke the flat-index invariant**: it appended
  16 words unconditionally even when `words_per_vec` was already another
  value — `top_k` then read out-of-bounds (panic) or returned wrong results
  when mixed with narrower `insert_f32`/`insert`. It now truncates/pads to
  the established width (like `insert`).
- **`scan_volume` hid torn tails**: a partial header of 1..15 bytes at EOF
  was silently ignored (`truncated = false`), unlike `FileStorage` recovery.
  A clean pre-zeroed EOF region is still treated as clean EOF (not truncation).
- **`scan_volume` indexed the checkpoint record as memory**: `sys/tickv_ckpt`
  was surfaced as a live key (`vlen != 0`) in the backend map. Now skipped
  (parity with the OS `recover()`); exposed when `TickvFile` began writing
  checkpoints.
- **`encode_ckpt` count mismatch**: entries skipped in the body (key > 65535
  or `sys/tickv_ckpt`) still inflated the `n` field — an OS decoder reading
  `n` entries would desync. `n` now counts only the entries actually written
  (hash already covered only those).
- **`Hit.dist` scale contract**: the hamming fallback in `recall` returned a
  raw distance in `0..64` while `Hit.dist` documents `1−cos` on `0..1`; it is
  now normalized by `words_per_vec × 64`.

### Added
- **`Sgdb::recall_oversampled(query, k, oversample)`** (upstream BQ/Qdrant): the
  coarse Hamming filter now fetches `oversample·k` candidates before the FP32
  rescore. With low-dim embeddings the BQ filter collides on bits and the exact
  match escapes a small top-k (stress measured exact@1 ≈ 42% at 100k × 16-dim);
  raising the oversample recovers it **without any format change**. `recall()`
  delegates with oversample=4 (unchanged behavior); `rag_context_oversampled`
  added. Test: exact match recovers at 64× on low dims.
- **`TickvFile::checkpoint()` + fast-mount TKCK** (OS TickvLite parity, roadmap
  v0.1 gap): `checkpoint()` writes the `sys/tickv_ckpt` record (TKCK,
  byte-identical to the OS) as the LAST record; `open()` now tries
  `try_mount_from_ckpt` (header-only scan, FNV-1a index verification, per-entry
  CRC + `TKL V` stale check, ckpt-must-be-last guard) and falls back to the
  full `scan_volume` on any anomaly (torn/stale ckpt, post-ckpt appends).
  `ScanResult` gains `offsets` + `append_off`. Bench (churn, 35k recs / 5k
  live): fast-mount 14.8ms vs full-scan 43.2ms (**~2.9x**); in an all-live
  volume both read the same bytes so it's parity — the win is not re-processing
  tombstones/dead records. Torn ckpt degrades to the previous-mount semantics.
- **`TickvFile::compact()`** (GC, roadmap v0.2, OS `maybe_gc` parity): rewrites
  the live set as fresh TKLV records + a final TKCK checkpoint, atomic rename.
  Removes tombstones/obsolete versions and leaves the volume fast-mountable.
- **ART shrink on delete** (Leis paper / artful parity): `delete` now removes
  leaves and shrinks nodes 256→48→16→4 when `n` drops below threshold instead
  of leaving dead leaves — memory is reclaimed under churn. `delete_rec` now
  returns `Option<Box<Node>>` (None = empty subtree); the `dead` leaf tombstone
  is gone. Tests: 200-key→1-key shrink, 100k-op churn, re-insert after empty.
- **Fault-injection for fast-mount**: deterministic fuzz (in-memory) truncating
  at every offset + corrupting every byte of a valid TKCK volume → never
  panics and falls back to full scan; plus file-level torn/corrupt ckpt tests.
- **CI (GitHub Actions)**: test (default + `p2p`), `no_std` gate
  (`x86_64-unknown-none`), examples build, stress and bench smoke on every push.
- **Honest BQ benchmark**: bench recall@5 now uses correlated cluster data
  (not pure noise, which measured a meaningless 0%) and reports the oversample
  curve — 22% (1×) → 35% (16×) on dense 1024-dim clusters, documenting that
  sign-BQ separates the cluster but not the exact member (FP32 rescore then
  re-ranks the candidates).

### Performance
- **Hamming dispatch hot path**: `ensure_selected()` used a `SELECTED.swap(true)`
  (locked RMW) on **every** `hamming()` call — it dominated short-vector scans.
  Now `load`+`store` (benign double-select race, `set_cpu_caps` still rearms).
  Measured: BQ top-5 (10k vec × 1024 dim) 213µs → **160µs** (~25%).
- **CRC32 table (256)**: `const fn` table, zero-dep, no_std-safe, same bytes
  (golden tests pin). 1 op/byte instead of 8. Plus `crc32_parts` computes
  CRC over key‖val without concatenating (1 fewer allocation per record in
  FileStorage append/recovery/compact). 1MiB bench is serial/bandwidth-bound
  on this host (no change there); the win is per-record write/recovery.
- **FileStorage append with a persistent lazy handle**: `put`/`delete` no
  longer open+close the file on every write (one `CreateFile`+`CloseHandle`
  syscall pair per op). Measured (stress 100k, release): `Storage::put` raw
  185µs → **4.8µs** (~38x), `remember_semantic` 422µs → **22µs** (~19x),
  `remember_exchange` 201µs → 8.4µs (~24x). The handle opens on first append
  and is closed before `compact()`'s atomic rename (reopened lazily) — writes
  after compaction always target the new file. `open()` stays O(file) without
  extra syscalls (no regression on open/close stress).

### Planned (v0.3+)
- CRDT per-layer merge policy (roadmap): LWW for config/state, multi-value
  register for episodic, causal merge via VectorClock (`docs/api.md`)
- `TickvFile` GC/compaction + TKCK checkpoint in the backend
- L6 Associative/Metacognitive + Memory Graph (Doc 01/03)

## [0.3.0] — 2026-08-10

### Added (maturation sprint)
- **VectorClock semantics**: semantic `PartialEq` (map node→counter, order-
  independent), `happens_before` (causal), `concurrent` (excludes equality),
  `merge` (element-wise max + saturation), `counter_of`; 8 tests
- **CRDT conflict preservation**: `MergeVerdict` (SelfPacket/Stale/Duplicate/
  Applied/Conflict), `conflicts` (concurrent versions never LWW-discarded),
  `own_writes` (concurrency base — peer causal successor converges), self-packet
  ignored; `MemoryVersion`/`MemoryDelta`/`MemorySnapshot` abstractions; tests
- **Deterministic retrieval**: recall dedupes by storage key (best score) +
  tie-break by key — same DB+query+k ⇒ same ordered results; tests
- **Bounded BQ top-k**: max-heap (O(N·D/64 + N log k)) instead of full sort;
  k==0/k>=len/empty handled; deterministic (dist,id); bench heap(k=5)=320µs vs
  full-sort(k=N)=592µs; 6 tests
- **Durability semantics**: `Durability` (Buffered/Flushed/Durable),
  `Storage::durability()` + `sync_durable()` (fsync real no FileStorage,
  read+write handle p/ Windows); InMemory = Buffered; test
- **FileStorage compaction**: `compact()` — live set rewritten to temp +
  atomic rename; removes tombstones/obsolete; crash-safe; empty value = TOMBSTONE
  (aligned com append); 3 tests
- **Index rebuild público**: `Sgdb::rebuild_indices()` + `open_with_node_id`/
  `node_id()`; teste write→close→reopen→rebuild→recall
- **MemoryState model**: Active/Superseded/Archived/Invalidated — NÃO serializado
  no NMD1 (contrato byte-idêntico com o OS intacto); side-table `sys/state/`
  via Storage cru; `Sgdb::get_state/set_state/supersede`; `MemoryLayer::from_u8`
  = ponto único de validação; 2 testes
- **Adversarial tests**: fuzz determinístico LCG para MemoryDoc decode/view,
  TKLV scan_volume, CRDT apply_remote_version — nunca panics em malformed input

### Fixed
- **Baseline**: `cargo test --no-default-features` quebrava (30 erros) —
  imports alloc nos testes, gates `#[cfg(feature="file-storage")]`, exemplo
  mcp_server com backend por feature
- **FileStorage recovery**: bounds sanitizados (klen≤4KiB/vlen≤1MiB, checked_add),
  le32() sem unwrap, truncação determinística da cauda; CRÍTICO tombstone
  (vlen=u32::MAX) era tratado como length absurdo e chave deletada RESSUSCITAVA;
  HIGH tombstone truncado panicava (slice sem bounds); tombstone agora com CRC
- **CRDT**: has_other_state usava local_version (adotado de peers) — sucessor
  causal do mesmo peer virava Conflict para sempre; fix own_writes
- **Parsing safety**: rd_u32/rd_u64/le32 checados (sem `try_into().unwrap()`)
  em MemoryDoc decode/view, tickv scan, CRDT recv
- **compact**: valor vazio gravava vlen=0 vs append TOMBSTONE — mesma chave
  mudaria de significado pós-compactação
- **set_state(Active)**: deletava sys/state/ incondicionalmente (log crescia);
  só deleta se existir
- **recall**: overwrite em L4 re-inseria no BQ — mesma memória voltava 2x;
  dedupe por storage key + tie-break determinístico

### Changed
- Top-k BQ: full sort → bounded heap (resultado idêntico, mais rápido)
- `Sgdb::open` propaga erro de rebuild (P1); `recovered_records()` expõe contagem
- Bench honesto: baseline recall@5 agora é cosseno FP32 real (dados sintéticos
  pseudo-aleatórios → 0%, documentando o trade-off do sign-BQ em ruído)

## [0.2.0] — 2026-08-10

### Fixed
- **CRITICAL — in-place tombstone resurrection** (`scan_volume`): OS-written
  deletes (`TKL\0`, magic[3]=0) were re-inserted as live data; now skipped
  before CRC, matching OS `recover()` (bughunt #1).
- **FileStorage CRC** now covers key‖val, not just the key — value bit rot is
  detected on open (bughunt #2).
- **Recall sort** uses the raw u32 score (OS parity: FP32 0..10000 vs ham
  0..64); companion-text lookup uses `replacen("/L4/", "/L2/", 1)` (bughunt #3/#6).
- **clamp** truncates at a char boundary — no panic on mid multi-byte cut (bughunt #7).
- **`set_cpu_caps`** rearms the SIMD kernel selection latch — no_std injection
  after first use now takes effect (bughunt #9).
- **MCP `remember`** keys are ms×1000+seq (no collision in the same ms);
  `demo_embed` falls back to individual bytes for text < 3 chars (bughunt #10).
- **Bench recall@k** baseline is true FP32 cosine over the original f32
  vectors — the previous "FP32-exact" comparison was tautological (bughunt #4).
- **`Sgdb::open`** propagates rebuild errors — an unreadable storage no longer
  opens as a silent "ready" (P1); `recovered_records()` exposes the reindexed
  count.

### Changed
- `scan_volume` bounds checks run before the record-size check, matching the OS
  (a mid-volume corrupt header no longer stops the scan — bughunt #5).
- `Sgdb::open` is now strict about storage scan errors (best-effort rebuild is
  gone; recovery is observable).
- Features split: file backends behind `file-storage`, SIMD auto-detect behind
  `simd-runtime` (defaults unchanged).
- Documentation 100% English: README, `docs/api.md`, AGENTS.md, CLAUDE.md,
  codemaps, release notes; all PT-BR doc-comments translated.

## [0.1.0] — 2026-08-10

### Added
- **Portable core extraction** from neural-os-core (`k_ai::sgdb`, ADR-0063):
  `ArtIndex` (Radix Tree Node4/16/48/256 + SSE2), `MemoryDoc`/`MemoryDocView`
  (NMD1 format, byte-identical to the OS), `BqFlatIndex` (1-bit quantization +
  Hamming top-k), `hamming_dispatch` (scalar/AVX2/AVX-512, `#[target_feature]`),
  instance-based `AiosDatabaseEngine` (RAM L0/L1 + Storage L2–L7, ART/BQ
  indexing, rebuild), `Sgdb` facade (remember_exchange/_full, remember_semantic,
  recall, rag_context, remember_fact, scan_prefix, checkpoint, prune_working_ram,
  get, recovered_records).
- **`Storage` trait** (put/get/scan_prefix/delete) + `SgdbError` with three
  shipped backends: `InMemory`, `FileStorage` (CRC32 append-log, crash-safe),
  `TickvFile` (byte-exact TKLV records, OS-readable).
- **TKLV/TKCK codec** (`src/tickv.rs`): `encode_record`, `scan_volume` (OS
  `recover()` semantics: 512-aligned corrupt hunt, EOF all-0x00/0xFF, in-place
  `TKL\0` tombstone skip, last-wins), `encode_ckpt`, `fnv1a64` — byte-exact
  storage interop with neural-os-core.
- **CRDT memory sync** (feature `p2p`, opt-in): `CrdtMemorySync` (symmetric
  LWW), `Transport` trait, `UdpTransport` (std, unauthenticated demo).
- **Examples**: `bench` (ART P50/P99, BQ top-k, recall@5 BQ vs FP32 cosine,
  zero-dep) and `mcp_server` (MCP over stdio, handshake `2025-11-25`, tools
  remember/recall/rag_context, trigram demo embedding).
- **Features**: `std`, `file-storage`, `simd-runtime` (default), `p2p`
  (opt-in). Dual-mode `no_std` + `std`, zero runtime dependencies.
- **Docs**: README, `docs/api.md` (contract, seams, migration map, format
  versioning, feature matrix, CRDT policy), `AGENTS.md`, `CLAUDE.md`, codemaps,
  release notes — all English.

### Fixed
- **CRITICAL — in-place tombstone resurrection** (`scan_volume`): OS-written
  deletes (`TKL\0`, magic[3]=0) were re-inserted as live data; now skipped
  before CRC, matching OS `recover()` (bughunt #1).
- **FileStorage CRC** now covers key‖val, not just the key — value bit rot is
  detected on open (bughunt #2).
- **Recall sort** uses the raw u32 score (OS parity: FP32 0..10000 vs ham
  0..64); companion-text lookup uses `replacen("/L4/", "/L2/", 1)` (bughunt #3/#6).
- **clamp** truncates at a char boundary — no panic on mid multi-byte cut (bughunt #7).
- **`set_cpu_caps`** rearms the SIMD kernel selection latch — no_std injection
  after first use now takes effect (bughunt #9).
- **MCP `remember`** keys are ms×1000+seq (no collision in the same ms);
  `demo_embed` falls back to individual bytes for text < 3 chars (bughunt #10).
- **Bench recall@k** baseline is true FP32 cosine over the original f32
  vectors — the previous "FP32-exact" comparison was tautological (bughunt #4).
- **`Sgdb::open`** propagates rebuild errors — an unreadable storage no longer
  opens as a silent "ready" (P1); `recovered_records()` exposes the reindexed
  count.

### Changed
- `scan_volume` bounds checks run before the record-size check, matching the OS
  (a mid-volume corrupt header no longer stops the scan — bughunt #5).
- `Sgdb::open` is now strict about storage scan errors (best-effort rebuild is
  gone; recovery is observable).
- Features split: file backends behind `file-storage`, SIMD auto-detect behind
  `simd-runtime` (defaults unchanged).

## [0.0.0] — 2026-08-09

### Added
- Initial scaffold: dual license (MIT OR Apache-2.0), README, `.gitignore`,
  `docs/api.md` (design target).
