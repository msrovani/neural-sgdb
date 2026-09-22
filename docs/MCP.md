# MCP — neural-sgdb

Guia de instalação, contrato e troubleshooting do servidor MCP
(`examples/mcp_server.rs`).

## Contrato atual (v1.1.24)

| Campo | Valor |
|-------|-------|
| Protocolo | JSON-RPC 2.0 over **stdio** (uma linha JSON por mensagem) |
| Handshake | `initialize` → `protocolVersion: 2025-11-25` |
| `serverInfo.version` | `1.1.24` (`MCP_CONTRACT_VERSION` em `examples/mcp_server.rs`) |
| Tools | **4** (`remember`, `recall`, `health`, `curate`) — 38 nomes antigos/alias ainda funcionam em `tools/call` (`ALIAS_SURFACE`) |
| Recall default | **lexical** (ADR-0008). Cosine: `embedding=` ou `NEURAL_SGDB_EMBEDDER=demo` |
| Embedder host | unset = none; `NEURAL_SGDB_EMBEDDER=demo` = trigrama explícito (**não** semântico) |
| Write sem vetor | `remember(text=)` → **L3** (`remember_text_with`); L4 só com `embedding=` ou `NEURAL_SGDB_EMBEDDER=demo` |
| Observabilidade | `health(view=era)` = era_report; `health(view=tensions)` = conflitos / superseded / scopes invisíveis; `health(view=staleness)` = TTL/Decay/contradicts/aging (read-only) |
| Resources | `nsgdb://doctrine`, **`nsgdb://session`** (cold-start JSON) |
| Harness (ADR-0010) | `curate(op=commit_run|deprecate_run)` + entities `mom/anti-pattern` |
| DB default | `NEURAL_SGDB_DB=.nsgdb/memory.db` (relativo ao cwd do processo) |
| Default scope | Preferir **por workspace** (`.cursor/mcp.json`). Global `~\.cursor\mcp.json` **não** deve forçar `DEFAULT_SCOPE` de outro repo no mesmo DB. |

### Tools (4 + aliases)

Lista: `remember`, `recall`, `health`, `curate`.

Dispatch: `remember(user+response)` → episódico L2; `remember(text=)` sem vetor →
L3; `recall(entities|at|rag=true)` → 1-hop / temporal / rag; `health(view=era|validate|tensions)`;
`curate(op=explain|reinforce|decay|consolidate|audit_checkpoint|audit_verify|rollback_to|commit_run|deprecate_run|…)`
(v1.1.10: metadado cognitivo; **v1.1.19+**: harness ADR-0010 `commit_run`/`deprecate_run`;
`audit_verify` expõe `structuredContent` com o `AuditReport`).

Aliases (ainda aceitos no call): **38** nomes — a lista vive em `ALIAS_SURFACE`
(`examples/mcp_server.rs`) e e pinada por teste, em vez de prosa (a contagem
"23" era do rework v1.1.8 e envelheceu com as ops cognitivas de v1.1.10 e o
harness de v1.1.19):

```text
associate  audit_checkpoint  audit_verify  close_event  commit_run  conflicts
consolidate  contradicts  decay  deprecate_run  diary  era_report
expire_old  expire_ttl  explain  feedback  forget  gc  merge_memories
profile  rag_context  recall_ann  recall_entities  recall_temporal  reinforce
related_to  remember_episodic  resolve_conflict  rollback_to  set_event
set_ttl  supersede  timeline  validate
```

**v1.1.21 — erro de tool desconhecida e util:** a falha continua sendo erro
`-32602`, mas passa a carregar `data.did_you_mean` (ate 3 sugestoes
deterministicas), `data.listed_tools` (as 4) e `data.alias_count`. Um agente que
errou o nome conserta o schema em uma chamada em vez de queimar um turno.

**v1.1.24 — ledger de negativos (4 aliases, JSON próprio).** `recall_ledger`
(`{query,k,now,scope}`) faz o probe lexical **e** reconcilia o ledger numa
chamada; `recall_absences` lista as ausências registradas (`{limit,scope,format}`);
`note_absence` registra explicitamente (`{query,now}`); `forget_absence` remove.
O JSON do ledger é **shape próprio** (`{query,scope,probes,first_tick,last_tick}`) —
nunca a lista de hits do `format=json`, então um consumidor de hits não vê campo
novo. Escopado: sem `scope` só as ausências globais aparecem (null-scoping).
Ver [ADR-0014](adr/0014-negative-ledger.md).

**v1.1.21 — `health(view=index)`** devolve `index_fingerprint`
(`{:016x}`), `doc_count`, `bq_len`, `indexed_embedding_dims` e o contrato de
medicao de custo de open (`open_rebuild_ms_last`, `open_rebuild_ms_max`,
`opens`). E opt-in de proposito: o fingerprint e O(n log n), entao o `health`
default nao paga esse custo.

**v1.1.23 — o `enum` anunciado agora lista todo view que o handler serve.** O
`view=index` (ADR-0011) existia desde o v1.1.21, o handler o servia e o hot test
o exercitava — mas `tools/list` anunciava o `enum` sem ele, e o modelo só lê o
schema. Feature invisível, corrigida no `enum` + descrição, com guard no hot test
(`tools/list` anuncia todos os views do `health`). Views do `health`:
`status` | `validate` | `era` | `tensions` | `staleness` | `index`.

**v1.1.22 — nenhuma mudança de superfície.** O contrato subiu junto com a versão
do crate (o `serverInfo.version` identifica a BUILD do server), mas as 4 tools,
os aliases, os parâmetros e os resources ficaram idênticos. O que entrou foi lib:
`recall_adaptive` (ADR-0012) — deliberadamente **não** exposto como tool/param
enquanto um host não medir que vale: o bench mostra que o threshold default faz
escalar quase sempre (ver `BENCHMARKS.md` §Recall adaptativo).

### Parâmetros importantes

- **`remember`**: `scope`, `entities` (lista de strings canônicas), `type`
  (`text`|`json`|`code`|`embedding`|`binary`), `embedding` opcional.
- **`recall` / `rag_context` / `recall_temporal` / `recall_entities`**:
  `scope`, `mode` (`semantic`|`lexical`|`hybrid`), `format=json` para hits
  estruturados (consumo máquina).
- **`health`**: onboarding JSON (`db_path`, `embedder`, dims indexadas,
  `mcp_tool_count`, **doctrine_scope/key**, link para embedder HTTP).
  Handshake injeta `instructions` = [`docs/doctrine.md`](doctrine.md).
  Resource `nsgdb://doctrine`. Seed no open do MCP (`ensure_doctrine`).

## Cold-start, multi-agente e 1 writer

O resource **`nsgdb://session`** inclui `cold_start`:

| Campo | Uso |
|-------|-----|
| `scopes_to_probe` | Scopes a recallar no início (inclui `unseen` do default) |
| `steps` | Ritual gather → act |
| `single_writer` | 1 processo MCP por ficheiro `NEURAL_SGDB_DB` |
| `telepathy_when` | Quando usar `p2p` (DBs/nós distintos) |
| `embedder_host` | Caminho semântico fora do core (ADR-0008) |

**Disciplina:**

1. Dois chats no **mesmo** DB partilhado = memórias comuns via storage — use
   `scope=agent/<id>` para isolar identidade sem rede.
2. Dois writers no mesmo ficheiro FileStorage = **risco** (append-log). Prefira
   um servidor MCP por path de DB.
3. Telepatia CRDT (já implementada e **verificada**):  
   `cargo run --release --example p2p_telepathy --features p2p`  
   `cargo run --release --example telepathy_two_db --features p2p` (2 ficheiros + reopen)  
   `powershell -File scripts/telepathy-demo.ps1`  
   Docs: [`telepathy-pt.md`](telepathy-pt.md) / [`telepathy.md`](telepathy.md).
4. Embedder real: `examples/embedder_http` ou `crates/nsgdb-embed` — nunca no
   core; unset `NEURAL_SGDB_EMBEDDER` = lexical.

Regra Cursor do repo: [`.cursor/rules/nsgdb-agent.mdc`](../.cursor/rules/nsgdb-agent.mdc).

## Self-program (qualquer LLM / IDE)

Pacote portátil para o agente **entender e auto-configurar** uso em capacidade
total (cold-start, pacote de memória, mapa write/read/curate/p2p, snippets de
user rule / system prompt):

- Playbook: [`agent-self-program.md`](agent-self-program.md)
- Harness prompts (6 pilares + auditoria): [`harness-prompts.md`](harness-prompts.md)
- Doutrina curta (MCP `instructions`): [`doctrine.md`](doctrine.md)
- Skill Cursor: [`.cursor/skills/nsgdb-full-usage/SKILL.md`](../.cursor/skills/nsgdb-full-usage/SKILL.md)

Se o cliente MCP ignorar `initialize.instructions`, a user rule + o playbook
são a rede de segurança.

## Build (obrigatório antes do IDE)

Preferir install em path fixo (não conflita com MCP rodando no Windows):

```bash
# Windows
powershell -File scripts/mcp-install.ps1
# Linux/macOS
bash scripts/mcp-install.sh
```

Binário em `.nsgdb/bin/mcp_server` (gitignored). Reload: [`MCP-RELOAD.md`](MCP-RELOAD.md).
`NEURAL_SGDB_DEFAULT_SCOPE` (launcher default `project/neural-sgdb`) aplica
escopo quando a tool omite `scope`. Core equivalente: `Sgdb::set_default_scope`.

Use `target/mcp-release` para rebuild enquanto o MCP está rodando — no Windows o
`.exe` em `target/release` fica **locked** pelo processo stdio do Cursor e o
linker não consegue sobrescrever (build “Finished” mas binário antigo).

Fallback: desligue o MCP no Cursor e rebuild normal em `target/release`.

## Cursor

### Windows (recomendado)

O repo inclui [`.cursor/mcp.json`](../.cursor/mcp.json) apontando para
[`scripts/mcp-server.ps1`](../scripts/mcp-server.ps1):

```json
{
  "mcpServers": {
    "neural-sgdb": {
      "type": "stdio",
      "command": "powershell",
      "args": [
        "-NoProfile", "-ExecutionPolicy", "Bypass", "-File",
        "${workspaceFolder}/scripts/mcp-server.ps1"
      ],
      "env": { "NEURAL_SGDB_DEFAULT_SCOPE": "project/neural-sgdb" }
    }
  }
}
```

**Troubleshooting Windows**

1. **`cargo` não encontrado no IDE** — use o script `.ps1` (binário release),
   não `cargo run` no `command`.
2. **`${workspaceFolder}` no `command`** — no Windows, mantenha `command`:
   `powershell` e o script em `args` (como acima).
3. **Binário ausente** — rode `cargo build --release --example mcp_server`.
4. **`tools/list` ≠ 4 tools** (`remember`/`recall`/`health`/`curate`) — binário
   antigo (a lista de 23 nomes era v1.1.6). Rebuild + reload.
5. **Recall vazio / dim mismatch** — chame `health(view=era)` (alias
   `era_report`); use o **mesmo** embedder/dimensão em `remember` e `recall`.
   Sem `embedding=` o default é lexical (mesmas palavras).

### macOS / Linux

[`scripts/mcp-server.sh`](../scripts/mcp-server.sh):

```bash
chmod +x scripts/mcp-server.sh
```

Config global ou `.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "neural-sgdb": {
      "type": "stdio",
      "command": "/caminho/absoluto/neural-sgdb/scripts/mcp-server.sh",
      "env": { "NEURAL_SGDB_DEFAULT_SCOPE": "project/neural-sgdb" }
    }
  }
}
```

## Claude Code / OpenCode

```bash
claude mcp add neural-sgdb -- /path/to/neural-sgdb/scripts/mcp-server.sh
```

Ou defina `NEURAL_SGDB_DB` e aponte para o binário release diretamente.

## Smoke test local

```bash
bash scripts/mcp-smoke.sh
```

Verifica: `mcp_tool_count=4`, schemas `remember`/`recall`, `health` com
`onboarding`, embedder `none` (salvo `NEURAL_SGDB_EMBEDDER=demo` explícito).

## Embedder HTTP (modelo real)

O core não embute modelo de embedding. Para plugar um endpoint HTTP:

```bash
cargo run --release --example embedder_http
```

Leia [`examples/embedder_http.rs`](../examples/embedder_http.rs): o trait
`Embedder` deve ser o **mesmo** na escrita e na busca (contrato S1 — dimensão
indexada). Configure `NEURAL_SGDB_EMBEDDER` conforme o exemplo MCP ou forneça
`embedding` explícito em cada `remember`/`recall`.

## Erros acionáveis

Erros de dimensão/era incluem hint para chamar `era_report`. Erros de embedding
citam o contrato same-model. Exemplo:

```
Invalid: query dims not in indexed_embedding_dims()

acao: chame `health(view=era)` (alias `era_report`) para veredito empty/ok/mixed_dims...
```

## Scope e entidades (multi-agente)

```json
{"name":"remember","arguments":{
  "key":"pref/theme",
  "text":"user prefers dark mode",
  "scope":"user:alice",
  "entities":["preference/theme"]
}}
```

```json
{"name":"recall","arguments":{
  "query":"dark mode",
  "k":5,
  "scope":"user:alice"
}}
```

Recall **global** (sem `scope`) não vaza memórias escopadas. Entidades exigem
strings **idênticas** na escrita e em `recall_entities`.

## Referências

- Contrato API: [`docs/api.md`](api.md)
- Hot test: [`docs/hot_test.md`](hot_test.md)
- Contribuição: [`CONTRIBUTING.md`](../CONTRIBUTING.md)
