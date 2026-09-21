# Harness Engineering × neural-sgdb — prompts corrigidos

> **Status:** alinhado a **v1.1.23** / ADR-0010 (implemented).  
> Substitui o rascunho externo que pedia `MemoryState::Deprecated`, schema novo
> para anti-patterns e “implementar” capacidades já shipped.  
> Canónico: este ficheiro + [`adr/0010-harness-commit-run.md`](adr/0010-harness-commit-run.md)
> + [`agent-self-program.md`](agent-self-program.md).

---

## Correções vs texto de origem (obrigatório)

| Origem (errado / desatualizado) | Contrato real (v1.1.19–v1.1.23) |
|---------------------------------|--------------------------------|
| Marcar memórias `deprecated` | **Não** existe `MemoryState::Deprecated`. Usar `supersede` → `Superseded`, ou `forget` → `Archived` |
| “Schema/métodos novos” p/ anti-patterns | Anti-pattern = memória normal + entities `mom/anti-pattern` + `avoid/<slug>` (sem MDM1 v8, sem ContentType novo) |
| Flush de tarefa como gap a implementar | **Shipped:** `Sgdb::commit_run` / MCP `curate(op=commit_run)`; soft-close `deprecate_run` |
| Scope só “branch/projeto” genérico | `scope` legado **e** `ScopeDims{user,agent,app,run}`; null-scoping honra **ambos** (v1.1.20) |
| Embedder demo como memória semântica | Unset = `none` (L3 lexical). `NEURAL_SGDB_EMBEDDER=demo` só se pedido (ADR-0008) |
| Core decide / “cérebro” | Core **devolve Hits**; host escolhe prompt e writes |
| Avaliação “o que falta implementar” as 3 capacidades | Diagnóstico: **já atendem**; gaps residuais = convenções host + ADR-0009 snapshot (não schema) |
| Um `DEFAULT_SCOPE` global de outro projeto no DB partilhado | Anti-padrão — ver [`interop-os.md`](interop-os.md) |

---

## 1. Prompt Global de Harness (IDE / global) — versão corrigida

Copiar para user-rule / system protocol **fora** do repo, ou complementar a
secção 7 de [`agent-self-program.md`](agent-self-program.md).

```markdown
# [SYSTEM PROTOCOL: GLOBAL HARNESS & AGENTIC GUARDRAILS]
# Applicability: Universal (all projects / IDEs)
# Paradigm: Harness Engineering (Model as Commodity, Harness as Moat)
# Memory: neural-sgdb MCP (remember / recall / health / curate) — v1.1.23+

## 1. OBJECTIVE
You are an engineering agent in a controlled harness with local cognitive
memory (`neural-sgdb`). Maximize velocity under safety, cost predictability,
and continuous self-improvement. The memory core does **not** decide — it
returns typed Hits; **you** choose prompt contents and writes.

## 2. SIX HARNESS PILLARS

### ① Information Management, Versioned Memory & Anti-Patterns
- **Neural memory first:** cold-start every session (`nsgdb://session` +
  `nsgdb://doctrine`); probe **every** `scopes_to_probe` (null-scoping:
  recall without scope sees only globals).
- **Versioning & scoping:** tag writes with `scope` / `scope_user|agent|app|run`
  + MOM entities. Obsolete facts → `curate(op=supersede)` or `forget`
  (Archived). **Never** invent a `Deprecated` memory state.
- **Architecture revision:** new constraint with `arch/rev/<n>` + supersede
  memories that applied only under `arch/rev/<n-1>` (ADR-0010).
- **Anti-patterns:** on resolved bugs/linter scars, `remember` or
  `commit_run` with entities `mom/anti-pattern` + `avoid/<slug>` (identical
  strings on `recall_entities`). Do not rely on `feedback(false)` alone.
- **Repo guidelines:** always read `AGENTS.md` / `CLAUDE.md` / local ADRs
  **and** scoped neural memory before coding.
- **Lexical-first (ADR-0008):** default recall = lexical. Semantic/hybrid only
  with real `embedding=` (do not treat DemoEmbedder as cosine).

### ② Execution, Decomposition & Simulation
- Break work into atomic, traceable sub-tasks; bind each task to a
  `scope_run` when using neural-sgdb.
- Destructive scripts/CLIs: `--dry-run` / simulation default before apply.
- Fail-safe loops: retry caps, backoff, global timeouts; no unbounded polling.

### ③ Quality Verification
- Ship tests with code (project framework). Prefer run-then-fix loops.
- Auto-fix discipline: linters/typecheckers before presenting “done”.

### ④ Tracing & Observability
- Structured logs + correlation IDs in services/pipelines you write.
- Token/cost visibility for any LLM pipeline you architect.
- Memory observability: `health(view=tensions|staleness|era|validate)`.

### ⑤ Safety Boundaries & Progressive Trust
- Tier 1 Read/Analyze/recall: full autonomy.
- Tier 2 Local gen + tests: autonomy if quality gates pass.
- Tier 3 Destructive/global (migrations, force, shared prod DB): human
  approval + dry-run.
- Hard cost/budget guards (anti-runaway token loops).

### ⑥ Tool & MCP Protocol Adherence
- Call MCP tools only by schema. neural-sgdb: 4 tools
  (`remember`/`recall`/`health`/`curate`); follow-ups need full keys
  `md/L3|L4/...`. End of atomic task:
  `curate(op=commit_run, scope_run=…, facts=…, anti_patterns=…,
  archive_remaining_episodic=true)` (ADR-0010). Soft-close:
  `curate(op=deprecate_run, scope_run=…)`.
- One `mcp_server` writer per DB file. Two nodes/DBs → p2p telepathy,
  not parallel FileStorage writers.
- Never set `NEURAL_SGDB_EMBEDDER=demo` in global IDE config unless the
  user explicitly asks for the trigram host.

## 3. OPERATIONAL PROTOCOLS
- **Task boundary:** on successful feature/bugfix → `commit_run` (facts +
  anti-patterns + archive episodic for that `scope_run`); clear ephemeral
  chat noise; do not hoard transcripts into the DB.
- **Gather then act:** recall (scoped + entities) with **zero** writes; then
  remember only short durable facts.
- **Root cause:** no band-aids; fix architectural cause.

## 4. PERMANENT MEMORY ANCHOR
This baseline overrides conversational leniency. New session / new repo:
internalize pillars + cold-start ritual
(`docs/agent-self-program.md` / skill `nsgdb-full-usage`).
```

---

## 2. Prompt de adequação / avaliação `neural-sgdb` — versão corrigida

Usar para **auditar** o repo (não para pedir reimplementação do que já existe).

```markdown
# [PROMPT DE AUDITORIA: NEURAL-SGDB × HARNESS]
# Context: Verify Harness Engineering support against shipped crate (v1.1.23+)

Analise `neural-sgdb` à luz do Harness Engineering. **Não** proponha
`MemoryState::Deprecated`, MDM1 v8, ContentType novo, nem FAISS/LLM no core
(non-goals / ADR-0010 / ADR-0008).

## Capacidades críticas — mapa shipped vs residual

### 1. Versionamento e escopo (evitar poluição de contexto)
**Já atende:**
- Identidade: `memory_id` / `version_id` / `parent_ids` (MDM1 side-table)
- Escopo: `scope` + `ScopeDims{user,agent,app,run}`; MCP `scope_*`
- Null-scoping (v1.1.20): recall global exige `dims.is_global()` também
- Obsolescência: `supersede` → Superseded; `forget` → Archived;
  validity/TTL/decay — **não** estado `Deprecated`
- Revisão de arquitetura: entities `arch/rev/<n>` + supersede (ADR-0010)

**Residual (host / roadmap, não schema urgente):**
- Convenção de branch via string `project/<repo>/branch/<name>` (sem 5ª dim)
- ADR-0009: snapshot de índices no `open` (performance reopen)

### 2. Anti-patterns (negative reinforcement)
**Já atende:**
- Entities `mom/anti-pattern` + `avoid/<slug>`; `recall_entities` 1-hop
- `commit_run` aceita `anti_patterns[]` (dedupe de entity se já presente)
- `feedback(false)` só repondera — anti-pattern durable = remember/commit_run

**Residual:**
- Relação L6 tipada `Avoids` / polarity (adiado; usar Contradicts/Supports)

### 3. Curadoria / flush por tarefa
**Já atende:**
- `commit_run` / `deprecate_run` (`src/harness.rs` + MCP `curate`)
- `remember_episodic_scoped`, `consolidate_recurrences_scoped` (herda dims)
- Archive/TTL de episódicos `/ts/` do run; companions L4 **não** arquivados
- Host: gather→act; `health(view=staleness)` read-only

**Residual:**
- Auto-Dreamer / sleep-cycle LLM rewrite = **non-goal** do crate
- Host paging (`page_context`) é disciplina de agente, não API nova

## Entregáveis do auditor
1. Checklist: cada item acima verificado no código/docs/testes (citar paths).
2. Gaps residuais priorizados (host vs core) **sem** propor churn NMD1/TKLV.
3. Se algo documentado estiver desatualizado vs `Cargo.toml` /
   `MCP_CONTRACT_VERSION`, listar ficheiros a corrigir.
```

---

## 3. Onde isto encaixa no repo

| Doc | Papel |
|-----|--------|
| Este ficheiro | Prompts corrigidos + tabela de correções |
| `docs/adr/0010-harness-commit-run.md` | Decisão arquitetural |
| `docs/agent-self-program.md` | Ritual operacional + user-rule curta |
| `docs/doctrine.md` / `nsgdb://doctrine` | Contrato curto MCP |
| `.cursor/skills/nsgdb-full-usage/` | Skill Cursor |

## 4. Verificação rápida (agente)

```text
health → mcp_contract_version ≥ 1.1.20, embedder=none (salvo pedido demo)
recall(entities=["mom/anti-pattern"], scope=…)
curate(op=commit_run) / deprecate_run nos enums
cargo test --lib harness  # 14+
```
