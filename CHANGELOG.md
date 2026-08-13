# Changelog — neural-sgdb

All notable changes to this project. Format based on
[Keep a Changelog](https://keepachangelog.com/), versions follow
[SemVer](https://semver.org/).

## [Unreleased]

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
