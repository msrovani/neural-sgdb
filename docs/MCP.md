# MCP — neural-sgdb

Guia de instalação, contrato e troubleshooting do servidor MCP
(`examples/mcp_server.rs`).

## Contrato atual (v1.1.9)

| Campo | Valor |
|-------|-------|
| Protocolo | JSON-RPC 2.0 over **stdio** (uma linha JSON por mensagem) |
| Handshake | `initialize` → `protocolVersion: 2025-11-25` |
| `serverInfo.version` | `1.1.9` |
| Tools | **4** (`remember`, `recall`, `health`, `curate`) — 23 nomes antigos ainda funcionam em `tools/call` |
| Recall default | **lexical** (ADR-0008). Cosine: `embedding=` ou `NEURAL_SGDB_EMBEDDER=demo` |
| Embedder host | unset = none; `NEURAL_SGDB_EMBEDDER=demo` = trigrama explícito (**não** semântico) |
| Write sem vetor | `remember(text=)` → **L3** (`remember_text_with`); L4 só com `embedding=` ou `NEURAL_SGDB_EMBEDDER=demo` |
| Observabilidade | `health(view=era)` = era_report; `health(view=tensions)` = conflitos / superseded / scopes invisíveis |
| Resources | `nsgdb://doctrine`, **`nsgdb://session`** (cold-start JSON) |
| DB default | `NEURAL_SGDB_DB=.nsgdb/memory.db` (relativo ao cwd do processo) |

### Tools (4 + aliases)

Lista: `remember`, `recall`, `health`, `curate`.

Dispatch: `remember(user+response)` → episódico L2; `remember(text=)` sem vetor →
L3; `recall(entities|at|rag=true)` → 1-hop / temporal / rag; `health(view=era|validate|tensions)`;
`curate(op=explain|reinforce|…)`.

Aliases (ainda aceitos no call): os 23 nomes antigos.

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
