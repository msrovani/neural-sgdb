# Migration Guide — neural-sgdb

This file describes how binary formats and on-disk state evolve between
releases. It complements `docs/api.md` §Format versioning and
`VERSIONING.md`.

## Golden rule

**Never silently reinterpret old bytes.** Every format change must be: (1)
backward-decodable (old bytes still decode), (2) covered by a golden test
updated in the same commit, and (3) documented here. When a field is absent
in an old version, decode to a defined default — never guess.

## Format registry

| Format | Version | Encode/decode | Golden test | Lives in |
|--------|---------|---------------|-------------|----------|
| NMD1 | v1 (stable) | `MemoryDoc` | `golden_nmd1_bytes` | `src/memory_doc.rs` |
| MDM1 | v7 | side-table meta codec | golden/decode tests (via MemoryRecord) | `src/memory_doc.rs` |
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

## NMD1 (document) — stays v1, byte-identical to neural-os-core

The OS interop contract. NMD1 NEVER changes without a joint bump in
`neural-os-core`. New metadata is added via **side-tables**, not in-record:

| Side-table | Purpose | Since |
|------------|---------|-------|
| `sys/state/` | `MemoryState` (Active/Superseded/Archived/Invalidated/Decayed) | v0.2 |
| `sys/validity/` | temporal `from|until u64le` window (invalidate-not-delete) | v0.2 |
| `sys/meta/` | `MemoryMeta` (memory_id, source, confidence, importance, created_tick, parent_ids, clock_overflow; + scope [v4], entities [v5], content_type [v6], scope_dims + model_id [v7]) | v0.6 |
| `sys/version/` | per-version identity reverse index | v0.7 |
| `sys/rel/` | L6 relations (`<kind>/<a>#<b>`) + derived ART fwd/rev | v0.8 |
| `sys/conflict/` | `ConflictRecord` (MDR1 evidence per candidate) | v0.9 |
| `sys/crdt/` | durable `CrdtState` (opt-in) | v0.7 |
| `sys/audit/` | hash-chain ledger (`AUD1`, `sys/audit/<seq:016x>`) + snapshot for `rollback_to` | v1.1.10 |
| `sys/ttl/` | TTL per-key → 8B `expires_at u64le` (host-driven sweep `expire_ttl`) | v1.1.15 |
| `sys/event/` | evento temporal (start/close) → timeline `recall_timeline` | v1.1.15 |
| `sys/negative/` | ledger de negativos: `NDG1` (probes/first/last/scope/query) por `fnv1a64(scope‖0x1f‖query)` — "o que já procurei e não estava lá" | v1.1.24 |

## Known migrations

### MDM1 v7 — sem bump no v1.1.24 (canonicalização de escrita)

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

### MDM1 v6 → v7 (v1.1.15 — era do modelo)

Adiciona `scope_dims` (`ScopeDims{user,agent,app,run}`) e `model_id` (ADR-0007).
Migração **explícita**: registros v1–v6 decodificam com `scope_dims` vazio e
`model_id = ""` — nenhum byte antigo é reinterpretado. Ver ADR-0007 para a
política de era (`mixed_models`, `era_report`, custo de re-embed).

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
   `golden_record_bytes`, or add a new one) plus decode tests for the old
   version (v1/v2/v3/v4/v5 intermediate decodes — every field must advance
   `off`, see the MDM1 v4/v5/v6 bugfixes).
4. Update `docs/api.md` §Format versioning and this file.
5. If the OS shares the format, coordinate the bump in `neural-os-core` —
   never ship a divergent layout (bughunt parity is a contract).