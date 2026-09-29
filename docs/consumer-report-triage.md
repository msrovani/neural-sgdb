# Triagem do relatório do consumidor (neural-os-core, s413)

**Fonte:** `nsgdb-issue-report-s413.md` — agente AI do neural-os-core, uso real em
bare-metal (kernel `no_std` + mesh 2 nós + forget HITL). Relatório de alta
qualidade: cada evidência foi checada contra o código 1.2.1 antes da
classificação. Vereditos: **ACEITO** (implementar), **ADIADO** (roadmap),
**DESLOCADO** (o problema é do lado do OS/kernel, não do crate), **REJEITADO**
(colide com contrato de formato).

Data da triagem: 2026-09-29. Base: crate 1.2.1 (commit pós-`5e88899`).

---

## Bloco 1 — Bugs / riscos de correção

### ISSUE 1 — VectorClock: dois "vazios" (`new()` vs `Default` derivado) — **ACEITO (S)**
Confirmado em `src/memory_doc.rs`: `#[derive(..., Default)]` gera
`nodes: [0u8; 8]` enquanto `new()` usa `[0xFF; 8]`. O `PartialEq` semântico
mascara a diferença em igualdade, mas o `Default` derivado cria um clock com
nó `0` implícito via `counter_of` — hazard real p/ consumidor.
**Plano:** `Default` manual = `new()`; `is_vacuous()`; `is_zero_at(node)`;
teste golden `Default() == new()`.

### ISSUE 2 — `put` ticka o relógio em toda escrita (sem rota "sem autoria") — **ACEITO (S)**
Confirmado: `engine::put_inner(tick_local=true)` no `Sgdb::put`; só
`import_record` não ticka. A facade não tem rota para dado operacional próprio
(`sys/`, `hw/`), forçando overload de `import_record`.
**Plano:** `Sgdb::put_operational(layer, key, payload)` (indexa, não ticka) +
**tabela de decisão** `put` vs `import_record` vs `merge_remote` vs
`put_many_raw` documentada em `docs/api.md` e AGENTS.md.

### ISSUE 3 — `compact()` OOB na media — **DESLOCADO (parcialmente aceito)**
O `compact()` do TickvFile escreve em arquivo (`std::fs`), sem limite físico.
O OOB citado (RamFlash 256KB) é do **TickvLite/RamFlash do kernel**. O que
procede no crate: o trait `Storage` não expõe capacidade, então nem o
consumidor consegue pré-checar. **Plano:** seam de capacidade no trait
(`capacity() -> Option<u64>`, default `None` = ilimitado) + doc da política.

### ISSUE 4 — GC sem histerese / O(n²) — **DESLOCADO**
O TickvFile **não tem auto-GC**: `compact()` é manual; não existe
`maybe_gc`/HIGH_WATER no crate. O O(n²) medido (490 compacts/1000 puts) é
política do TickvLite do OS. **Opcional futuro:** política `gc_policy(Policy)`
opt-in no crate. Não é bug.

### ISSUE 5 — Forget atômico (tombstone → delete → audit) — **ACEITO (M)**
Confirmado: `set_state` em ghost key = `Err`; `forget()` hoje só arquiva
(`Archived`); `audit_forget` já existe (`AUDIT_OP_FORGET`, upstream `7e939f0`)
mas a composição não é canônica. Nota: `delete(key) -> Result<bool>` já distingue
não-existia (Ok(false)) de erro de storage (Err) — metade do ponto coberta.
**Plano:** `Sgdb::forget_purge(key, reason) -> ForgetOutcome{existed,
tombstoned, deleted, audit_seq}` — sequência atômica num call-site canônico.

### ISSUE 6 — FNV-1a forgeável + `digest` sobrecarregado por op — **ACEITO (parcial)**
Chain sem chave = tamper-evidence acidental, por design (ADR-0006: crypto é
seam) — o OS manter seu trail SHA-256/Ed25519 é a arquitetura correta, não
duplicação. Mas o **digest sobrecargado** é real: em `audit_forget`
`digest = fnv1a64(reason)`, em CHECKPOINT é digest de estado — decode/verify
não distinguem. **Plano:** doc por-op no `src/audit.rs` + `audit_entries(since,
limit)` / `audit_for_key(sk)` (leitura hoje exige full-scan). Trait `Hasher`
seam: **ADIADO** (aditivo, não urgente).

### ISSUE 7 — Audit não cobre o ciclo cognitivo — **ACEITO (M)**
`src/audit.rs` tem só CHECKPOINT/ROLLBACK/FORGET. `resolve_conflict` (a decisão
HITL mais importante) não deixa rastro no core. **Plano:** `AUDIT_OP_RESOLVE`
(conflict_id + winner + reason), agregados por checkpoint p/ ops de alta
frequência (decay/feedback). `AUDIT_OP_IMPORT`: avaliar custo de chain-growth.

---

## Bloco 2 — API / contratos

### ISSUE 8 — `SgdbError` sem códigos — **ACEITO (S, forma aditiva)**
`SgdbError` = `Storage(&'static str)/Corrupt/Invalid(&'static str)`. As strings
estáticas são doutrina no_std-safe e permanecem. **Plano:** método
`code() -> ErrorCode` com enum `#[non_exhaustive]` derivado da variante +
tabela estática de códigos. Não reescrever o enum.

### ISSUE 9 — `MemoryLayer` sem parse reverso — **ACEITO (S)**
`from_str` existe só para `RelationKind`; `MemoryLayer` tem `as_str`/`from_u8`
sem parse de `"L4"`. **Plano:** `from_label` + roundtrip test. Zero custo.

### ISSUE 10 — Framing de N NMD1 num blob — **ACEITO (verificar MSNP antes)**
NMD1 não carrega tamanho total (contrato). O OS inventou `[len u32le][NMD1]` —
contrato de wire que deveria viver no crate. **Primeiro passo:** verificar se o
wire type MSNP (já no crate, fuzzado) cobre o caso do mesh; se não,
`FrameWriter`/`FrameIter` com política documentada (fail-stop) + golden tests
de truncamento meio/fim.

### ISSUE 11 — `conflicts()` sem filtro; resolução sem outcome — **ACEITO (S)**
`conflicts()` clona tudo; `resolve_conflict` devolve `Result<(),>`.
**Plano:** `conflicts_open()`, `conflicts_count_open()`,
`ResolveOutcome{imported, superseded}`.

### ISSUE 12 — Key não indexada no lexical — **ACEITO (S)**
Comportamento real e não documentado: lexical indexa texto do payload, não a
key — `recall_lexical("net_config")` não acha `sys/net_config` com payload
`b"mode=slirp"`. **Plano:** `RememberOptions::index_key: bool` (opt-in) + doc
explícita do que gera companion lexical hoje.

### ISSUE 13 — Unificar família `remember_*` — **ADIADO (gosto de design)**
As assimetrias existem, mas helpers explícitos são escolha deliberada do repo.
`RememberRequest` unificado: avaliar com uso real acumulado.

---

## Bloco 3 — Performance / escala

### ISSUE 14 — `scan_prefix` materializa tudo — **EM PARTE JÁ RESOLVIDO (S)**
O relatório erra o detalhe: `scan_prefix` no crate devolve `Vec<(String, u64)>`
(keys, não valores) e **`scan_prefix_page(offset, limit)` já existe** (P1-6).
**Plano:** só `count_prefix(prefix)` (contagem sem materializar).

### ISSUE 15 — Sub-sector packing (512B) — **REJEITADO (contrato de formato)**
O alinhamento 512 é **contrato TKLV byte-idêntico com o OS** (regra 5 do
AGENTS.md). Mudar packing quebra a interop s410i que o próprio relatório
elogia. Seria migração coordenada crate+OS com bump de formato — não fix
unilateral. O ganho citado (HIGH_WATER vs live-set) é do lado do OS.

### ISSUE 16 — BQ vazio pós fast-mount — **DESATUALIZADO (boa notícia) + log mentiroso**
Desde o IDX2 (1.2.0), o mount restaura os bitvecs do snapshot
(`engine.rs`: "o pool semântico funciona COMPLETO no mount"). O recall
degradado **não é mais verdade**. Achado residual válido: a linha de log do
`open_with_snapshot` ainda diz "BQ vazio até rebuild" — **corrigir a string**.
`recall_degraded_reason` no health: avaliar se ainda tem caso (rebuild fallback).

---

## Bloco 4 — Testes / CI / DX

### ISSUE 17 — Statics globais de teste — **DESLOCADO**
Os statics FLASH/TICKV são do harness do kernel. Os testes do crate usam
`InMemory` por teste, sem estado global. O flake citado ocorreu no repositório
consumidor.

### ISSUE 18 — Fuzz/golden no CI — **ACEITO (S, orquestração)**
`wire_fuzz` **é** teste da matriz (9 wire types, roda a cada `cargo test`) — o
ponto "sem fuzz" está desatualizado. O que falta: job de CI dedicado com budget
+ golden vectors TKLV internos (a paridade hoje é provada do lado do kernel).
**Plano:** golden vectors de NMD1/MDM1/AUD1 no repo + workflow CI.

### ISSUE 19 — CHANGELOG por API — **ACEITO (S)**
CHANGELOG.md existe e é mantido por release, mas sem seção estruturada "novas
APIs / mudanças de contrato / deprecated". **Plano:** adicionar a seção a partir
de 1.3.0 (retroativo para 1.2.x). `#[deprecated]`: junto do ISSUE 2.

---

## Bloco 5 — Visão 2.0

### ISSUE 20 — `CognitiveOps` trait — **ADIADO (direção correta)**
Coerente com a doutrina (core reporta, nunca decide). Depende de fechar 5+7.
Também avaliar via MCP (superfície agêntica) antes de travar trait Rust.

### ISSUE 21 — `MemoryMeta.authority` no merge — **ADIADO (migração de formato)**
Requer **MDM1 v6→v7** com migração explícita (padrão das versões anteriores).
O problema é real (peer barulhento pode antecipar decisão humana), mas bump de
formato merece lote próprio com testes de mutação.

### ISSUE 22 — `explain_hit` — **ADIADO (composição de peças existentes)**
Peças todas presentes (meta + chain + lineage). Barato depois de 6/7.

### ISSUE 23 — `export_delta(since, max)` — **ACEITO (M)**
`keys_for_clock` + delta export = anti-entropy direcionado em vez de full-scan.
Aditivo, sem mudança de formato.

### ISSUE 24 — Reconhecimento — **RECÍPROCO**
Confirmado; a qualidade do relatório (evidências com commit/SESSION) é o que
permitiu esta triagem item a item.

---

## Ordem de execução acordada (substitui a tabela do relatório)

| Lote | Issues | Custo |
|------|--------|-------|
| 1. Fixes rápidos | #1 Default/vacuous, #9 FromStr, #14 count_prefix, #16 log stale | S |
| 2. Escrita operacional | #2 put_operational + tabela de decisão, #8 ErrorCode, #12 index_key, #19 CHANGELOG por API | S–M |
| 3. Ciclo cognitivo auditado | #5 ForgetOutcome, #7 AUDIT_OP_RESOLVE, #6 audit_entries/for_key | M |
| 4. Mesh | #10 framing (verificar MSNP), #23 export_delta | M |
| 5. Infra | #3 capacity seam, #18 golden vectors + CI | S–M |
| 6. 2.0 | #20 CognitiveOps, #21 MDM1 v7 authority, #22 explain_hit, #4 gc_policy opt-in | L |

**Rejeitados/deslocados (com razão):** #4 (política do OS), #15 (formato),
#17 (statics do kernel), #13 (adiado por design).
