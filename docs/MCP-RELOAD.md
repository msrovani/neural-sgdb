# MCP reload checklist (neural-sgdb)

Use após `git pull`, mudanças em `examples/mcp_server.rs`, ou recall/health desatualizados.

## Windows (Cursor)

1. **Build/install** (não conflita com MCP rodando):
   ```powershell
   powershell -File scripts/mcp-install.ps1
   ```
2. **Cursor → Settings → MCP → Reload** (ou reinicie o IDE).
3. **Verifique** (agente ou manual):
   - tool `health` → `mcp_tool_count: 4`, `onboarding` presente, `embedder` = `none` (a menos que `NEURAL_SGDB_EMBEDDER=demo`)
   - `health(view=tensions)` e resource `nsgdb://session`
   - `build_git` corresponde ao commit local (`git rev-parse --short HEAD`)
4. Se ainda 22 tools: desligue o MCP, apague `target/release/examples/mcp_server.exe` se locked, reinstale.
5. Se o MCP falha na subida (`failed during live tool discovery`): o Cursor
   usa **PowerShell 5.1** — `>&2` é erro de parse e stderr do `cargo` dispara
   `Stop`. Os launchers `.ps1` devem usar `[Console]::Error.WriteLine`.

## macOS / Linux

```bash
bash scripts/mcp-install.sh
# reload MCP no IDE
bash scripts/mcp-smoke.sh
```

## Config Cursor: workspace vs global

| Arquivo | Escopo |
|---------|--------|
| `.cursor/mcp.json` | Só este repo (commitado) |
| `~/.cursor/mcp.json` | Todos os projetos |

Se ambos definem `neural-sgdb`, o comportamento depende da versão do Cursor — prefira **workspace** para path relativo ao repo. Global: use caminho absoluto para `scripts/mcp-server.ps1`.

## Variáveis de ambiente (launcher)

| Var | Default | Uso |
|-----|---------|-----|
| `NEURAL_SGDB_DB` | `.nsgdb/memory.db` | Persistência |
| `NEURAL_SGDB_EMBEDDER` | `demo` | Trigrama 256-dim (não semântico) |
| `NEURAL_SGDB_DEFAULT_SCOPE` | `project/neural-sgdb` | Scope quando tool omite `scope` |

## Core (não só MCP)

- `Sgdb::remember_semantic_with` — write L4+L2 + scope/entities/type numa operação
- `Sgdb::recall_empty_hint` — diagnóstico quando recall vazio
- `Sgdb::scope_distribution` / `HealthReport.scope_labels` — observabilidade multi-agente
- `Sgdb::set_default_scope` — tenant default no host (MCP, SDK, examples)

Ver [`docs/MCP.md`](MCP.md) e [`docs/api.md`](api.md).
