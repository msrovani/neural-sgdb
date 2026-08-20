# Hermes MemoryProvider

Provider executável para a interface atual
`agent.memory_provider.MemoryProvider`.

## Instalação

1. Copie `connectors/hermes` para
   `$HERMES_HOME/plugins/neural-sgdb`.
2. Copie `connectors/mcp_client` para
   `$HERMES_HOME/plugins/mcp_client`.
3. Configure `memory.provider: neural-sgdb` no `config.yaml` do perfil.
4. Defina o comando do server e os IDs de scope.

Exemplo Windows:

```powershell
$env:NEURAL_SGDB_MCP_BIN = "C:\DEV\neural-sgdb\target\release\examples\mcp_server.exe"
$env:NEURAL_SGDB_TENANT_ID = "local-user"
$env:NEURAL_SGDB_AGENT_ID = "coder"
$env:NEURAL_SGDB_WORKSPACE_ID = "neural-sgdb"
```

Alternativa para comandos com argumentos:

```powershell
$env:NEURAL_SGDB_MCP_COMMAND = '["powershell","-NoProfile","-File","C:\\DEV\\neural-sgdb\\scripts\\mcp-server.ps1"]'
```

O processo recebe `NEURAL_SGDB_DB` do provider. Default:
`$HERMES_HOME/neural-sgdb/memory.db`.

## Configuração opcional

| Variável | Default | Limite |
|---|---:|---:|
| `NEURAL_SGDB_AUTO_RECALL` | `true` | hook `prefetch` |
| `NEURAL_SGDB_AUTO_CAPTURE` | `false` | hook `sync_turn` |
| `NEURAL_SGDB_RECALL_MAX_HITS` | `5` | 1–20 |
| `NEURAL_SGDB_RECALL_MAX_CHARS` | `4000` | 256–32000 |
| `NEURAL_SGDB_DB` | perfil Hermes | um writer |

Auto-capture não usa LLM nem extrai fatos livremente. Quando explicitamente
ligado, captura apenas frases do usuário com marcadores conservadores de
preferência, decisão ou restrição. Revise essa política para o seu domínio antes
de ativar.

## Tools expostas

- `memory_recall(query, limit?)`
- `memory_store(text, kind)`
- `memory_forget(key)`
- `memory_health(view?)`

`memory_forget` exige a storage key completa (`md/L3/...`) devolvida por
`memory_store` e arquiva logicamente a memória.
