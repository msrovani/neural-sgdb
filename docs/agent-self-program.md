# neural-sgdb — Agent self-program (uso pleno)

> **Para qualquer LLM / IDE / agente.** Este ficheiro + [`doctrine.md`](doctrine.md)
> bastam para **entender** e **auto-configurar** o uso em capacidade total.
> Doutrina = contrato curto (MCP `initialize.instructions`). Este playbook =
> como se programar e que memória carregar.

**Crate / MCP:** ver `Cargo.toml` e `MCP_CONTRACT_VERSION` (tools: `remember`,
`recall`, `health`, `curate`). Instalação: [`MCP.md`](MCP.md).

---

## 0. O que é (e o que não é)

- É **substrato de memória** (camadas L0–L7, identity, clock, scope) — **não**
  vector DB genérico, **não** RAG pronto, **não** o cérebro que decide.
- O core **devolve evidência tipada** (`Hit`: path, type, payload_type, score,
  rel, matched_terms). **Você** escolhe o que entra no prompt e o que grava.
- Fonte canónica curta: [`doctrine.md`](doctrine.md) ≡ resource `nsgdb://doctrine`.

---

## 1. Auto-programação (checklist — faça isto uma vez por IDE)

Copie e execute mentalmente / na config:

| # | Ação | Onde |
|---|------|------|
| 1 | Instalar MCP stdio (`scripts/mcp-install.*` + `mcp.json`) | [`MCP.md`](MCP.md) |
| 2 | Colar **User rule** (secção 7 abaixo) nas regras globais da IDE | Cursor / Windsurf / Claude … |
| 3 | Ativar skill / regra de projeto se existir | `.cursor/rules/nsgdb-agent.mdc`, `.cursor/skills/nsgdb-full-usage/` |
| 4 | Confirmar `health` → `mcp_tool_count=4`, `embedder` honesto | tool `health` |
| 5 | Em **toda** sessão nova: ritual cold-start (secção 2) | `nsgdb://session` |

Se o cliente MCP **ignorar** `initialize.instructions`, a user rule + este
playbook são a rede de segurança.

---

## 2. Cold-start obrigatório (toda sessão)

1. `resources/read` **`nsgdb://session`** — ler `cold_start` (`scopes_to_probe`,
   `steps`, `single_writer`, `telepathy_when`, `embedder_host`).
2. `resources/read` **`nsgdb://doctrine`** (ou confiar em `instructions` do handshake).
3. Para **cada** scope em `scopes_to_probe` (não só o default):
   - `recall(mode=lexical, scope=<s>, k=5, format=json)` **ou**
   - `recall(entities=[…], scope=<s>, format=json)`
4. Carregar a **memória operacional** (secção 3) se ainda não estiver no contexto.
5. `health(view=tensions)` — `unseen_scopes` ≠ DB vazio (null-scoping).
6. `health(view=staleness)` — o que envelheceu (TTL / Decayed / contradicts); **não** auto-forget.
7. Só então agir / `remember`. **Gather → act. Não hoarde.**

---

## 3. Memória que o agente precisa (pacote mínimo)

Grave / recall estes fatos (se faltarem, `remember` com as entities abaixo).

### Ontologia MOM (papéis cognitivos — strings idênticas write/recall)

| Entity | Uso |
|--------|-----|
| `mom/constraint` | Restrição latente — prioridade no **active** do paging |
| `mom/decision` | Decisão fechada de sessão |
| `mom/fact` | Fato estável do mundo / projeto |
| `mom/pattern` | Padrão recorrente |
| `mom/anti-pattern` | Lição negativa / o que **não** repetir (ADR-0010; + `avoid/<slug>`) |
| `mom/learning` | Lição / reflexão (citar evidência) |
| `mom/pref` | Preferência (além de `pref/*`) |

Usar **uma** role MOM + entities de domínio (`adr/*`, `pref/*`, …). Core **não** extrai.

### Preferências IDE (scope tipicamente `ide/cursor` ou default do launcher)

| Entity | Conteúdo esperado |
|--------|-------------------|
| `pref/idioma` | Comunicação em pt-BR (se o utilizador assim definiu) |
| `pref/memoria` / `nsgdb/usage` | Política: gravar constraints curtas; não dump de docs/transcripts |
| `constraint/multi-agent` | DB partilhado ≠ telepatia; 1 writer por ficheiro |

### Projeto neural-sgdb (scope `project/neural-sgdb`)

| Entity | Conteúdo esperado |
|--------|-------------------|
| `doc/protocol` | Doutrina (também em `nsgdb/doctrine`) |
| `adr/index` | ADRs 0001–0010 destilados |
| `roadmap/non-goals` | Sem FAISS/LLM/crypto no core; browser parked |
| `docs/versioning` | MAJOR = API/formato/no_std/deps |
| `docs/telepathy` / `constraint/p2p` | 2-DB = `telepathy_two_db`; conflito preservado |
| `constraint/open-snapshot` / `adr/0009` | Snapshot no open = design ADR-0009, ainda ROADMAP |
| `constraint/era` / `adr/0007` | Era + `model_id` (MDM1 v7); dim nova → Invalid |
| `adr/0010` / `mom/anti-pattern` | Harness: `commit_run` / `deprecate_run`; anti-patterns 1ª classe |
| `mom/constraint` | Taxonomia MOM + disciplina write/paging |

### Recall de arranque sugerido

```text
recall(entities=["pref/idioma","pref/memoria","nsgdb/usage","mom/pref"], scope=<default>)
recall(entities=["mom/constraint","adr/index","roadmap/non-goals","docs/telepathy","adr/0009"], scope="project/neural-sgdb")
recall(scope="nsgdb/doctrine", mode=lexical, query="doctrine protocol")
```

---

## 3b. Write-path filter (surprise → reinforce)

Antes de `remember`:

1. Gather: `recall(entities=[…], scope=…)` e/ou lexical com as **mesmas** palavras/entities.
2. Fato **idêntico** já existe → `curate(op=reinforce|feedback)` na storage key completa (`md/L4/…`). **Não** duplicar.
3. Objeto / conteúdo **mudou** → `remember` + `supersede` (ou version bump na mesma identidade).
4. Referência: `examples/agent_protocol.rs` (`remember_fact_checked`).

---

## 3c. Context paging (host — sem LLM no core)

1. Pool: `recall` / hybrid com k ampliado, ou `rag=true` + `rerank` se houver embedding.
2. **active** = top N (default 5); se o pool tiver `mom/constraint` ou `pref/*`, sobem para active.
3. **background** = resto como `{key, matched_terms, path}` — **sem** resumir com LLM.
4. Só então montar o prompt. Core reporta Hits; você escolhe.

---

## 4. Capacidade total — mapa de uso

### Write (`remember`)

| Intenção | Como |
|----------|------|
| Texto lexical (default) | `text=` sem embedding → L3 |
| Episódio verbatim | `user=` + `response=` → L2 |
| Semântico L4 | `embedding=` real (mesmo modelo/dim/`model_id`) ou host embedder |
| Multi-agente | `scope=` / `scope_user|agent|app|run` + `entities=` canónicas |
| Tipo máquina | `type=json\|code\|embedding\|binary` |

### Read (`recall`)

| Intenção | Como |
|----------|------|
| Default | `mode=lexical` (mesmas palavras) |
| Semântico / híbrido | `embedding=` + `mode=semantic\|hybrid` |
| 1-hop | `entities=[...]` (strings **idênticas** às da escrita) |
| Temporal | `at=` (epoch) |
| RAG | `rag=true` (+ `rerank` se fizer sentido) |
| Máquina | `format=json` — Embedding/Binary **não** são prosa UTF-8 |
| Histórico | `historical=true` se Active-only escondeu |

### Observabilidade (`health`)

| view | Uso |
|------|-----|
| (default) | onboarding, dims, doctrine pointers |
| `era` | ADR-0007 — empty/ok/mixed_dims/mixed_models |
| `tensions` | conflicts, superseded, unseen_scopes |
| `staleness` | TTL / Decayed / contradicts / aging — só classifica; curate manual depois |
| `validate` | integridade |

### Curadoria (`curate`)

Ops úteis: `explain`, `reinforce`, `feedback`, `forget`, `supersede`,
`expire_old`, `decay`, `consolidate`, `commit_run`, `deprecate_run`, `diary`, `profile`, `associate`,
`related_to`, `audit_checkpoint`, `audit_verify`, `rollback_to`, TTL/timeline/gc
(conforme contrato MCP atual). **Sempre** key completa `md/L4/...`.

### Multi-nó / telepatia (`p2p`)

- Mesmo ficheiro MCP = storage partilhado (1 writer).
- Dois DBs/nós → `examples/telepathy_two_db` / `p2p_telepathy` /
  `scripts/telepathy-demo.ps1`. Conflito preservado; arbitragem na **leitura**.

### Era (ADR-0007)

- Uma dim/modelo por DB vivo; `model_id` no meta (MDM1 v7).
- Dim nova → `health(view=era)`; **não** forçar write (BQ truncaria).
- Passado: lexical / entities.

---

## 5. Anti-padrões (não faça)

- Tratar `DemoEmbedder` / `NEURAL_SGDB_EMBEDDER=demo` como semântica real.
- Recall sem `scope` e concluir que “não há memória” (null-scoping).
- Hoarding / dump de markdown ou transcripts no DB.
- Follow-up com key curta em vez de `md/L4/...`.
- Dois `mcp_server` a escrever no mesmo FileStorage.
- Propor FAISS/HNSW/crypto/LLM **no core** (non-goals / ADRs).
- Assumir que snapshot de índices no `open` já existe (ADR-0009 = design Next).

---

## 6. Duas passadas (decisão)

1. **Gather** — recall lexical + scoped + entities; **zero** writes.  
2. **Act** — decidir; `remember` só o útil (fato curto + entities); quotes → episódico.

Protocolos de referência no repo: `examples/agent_protocol.rs`,
`two_ai_protocol.rs`, `memory_arena_eval.rs`.

---

## 7. Snippet — User rule (colar na IDE global)

```text
neural-sgdb / MCP: substrato de MEMÓRIA, não vector DB. O core não decide — devolve Hits tipados; eu escolho prompt e writes.

Obrigatório no início de cada sessão com MCP neural-sgdb:
1) Ler nsgdb://session (cold_start) e nsgdb://doctrine.
2) recall lexical/entities em CADA scope de cold_start.scopes_to_probe (null-scoping: sem scope = só globais).
3) Carregar preferências (pref/idioma, pref/memoria) e constraints do projeto (adr/index, roadmap/non-goals) se existirem.
4) Só então remember. Não hoarde. Follow-ups usam storage key completa md/L4/....
5) Default recall = lexical (ADR-0008). Semantic/hybrid só com embedding real. format=json para consumo máquina.
6) Um writer por ficheiro DB. Dois DBs/nós = p2p (telepathy_two_db), não o mesmo FileStorage em paralelo.
7) Comunicar em pt-BR se pref/idioma assim o disser.
Playbook completo: docs/agent-self-program.md no repo neural-sgdb (skill nsgdb-full-usage).
```

---

## 8. Snippet — System prompt mínimo (hosts sem “rules”)

```text
You use neural-sgdb via MCP (remember/recall/health/curate). Follow docs/doctrine.md
and docs/agent-self-program.md: cold-start every session, null-scoping, lexical
default, full storage keys, gather-then-remember, no hoarding. Prefer format=json.
```

---

## 9. Onde aprofundar

| Tema | Doc |
|------|-----|
| Doutrina curta | `docs/doctrine.md` |
| API | `docs/api.md` |
| MCP install | `docs/MCP.md` |
| ADRs | `docs/adr/` (0001–0009) |
| Telepatia | `docs/telepathy-pt.md` |
| Status | `docs/implementation-status.md` |

---

*Fim do self-program. Se és um agente: após ler isto, executa a secção 2 e
confirma `health` antes da primeira escrita.*
