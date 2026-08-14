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
| startup do server | — (spawnado pelo opencode) | ~4–5 ms | 1–4 ms |
| nº tools listadas | 15 | 15 | 15 |
| nº chamadas feitas | 15 (sessão real) | 4 (handshake+diag) | 30 (10 fases) |
| tempo total | — | — | ~30 ms (diálogo) |
| erros encontrados | 1 (storage: open append — FIX) | 0 | **2 bugs reais (corrigidos)** |
| asserções passaram | — | — | 45/45 |
| persistência entre sessões | ✅ (doc_count sobreviveu restart) | ✅ | ✅ (restart do processo) |

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

**Via 1 — sessão real (pós-restart do opencode):**
- `health` inicial: backend=file, doc_count=2 (persistidos de sessões anteriores
  na Via 2). **Persistência real entre reinícios do processo confirmada.**
- `remember` ×3 → chaves `md/L4/mcp/...` completas (o fix do Bug1 em ação).
- `recall` "hot_test bugs MCP server chave demo_embed" → top-1 correto d=0.634.
- `rag_context` "Matriz de testes clippy no_std gates" → top-1 d=0.477.
- `explain` → memory_id/version_id/source/created_tick (proveniência completa).
- `reinforce` +0.1 → `last_reinforced: 4` persistido (re-explain confirmou).
- `associate` + `related_to` → aresta viva (RelatedTo -> outra chave).
- `supersede` → memória antiga `state: Superseded` (linhagem causal).
- `validate` final → "banco saudavel". `health` final: doc_count=8, bq_len=4.
- **Achado de usabilidade**: matar o processo `mcp_server` do opencode derruba o
  toolset (`neural-sgdb_*` some) e o opencode NÃO re-spawna — precisa reiniciar
  o opencode. `cargo build` também falha (Acesso negado) com o binário em uso.

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
5. **NÃO matar o processo `mcp_server` em sessão MCP ativa**: o toolset
   `neural-sgdb_*` some e o opencode não re-spawna — custa um restart. Para
   rebuild do binário, parar o opencode primeiro (Windows bloqueia arquivo em
   uso → `Acesso negado` no link).
6. Via 1 é o caminho real de memória de agente: use `remember`/`recall` em
   todas as sessões; o banco `.nsgdb/` sobrevive restart e é gitignored.

---

## Follow-up 2026-08-14 — Audit Cognitivo (`examples/audit.rs`)

O teste da MISSÃO: **"retorna memórias, não dados"** — 3 baterias, 54
asserções, exit 0 (`cargo run --release --example audit`).

| Bateria | Asserções | O que prova |
|---------|-----------|-------------|
| **1. Attack** | 23 | embeddings hostis (NaN/Inf/vazio) rejeitados; chaves maliciosas (`/`, `#`, `sys/`, prefixo-ART) tratadas; set_state em chave fantasma rejeitado; nenhuma side-table órfã pós-ataque |
| **2. Corruption** | 11 | bit-rot no record L4 → recovery trunca no 1º record inválido; doc corrompido **nunca ressuscita** com bytes alterados; truncamento físico reabre limpo; `rebuild_indices` reconcilia |
| **3. Fidelity** | 20 | recall devolve **texto + proveniência** (memory_id/camada/estado), não bytes; forget arquiva (história preservada); supersede constrói DAG; validade temporal gateia `recall_at`; invalidate = validade, não estado; `recall_weighted` rankeia por importância de CAMADA |

**2 bugs reais achados e corrigidos pelo audit** (a cobaia atacou o core,
não só o MCP):
1. `set_state` aceitava chave fantasma → side-table `sys/state/` órfã
   (mesma família do bughunt da Via 1). Agora recusa `Invalid` antes de
   gravar; `Active` continua remove-only.
2. `validate()` contava só `md/L4/` no BQ, mas o `put_inner` indexa L4 **e**
   L5 — um doc L5 legítimo com embedding quebrava o validate (falso
   positivo). Regra replicada exata.

**Lições novas para a cobaia**:
- `remember` com chave crua `h/imp` NÃO resolvia para `md/L4/h/imp` —
  seguir SEMPRE com a storage key canônica completa (reforça a lição 3).
  **FIX (v1.1.2 P1)**: `resolve_known_key` agora resolve a chave crua para a
  canônica existente por prioridade de camada — a lição virou comportamento.
- `invalidate(key, now)` é **validade**, não estado; `until <= from` apaga a
  marcação — usar `now > from`.
- O default de importância é POR CAMADA (L4 = 1.0). **FIX (v1.1.2 P2)**:
  `recall_weighted` agora pondera a importância do DOC (`set_importance`/
  `reinforce`, penalty `1−imp`); sem meta, cai para a default da camada.
- Corrupção no meio do append-log é **truncada** no open (nunca aceita) —
  o custo da segurança é perder records após o ponto de bit-rot; docs
  anteriores e `rebuild_indices` seguem íntegros.

---

## Follow-up v1.1.2 — Guinea-pig plan (o que a cobaia consertou usando de verdade)

Depois de entregar o veredicto ("memórias, não dados" ✓), a cobaia virou
corretora: cada coisa que irritou/faltou virou uma entrega com teste de
regressão + commit (hot test agora 49/49 exit 0).

| P | O que era | Correção | Regressão |
|---|-----------|----------|-----------|
| **P1** | chave crua `h/imp` → `md/h/imp` fantasma; `meta`/`set_importance` falhavam **silenciosos** (3×) | `resolve_known_key`: fallback determinístico por camada (L4 1º) + `ensure_meta` com dica da canônica | `resolve_known_key_finds_layer_for_raw_key` |
| **P2** | `recall_weighted` pesava CAMADA (L4=0.0), não o doc | usa `Hit.provenance.importance` (penalty `1−imp`); sem meta → camada | `recall_weighted_uses_doc_importance_not_layer` |
| **P3** | `associate` aceitava ghost sem feedback (design) | `associate_checked` valida os 2 lados (ghost → `Err`, sem órfã); `associate` cru preserva o design | `associate_checked_rejects_ghost_keys_no_orphan_relation` |
| **P4** | demo_embed trigram não entende sinônimo | trait `Embedder` no core (no_std, zero-dep) + MCP aceita `embedding` do agente (`NEURAL_SGDB_EMBEDDER` fallback) | embedder 4 testes + hot test +4 checks |

Contrato P4 (quem fornece embedding usa o MESMO modelo na gravação e na
busca): vetor de 4 dims do agente ≠ 256 dims do demo — não casam, por
design. Matriz: **193+1 / 239+1 / 148+1**.
