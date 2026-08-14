# AGENTS.md — neural-sgdb

Guide for AI agents (OpenCode, Cursor, Windsurf, Claude Code) working in this
repo. **Read `codemap.md` (atlas), `docs/api.md` (contract) and
`docs/architecture/` (v0.2 design — Memory Model, Lifecycle, Retrieval,
Distributed, Storage, Cognitive API) before editing code.**

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
examples: `mesh_simulation`, `signed_peer`. Matriz: **185+1 / 231+1 / 140+1**,
no_std gate ok. `.freebuff/` is tool state — gitignored, never commit.

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
cargo run --release --example mcp_client   # HOT TEST: drives mcp_server like an IDE (45 checks)
cargo run --release --example mesh_simulation --features p2p  # layered AI telepathy mesh
cargo run --release --example signed_peer --features p2p      # signed-transport seam flow
```

## Agent self-memory (hot test, v1.1)

The project agent is a test subject: `.opencode/opencode.json` registers the
MCP server as a local server (`NEURAL_SGDB_DB=.nsgdb/memory.db`, gitignored),
giving the agent `mcp__neural-sgdb__*` tools (15: remember/recall/rag_context/
explain/reinforce/forget/associate/related_to/contradicts/supersede/conflicts/
resolve_conflict/merge_memories/health/validate). Audit log in
`docs/hot_test.md`. Lessons learned (2026-08-13): `remember` returns the FULL
storage key (`md/L4/...`), always use it for follow-up (`explain`/`reinforce`
on the raw `mcp/...` key fails — was fixed); recall hits print `h.key | text`;
use the SAME words in queries as in the stored text (demo_embed is a trigram
hash, not a semantic model). Restart opencode after changing the config.

## Running tests

```bash
cargo test                                 # 185+1 tests (InMemory/FileStorage/TickvFile)
cargo test --features p2p                  # 228+1 (includes CRDT sync + mesh harness)
cargo test --no-default-features           # 139+1 (no_std core, host test harness)
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
  exposes read-only `health`/`validate` tools (P2-3 surface) for agents
  monitoring the DB.
- **Wire-codec fuzz harness** (`src/wire_fuzz.rs`, P2-4): the single LCG
  never-panic/roundtrip/truncation gate over ALL 8 wire types — add a new
  wire type there (plus its per-module `prop_tests`), and keep the matrix
  (185+1 / 231+1 / 140+1) green. `SignedEnvelope::decode` returns
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
  `w_sem·dist + w_rec·recência(/ts/hex) + w_imp·importância(camada)`.
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
