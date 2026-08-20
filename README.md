# neural-sgdb

**Persistent, transferable memory database for AI agents.**

> Memories, not data.

`neural-sgdb` is a memory substrate for AI systems: what it stores, syncs and
transfers are **memories** — with cognitive layer, vector clock and identity —
not generic data packets.

Born inside [neural-os-core](https://github.com/msrovani/neural-os-core), a
bare-metal OS with AI from boot, this project is the independent extraction of
its memory management system (SGDB) for community use.

## What it does

- **8 memory layers (L0–L7):** Sensory → Working → Short/Long-term Episodic →
  Semantic → Procedural → Identity
- **Semantic `remember` / `recall`:** binary-quantized vector search (BQ) with
  SIMD dispatch (AVX-512 / AVX2 / scalar), no external dependencies (no FAISS,
  no HNSW)
- **Memory transfer between nodes:** CRDT synchronization (last-write-wins) —
  memories travel between agents/instances with versioning, not packets
- **Power-loss safe persistence:** append-log with CRC; memory survives
  crash/restart (checkpoint/restore)
- **O(k) key/fact lookup:** ART (Adaptive Radix Tree) index Node4→16→48→256,
  no rebalancing
- **`no_std` + `std`:** runs on bare-metal and host applications — one core

## Why memories?

AI agents today have ephemeral context. `neural-sgdb` gives them a persistent
brain: memory layers with real semantics, microsecond semantic recall and the
ability to **transfer memories between instances** — no SQL, no traditional
filesystem, no external runtime.

## Status

**v1.1.6** ✅ — hits tipados para consumidor máquina, dual-mode (`no_std` +
`std`, zero dependencies). Versão da crate em `Cargo.toml`: **1.1.6**;
histórico em `CHANGELOG.md` e `docs/api.md`.

- `cargo test` on host: **229 tests + doc-test** (275 + doc-test with `p2p`,
  181 + doc-test with `--no-default-features`)
- `cargo check --no-default-features --target x86_64-unknown-none`: **clean**
- **Typed hits (v1.1.6)**: o recall devolve, por hit, `path` (semantic/
  lexical/entities), `content_type` (Text/Json/Code/Embedding(dim)/Binary),
  `payload_type` (o datum REAL do primário — `Embedding(dim)` para L4/L5),
  `score` bruto, `matched_terms` (grounding BM25), `validity` e `rel`
  (companion → primário). Projeção prosa só para Text/Json/Code — embeddings e
  binários NUNCA passam por `from_utf8_lossy`. **Seam de write**
  (`set_content_type`, MDM1 v6): quem fornece declara o rótulo estável e
  `declared wins` sobre o detector. **MCP `format=json`** devolve hits
  estruturados para consumo máquina. Exemplo ponta a ponta:
  `examples/two_ai_protocol.rs` (16/16 checks).
- **Recall stack**: BQ coarse → FP32 rescore, SIMD hamming (AVX-512/AVX2/
  scalar), auto-oversample by dimensionality, `recall_oversampled`,
  `recall_weighted` (recency·importance·semantic), `recall_lexical` +
  `recall_hybrid` (BM25 dual-path), `MihIndex` (multi-index hashing,
  sub-linear candidates), `recall_temporal` (bi-temporal intent),
  `recall_entities` (1-hop), `rag_context_reranked` (ancoragem lexical,
  `anchors=N`)
- **Storage**: `Storage` trait + `InMemory` + `FileStorage` (CRC32 append-log,
  persistent lazy handle ~38x, durability levels, atomic compaction) +
  `TickvFile` (byte-exact TKLV **with TKCK checkpoint fast-mount + GC/compact**
  + in-place `TKL\0` invalidation)
- **Memory semantics**: 8 layers L0–L7, `MemoryState` lifecycle (incl.
  `Decayed`), temporal validity window (`sys/validity/` —
  invalidate-not-delete), physical `Sgdb::delete` (tombstone + index
  removal, distinct from logical state), **memory identity + provenance**
  (`memory_id`, source, confidence, importance, parent_ids, scope, entities,
  content_type in `sys/meta/`, exposed as `Hit.provenance`), **dynamic
  VectorClock** (8-node fast path + bounded overflow registry, NMD1 stays
  byte-identical), CRDT sync with conflict preservation + delta sending
  (`p2p`), **replication units** (`MemoryRecord` carries doc + state +
  validity + provenance; `export`/`import`/`merge_remote`), **per-layer merge
  policy** (L2/L3 multi-value, L4 causal-LWW-with-history, L0/L1 local-only),
  a **3-node partition/rejoin test harness**, and **anti-entropy** (v0.7):
  clock announce/gossip through relay nodes, directed pull of the missing
  causal range (`keys_for_clock`), durable `CrdtState` for restart-safe
  clocks, **L6 associative memory** (associate/related_to/causes/supports/
  contradicts/derived_from on ART), **provenance-aware recall** (default =
  active only; historical opt-in) and a **deterministic `MemoryLifecycle`**
  (commit/promote/semanticize/decay/archive with no hidden wall clock),
  **first-class conflict model** (deterministic id, MDR1 evidence per
  candidate, `resolve_conflict` via evidence, `dismiss_conflict`),
  **cognitive API** (`reinforce`/`forget`/`explain`/`transfer_to`/
  `merge_memories`/`conflicts`/`resolve_conflict`/`feedback`/`diary`/
  `profile`/`expire_old`) and **MCP 23 tools** (provenance per hit in recall,
  `health`/`validate` observability, `era_report`, `remember_episodic`,
  retrieval modes semantic/lexical/hybrid, `recall_temporal`,
  `recall_entities`, ServerInfo v1.1.6) + **write-side era guard** (S1 no
  write também — `remember_semantic` fora da era do corpus vivo é `Invalid`)
  + **seams de conteúdo** (`Embedder` trait, `entities`, `content_type` —
  quem fornece declara)
- **Interfaces**: MCP server with `memory://{layer}/{key}` resources +
  `nextCursor` pagination + 23 cognitive/observability tools + tool
  annotations; `cargo run --release --example stress` (100k-op stress) and
  `--example bench`
- Full API contract in [`docs/api.md`](docs/api.md)
- Architecture + status in [`docs/architecture/`](docs/architecture/) and
  [`docs/implementation-status.md`](docs/implementation-status.md)

The reference implementation runs on bare-metal in the parent OS
(`k_ai::sgdb`, AGPL); this repo evolves separately (MIT OR Apache-2.0).

## Quick start

```rust
use neural_sgdb::{Sgdb, FileStorage};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = Sgdb::open(FileStorage::open("agent_memory.db")?)?;

    // L1 + L2 (RAM; persiste com checkpoint)
    db.remember_exchange("how's the weather?", "sunny, 24 degrees")?;
    db.checkpoint()?;

    // L4 semântico — embeddings fornecidos pelo caller
    let emb = vec![1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
    db.remember_semantic("turn:1", "sunny weather in sao paulo", &emb)?;

    // recall: BQ + FP32 rescore, auto-oversample; variantes ponderada/híbrida
    let hits = db.recall(&emb, 5)?;
    let recent = db.recall_weighted(&emb, 3, 1.0, 1.0, 0.5, 1000)?;
    let lex = db.recall_lexical("sunny weather", 3)?;

    // hits TIPADOS (v1.1.6): path/content_type/payload_type/score/matched_terms
    // — o consumidor máquina sabe QUE datum é e COMO parseá-lo
    for h in &hits {
        match h.content_type {
            neural_sgdb::ContentType::Embedding(dim) => { /* re-usa o vetor */ }
            neural_sgdb::ContentType::Json => { /* parseia verbatim */ }
            _ => { /* prosa */ }
        }
    }

    // L3 fato temporal + janela de validade (invalidar-não-deletar)
    db.remember_fact("user prefers dark mode", 42)?;
    db.set_validity("md/L3/ts/000000000000002a", 0, 1000)?;

    let ctx = db.rag_context(&emb, 3)?;
    println!("{ctx}");
    Ok(())
}
```

More: `cargo run --release --example bench` (benchmarks), `--example stress`
(100k-op stress), `--example mcp_server` (MCP), `--example two_ai_protocol`
(máquina→máquina: IA-A grava datum declarado, IA-B lê tipado, 16/16 checks),
`--example agent_protocol` (protocolo de decisão), `--example
memory_arena_eval` (utilidade da memória), and **telepathy** —
`cargo run --release --example p2p_telepathy --features p2p` exchanges
memories between two `Sgdb` instances via CRDT version sync + record pull
(`export_record` → `merge_remote` — state/validity/lineage travel too;
two instances converge with no central server). The crate doc
(`cargo doc --open`) is a runnable tour of the whole API.

**How the memory sync really works** — [docs/telepathy.md](docs/telepathy.md)
(EN) / [docs/telepathy-pt.md](docs/telepathy-pt.md) (PT-BR): the CRDT model,
the two-instance convergence flow, the honest cost (eventual consistency, no
global order, conflict preservation) and how an AI at the root of the process
arbitrates the preserved conflicts.

## Checklist pós-clone

```bash
git clone https://github.com/msrovani/neural-sgdb.git && cd neural-sgdb
git config user.name "Seu Nome"    # ver CONTRIBUTING.md
git config user.email "seu@email.com"
cargo build --release --example mcp_server
cargo test
bash scripts/mcp-smoke.sh          # 23 tools + health onboarding
```

No Windows, use `scripts\mcp-server.ps1` no lugar do `.sh`. Detalhes MCP:
[`docs/MCP.md`](docs/MCP.md).

## MCP (AI agents)

Guia completo: **[`docs/MCP.md`](docs/MCP.md)** (Cursor/Windows, smoke test,
embedder HTTP, troubleshooting).

`cargo run --release --example mcp_server` exposes 23 tools: `remember` /
`recall` / `rag_context` as MCP tools (JSON-RPC 2.0 over stdio, `2025-11-25`
handshake), memories as **resources** (`memory://{layer}/{key}`), recall with
opaque `nextCursor` pagination, retrieval **modes** (`semantic`/`lexical`/
`hybrid`), **typed hits** (`format=json` — estruturados p/ consumo máquina;
`remember(type=)` — seam de write), `era_report` + `health` onboarding, and
tool annotations (`readOnlyHint`/`destructiveHint`/`idempotentHint`).

### Cursor (Windows)

1. `cargo build --release --example mcp_server`
2. Abra o repo no Cursor — `.cursor/mcp.json` usa `scripts/mcp-server.ps1`
   (binário release, **sem** `cargo` no PATH do IDE).
3. Recarregue MCP se rebuildar o binário.

**Troubleshooting:** binário ausente → rebuild; recall vazio → `era_report`;
22 tools → binário desatualizado; ver [`docs/MCP.md`](docs/MCP.md).

### Scope / entidades (exemplo)

```json
{"name":"remember","arguments":{"key":"fact/diet","text":"vegan","scope":"user:bob","entities":["preference/diet"]}}
{"name":"recall","arguments":{"query":"diet","k":3,"scope":"user:bob"}}
```

Recall global (sem `scope`) não vaza scopes. Entidades exigem strings idênticas
em escrita e `recall_entities`.

### Outros IDEs

```bash
# macOS/Linux — launcher no repo
chmod +x scripts/mcp-server.sh
# Claude Code (ajuste o caminho)
claude mcp add neural-sgdb -- /path/to/neural-sgdb/scripts/mcp-server.sh
```

⚠️ The MCP recall embedding is a **demo** (character-trigram hash); for real
semantic recall, provide your own embeddings via `remember`/`recall` or see
[`examples/embedder_http.rs`](examples/embedder_http.rs).

## Benchmarks

`cargo run --release --example bench` — deterministic, reproducible numbers.
See **[`BENCHMARKS.md`](BENCHMARKS.md)** for methodology, measured environment,
per-run tables and honest caveats (this README no longer embeds raw numbers to
avoid stale/unreproducible claims).

## Docs

- **API contract** — [`docs/api.md`](docs/api.md) (Storage trait, seams,
  migration map, format versioning, feature matrix, CRDT policy)
- **arXiv preprint draft** — [`docs/paper/neural-sgdb-telepathy.tex`](docs/paper/neural-sgdb-telepathy.tex)
  (LaTeX, two-column article; **compiled [`neural-sgdb-telepathy.pdf`](docs/paper/neural-sgdb-telepathy.pdf)**,
  8 TikZ figures + design rationale, threats to validity, case study, appendix;
  upload the `.tex` source to arXiv)
- **Architecture (v1.1.6)** — [`docs/architecture/`](docs/architecture/)
  — Memory Model (01), Lifecycle (02), Retrieval (03), Distributed (04),
  Storage (05), Cognitive API (06) — documents the shipped cognitive memory
  system; [`docs/implementation-status.md`](docs/implementation-status.md)
  tracks capability vs code
- **AI agent guides** — `AGENTS.md`, `CLAUDE.md`, `codemap.md` (atlas),
  [`docs/MCP.md`](docs/MCP.md) (instalação MCP)
- **Contributing** — [`CONTRIBUTING.md`](CONTRIBUTING.md)
- **Governance** — [`SECURITY.md`](SECURITY.md) (policy + trust model),
  [`VERSIONING.md`](VERSIONING.md) (SemVer + release process),
  [`MIGRATIONS.md`](MIGRATIONS.md) (format migrations),
  [`ROADMAP.md`](ROADMAP.md) (honest status + non-goals),
  [`docs/adr/`](docs/adr/) (architecture decision records)

## License

Licensed under **MIT** **or** **Apache-2.0** (dual license), your choice.

## Roadmap

- [x] Portable core extraction (ART, MemoryDoc L0–L7, BQ + Hamming SIMD)
- [x] Pluggable Storage trait (InMemory + FileStorage) and injectable clock/CPUID
- [x] CRDT memory sync as optional `p2p` feature (`CrdtMemorySync` +
      `Transport` trait + std `UdpTransport`; symmetric LWW merge)
- [x] Published benchmarks (`cargo run --release --example bench` — ART
      P50/P99, BQ top-k, recall BQ vs FP32)
- [x] MCP server layer (`cargo run --release --example mcp_server` — exposes
      `remember`/`recall`/`rag_context` to AI agents via MCP over stdio;
      trigram demo embedding)
- [x] **Byte-exact TKLV/TKCK storage interop with the OS** (`src/tickv.rs`:
      byte-exact codec + OS-readable `TickvFile` backend; golden test +
      `scan_volume` re-parse; NMD1 and TKLV interoperable)

## Interop with neural-os-core

- **NMD1 (document):** `MemoryDoc` encode/decode byte-identical to the OS
- **TKLV/TKCK (storage):** `tickv::encode_record`/`scan_volume` replicate the
  TickvLite format (`crates/k_nano/src/storage/tickv.rs`) — a volume written on
  either side is read by the other. `TickvFile` writes 512-aligned records with
IEEE CRC32 over key‖val; tombstone `TKL\0`/`vlen=0`; EOF all-0x00/0xFF.
   `TickvFile` writes TKCK checkpoints (fast-mount via `try_mount_from_ckpt` with
   full `scan_volume` fallback) and supports GC/compaction (rewrites live set +
   ckpt + atomic rename).
