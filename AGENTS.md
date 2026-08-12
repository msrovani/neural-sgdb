# AGENTS.md — neural-sgdb

Guide for AI agents (OpenCode, Cursor, Windsurf, Claude Code) working in this
repo. **Read `codemap.md` (atlas), `docs/api.md` (contract) and
`docs/architecture/` (v0.2 design — Memory Model, Lifecycle, Retrieval,
Distributed, Storage, Cognitive API) before editing code.**

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
   break silently; use fixed-width suffixes.
5. **Formats are contracts** — NMD1 (`memory_doc.rs`) and TKLV (`tickv.rs`) are
   byte-identical to the OS; do NOT change encode/decode/layout without
   updating the OS.
6. **Seams, not globals** — clock via `now: u64`, SIMD via `cpu_caps()`/
   `set_cpu_caps()`, log via `sgdb_log!`. No global engine statics.
7. **Verification** — `cargo test` (120+1 default, 143+1 `--features p2p`,
   81+1 `--no-default-features`) and `cargo check` (std + no_std) before
   committing.

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
```

## Running tests

```bash
cargo test                                 # 120+1 tests (InMemory/FileStorage/TickvFile)
cargo test --features p2p                  # 143+1 (includes CRDT sync + mesh harness)
cargo test --no-default-features           # 81+1 (no_std core, host test harness)
cargo check --no-default-features --target x86_64-unknown-none   # no_std gate
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
  `demo_embed` embedding is a trigram hash — NOT a real semantic model.
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
- **`MemoryRecord::decode`**: cuidado com flags — cada flag (vflag/metaflag)
  avança `off` mesmo no ramo 0 (bug real: metaflag=0 não avançava e o NMD1
  começava no byte errado).
- **ART** (`src/art.rs`): `scan_prefix_stats` expõe nós visitados (pruning de
  range — só desce em filhos que casam o prefixo). `MCP` (example): resources
  `memory://{layer}/{key}`, `nextCursor` paginação opaca, annotations
  `readOnlyHint`/`destructiveHint`/`idempotentHint`.
- **Format contracts**: NMD1/TKLV byte-identical to the OS — golden tests
  (`golden_nmd1_bytes`, `golden_record_bytes`, `fnv1a64_known_vector`) pin the
  layout; change them in the same commit as any format change.
