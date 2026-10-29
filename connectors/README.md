# Connectors neural-sgdb ↔ apps claw

Camada de host para integrar agentes ao `mcp_server` sem alterar o core Rust,
NMD1 ou TKLV. **Não bumpa** o SemVer do crate (`VERSIONING.md` §Host
connectors). Alinhar docs/adapters ao `MCP_CONTRACT_VERSION` atual do
`examples/mcp_server.rs` (hoje **1.5.0** — 5 tools: `remember`/`recall`/
`health`/`curate`/`decide`; lexical-first).

**Status:** Hermes provider executável + contract tests; OpenClaw esqueleto TS.
```text
host → adapter → MCP JSON-RPC/stdio → examples/mcp_server.rs → Sgdb
```

## Entrega

- `mcp_client/`: cliente Python compartilhado, handshake `2025-11-25`,
  `tools/call`, scope/entidades canônicos, filtros e lock cooperativo.
- `hermes/`: `MemoryProvider` completo com quatro tools, `prefetch` bounded e
  plumbing de `sync_turn`.
- `openclaw/`: esqueleto de integração alinhado aos hooks oficiais atuais.
- `tests/`: contract test contra o `mcp_server` real.

Nenhuma dependência foi adicionada ao crate ou ao cliente Python.

## Contrato adotado

- retrieval MVP: `mode=lexical` + `format=json`;
- scope sempre explícito:
  `tenant/{id}/agent/{id}/workspace/{id}`;
- entidades:
  `host/openclaw|hermes`, `session/{id}` e
  `kind/preference|decision|constraint`;
- `remember` sem embedding grava L3; o adapter não ativa `DemoEmbedder`;
- auto-recall é limitado por hits, query e caracteres de contexto;
- auto-capture é **OFF por default** e, quando habilitado, usa somente filtros
  determinísticos conservadores;
- `forget` chama `curate(op=forget)`: arquivamento lógico, com história
  preservada. Não é deleção física/GDPR erase.

IDs são tratados como opacos e percent-encoded por segmento. Memória recuperada
é delimitada como evidência histórica não confiável, nunca como instrução.

## Limites e política de memória sensível

O core não avalia conteúdo, classifica terceiros, decide consentimento ou
garante proteção. Se o produto escrever memórias sobre outros sujeitos (menores,
contexto parental, saúde, educação, comportamento de terceiros) ou operar em
contexto vulnerável, o sistema é **insuficiente por si só**.

- **Política no host**: use `MemoryConnector` com metadados opcionais
  (`memory_category`, `subject_type`, `requires_consent`, `retention_rule`).
  Esses campos são repassados/protegidos pelo host, sem interpretação pelo core.
- **Recall sensível**: configure `RecallPolicy` com `only_own_scope`,
  `allow_cross_scope=False`, `vulnerable_context=True` e use
  `auto_recall_context(..., recall_context={"vulnerable_context": True})`.
- **Rastreabilidade**: o connector registra logs leves em escritas sensíveis.
  Use `forget` e remoção por escopo em caso de revogação de consentimento ou
  expiração de retenção.
- **Testes**: veja `connectors/tests/test_contract.py` para cenários sensíveis
  (escrita sobre terceiro, recall indevido, recall fora de escopo).

Consulte `docs/sensitive-agent-memory.md` e
`docs/adr/0021-agent-protective-policy-connector.md` no repositório.

## Build e contract test

No diretório raiz:

```powershell
cargo build --release --example mcp_server
python -m unittest discover -s connectors/tests -v
```

Opcionalmente aponte para outro binário já compilado:

```powershell
$env:NEURAL_SGDB_MCP_BIN = "C:\path\to\mcp_server.exe"
python -m unittest discover -s connectors/tests -v
```

O teste cobre handshake, lista exata das cinco tools MCP, store/recall/forget,
isolamento de scope, health e lock single-writer.

## Exclusão single-writer

Cada cliente que inicia um server dedicado cria atomicamente:

```text
<memory.db>.connector.lock
```

Um segundo adapter falha antes de iniciar outro writer. O lock é cooperativo:
processos que abrem o mesmo `memory.db` fora desta camada (por exemplo um
launcher antigo) não são detectados. Configure todos os hosts que compartilham
o arquivo para usar o mesmo launcher/lock, ou use um único processo MCP
multiplexado. Um lock órfão só deve ser removido após confirmar que o PID
registrado encerrou.

## Limites atuais

- O adapter Hermes é o caminho executável desta entrega.
- O OpenClaw está documentado como esqueleto porque sua extensão usa tipos e
  helpers internos do checkout OpenClaw; o passo seguinte é portar o cliente
  stdio para TypeScript dentro daquela árvore e executar os testes do host.
- Não há semantic/hybrid em produção nesta camada. Um modelo real exige
  embeddings do mesmo modelo na escrita e na busca, além de gestão explícita
  de era.
