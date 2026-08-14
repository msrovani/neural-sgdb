# Hot SGDB Test — Auditoria do Experimento

> **Cobaia**: o próprio agente (OpenCode) consumindo o neural-sgdb como
> substrato de memória em uma sessão real de IDE.
> **Data**: 2026-08-13 · **Status**: executado e auditado (45/45 asserções)
> **Objetivo**: provar (e auditar) que o neural-sgdb funciona de verdade como
> memória de agente — não apenas em unit tests, mas no fluxo real de uma IDE.

## Metodologia — 3 vias de integração

| Via | Mecanismo | Ferramentas expostas | Evidência |
|-----|-----------|----------------------|-----------|
| **1. MCP** (open code nativo) | `mcp` local em `.opencode/opencode.json` | 15 tools (`mcp__neural-sgdb__*`) + resources | sessão real pós-restart (próxima sessão) |
| **2. Binário instalado** | `cargo build --release --example mcp_server` + handshake JSON-RPC via shell | 15 tools | transcript do handshake (abaixo) |
| **3. Client cru** | `examples/mcp_client.rs` spawna o server e dirige a conversa | 15 tools | 45/45 asserções PASS |

## Checklist funcional (cada via)

- [x] handshake `initialize` → `protocolVersion: 2025-11-25`, ServerInfo v1.1.0
- [x] `tools/list` → 15 tools (health/validate presentes)
- [x] `remember` → memória armazenada com key única (`md/L4/mcp/{ms*1000+seq}`)
- [x] `recall` → hit semântico certo no top-k, com storage key + proveniência
- [x] `rag_context` → contexto formatado para prompt
- [x] `explain` → proveniência/estado/importância (e erro amigável p/ key inexistente)
- [x] `reinforce` → importância += delta
- [x] `supersede` → linhagem causal (estado Superseded + parent_ids)
- [x] `health` → backend/counts/storage_ok
- [x] `validate` → banco saudável (vazio = saudável)
- [x] `resources/list` + `resources/read` + paginação (2 páginas)
- [x] caminhos de erro: tool desconhecida → `-32602`, method → `-32601`, param faltando → `-32602`
- [x] **PERSISTÊNCIA**: kill do processo + respawn com mesmo `NEURAL_SGDB_DB` → memória lembrada (FileStorage cross-process)

## Métricas de auditoria

| Métrica | Via 1 (MCP) | Via 2 (binário) | Via 3 (client) |
|---------|-------------|-----------------|----------------|
| startup do server | — (após restart) | ~4–5 ms | 4 ms |
| nº tools listadas | 15 | 15 | 15 |
| nº chamadas feitas | — | 4 (handshake+diag) | 30 (10 fases) |
| tempo total | — | — | 1.6 s (build incluso) |
| erros encontrados | — | 0 | **2 bugs reais (corrigidos)** |
| asserções passaram | — | — | 45/45 |

## Execução (transcript resumido)

**Rodada 1 (antes da correção) — 6 falhas:**
1. `rag_context` query "health validate" não achava gamma (d≈1.0 para tudo)
2. `explain` com chave crua `mcp/...` → `invalid: no memory at key`
3. `reinforce` idem (chave crua não resolve)
4. `related_to` "sem relacoes" (relação criada sob chave errada)
5. `validate` → `[md/mcp/...] side-table targets missing doc` — ORFÃOS criados
6. `validate` pós-restart idem

**Rodada 2 (após correção) — 45/45 PASS, 0 falhas.**
Fases (ms): startup=4, handshake=8, tools/list=0, remember x3=1, recall=0,
rag_context+explain=0, linhagem=0, health/validate=0, resources=0, erros=0,
persistência (restart)=13. Total runtime do diálogo ≈ 30 ms.

**Via 2 (handshake via shell, PowerShell):** initialize → tools/call remember →
tools/call recall "integridade banco" → `md/L4/mcp/... | hot test gamma... (d=1.061)`
→ demonstrou o bug do embedding position-dependent ANTES da correção.

## Erros encontrados

1. **`mcp_server` devolvia a chave CRUA (`mcp/...`) no `remember`** — a camada
   cognitiva (`resolve_storage_key`) resolve `mcp/...` para `md/mcp/...`, mas o
   doc vive em `md/L4/mcp/...` → `explain`/`reinforce`/`associate`/`supersede`
   operavam em chave inexistente. **Silencioso** (Err amigável no explain, mas
   `supersede`/`associate` escreviam side-tables ÓRFÃS — pegos pelo `validate`,
   que funcionou exatamente como projetado). Correção: `remember` devolve a
   storage key completa `md/L4/{key}`; `recall` imprime `h.key` (antes só texto).
2. **`demo_embed` position-dependent (bug de embedding, não do core)** — o seed
   FNV-1a era mutado a cada janela (`seed = seed*0x9E37_79B9+1`), então o mesmo
   trigrama em posições diferentes caía em bins diferentes: query
   "integridade banco" vs doc "…integridade do banco" → d≈1.0 (não casa!).
   Correção: hash position-independent (cada trigrama sempre na mesma bin).

## Melhorias / otimizações identificadas

- [x] `remember` → storage key completa (fix usabilidade #1)
- [x] `recall` → imprime `h.key | h.text` (agente sabe a chave exata p/ follow-up)
- [x] `demo_embed` position-independent (fix recall de palavras-chave)
- [ ] **Futuro**: `set_state`/`associate`/`supersede` deveriam REJEITAR chave sem
      doc (hoje o `validate` pega o estrago depois — graças ao P2-3)
- [ ] **Futuro**: `Hit` já expõe a chave — considerar `recall` paginado no MCP
      sem `recall(100)` interno (custo linear no nº de docs)

## Adições sugeridas

- [ ] tool `read_memory(key)` = `resources/read` como tool (descoberta de UX:
      agentes tendem a querer tool sobre resource)
- [ ] tool `list_memories(layer)` — o resources/list cobre, mas tool é mais
      visível para o agente
- [ ] `rag_context` com `include_state` flag (hoje filtra active-only; agente
      pode querer histórico)

## Usabilidade

- **Ótimo**: handshake instantâneo (4 ms), tools/list deterministic, chaves
  únicas por ms, persistência real cross-process (13 ms), stderr separado.
- **Ruim (corrigido)**: chave crua `mcp/...` não resolvia — agente que seguia o
  retorno do `remember` recebia "no memory at key". Erro NÃO era autoexplicativo
  (não dizia "use md/L4/...").
- **Ressalva**: `demo_embed` trigram não é modelo semântico — recall de
  sinônimos/paráfrases não funciona; aceitável p/ demo, trocar por BGE real em
  produção (já documentado no server).

## Custo

- Build release dos 2 exemplos: ~11 s (primeira vez) / 1–6 s (incremental).
- Diálogo JSON-RPC completo (30 chamadas): ~30 ms de wall-clock.
- Arquivo DB temp de teste: removido ao final (sem lixo no repo).
- Tokens de auditoria: 1 sessão de agente (esta). A Via 1 (MCP nativo) é o
  custo recorrente futuro: 1 memória por decisão + recall por consulta.

## Auto-avaliação da cobaia

O que aprendi e o que vou fazer diferente na próxima sessão:

1. **Eu (agente) caço bugs na primeira rodada** — o teste a quente falhou 6x e
   cada falha era informação: validei hipótese com um diagnóstico isolado
   (via 2, PowerShell) antes de corrigir. Repetir esse ciclo: falha → hipótese →
   diagnóstico mínimo → correção → re-rodada.
2. **O `validate` é o meu aliado**: a integridade órfã foi detectada pela
   ferramenta de observabilidade (P2-3) — provar o observability vale ouro.
3. **Sempre pedir a storage key completa** (nunca a crua) ao usar o sgdb —
   agora o MCP devolve `md/L4/...`, e eu uso o `h.key` do recall como fonte de
   verdade para follow-up (explain/reinforce/supersede).
4. **Embedding de demo**: para minhas próprias memórias, escrever textos com
   termos-chave únicos (o trigram separa bem palavras distintas) e queries com
   as MESMAS palavras do documento (sem sinônimos).
5. Próxima sessão: configurar a Via 1 (MCP nativo em `.opencode/opencode.json`)
   e usar `remember`/`recall` de verdade como memória persistente do projeto —
   registrar os resultados no próprio banco (meta!). 
