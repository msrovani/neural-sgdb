# Future Horizons — neural-sgdb

> Mapa do que o ecossistema já faz (estado da arte), do que está só estudado
> (2026), e da visão além do alcance. Complementa `memory-landscape.md` (que
> mapeou o roadmap v0.2, entregue em v1.1.4). Pesquisa web: arxiv, GitHub,
> GitLab, fóruns Rust (users.rust-lang.org) e apps de memória, 2026-08.
>
> Separação proposital em 4 camadas:
> 1. **Outros usos do nsgdb** (superfícies de uso, fora de IDE) → agora em
>    `docs/adr/0008-local-embedder.md` §"Other uses".
> 2. **O que já é estudado/implanta** em outros SGDB (estado da arte).
> 3. **O futuro incipiente** — só estudado em papers 2026, pré-implantação.
> 4. **Visão além do alcance** — direções de longo prazo.

---

## 2. Estado da arte — o que outros SGDB já estudam e implantam (2026)

| Sistema | Já faz | Status no neural-sgdb |
|---|---|---|
| **mem0 v3** (abr/2026) | ADD-only single-pass (sem UPDATE/DELETE), entity linking, retrieval multi-sinal (semântico+BM25+entidades+tempo), fato gerado por agente como 1ª classe, scoping 4-dim (`user_id`/`agent_id`/`app_id`/`run_id`) | ✅ ADD-only, 1-hop entities, `mode` semantic/lexical/hybrid, `recall_temporal`. ⚠️ scope é `String` única, não multi-dim |
| **memrust** (Rust) | Recall híbrido HNSW+BM25+grafo de entidades+recência com **breakdown de score por sinal** (`vector\|lexical\|graph\|recency\|importance\|rerank`), TTL de working memory, consolidação episódico→semântico, MCP+HTTP+dashboard, dim fixa por coleção | ✅ hits tipados, `matched_terms`, guard de dim (S1). ✅ v1.1.10: `recall_weighted_full` breakdown por sinal, `decay_importance`, `consolidate_recurrences` |
| **engram** (Rust) | 95.8% LongMemEval-S; extração 5-dim (entidades, *epistemic type*, temporalidade, fonte, confiança); fusão RRF | ✅ meta tem `source`/`confidence`. ✅ v1.1.10: `recall_weighted_full` pondera confiança/fonte (`trust` p2p). ⚠️ sem RRF |
| **Letta (MemGPT)** | Memory blocks **sempre-no-contexto** (core/archival/recall), agente edita a própria memória, shared blocks, sleep-time compute | ⚠️ sem blocos fixos no contexto, sem shared blocks |
| **Graphiti/Zep** | Grafo bi-temporal, invalidar-não-deletar, episódio = proveniência | ✅ `sys/validity`, `supersede`, `DerivedFrom`/`Supports` |
| **mnem** (Rust) | Content-addressed, GraphRAG híbrido, **WASM/browser**, single binary ~40MB com **embedder ONNX embutido**, MCP+CLI+HTTP+Python, offline | ⚠️ sem WASM, sem embedder embutido (ADR-0008) |
| **mentedb** (Rust) | WAL/HNSW/KG, decay temporal, consolidação, belief propagation, **context assembly com consciência de atenção**, speculative cache | ✅ v1.1.10: `decay_importance` + `consolidate_recurrences`. ⚠️ sem belief propagation / contexto atento |
| **mnemos** (GitLab) | Memória como OS: **ciclo de sono** (REPLAY→CLUSTER→CONSOLIDATE→SYNTHESISE), worker de esquecimento GDPR, observabilidade de recall (KRONOS), webhooks, MCP+REST+OpenAI-compatible | ✅ v1.1.10: consolidação + auditoria hash-chain (`sys/audit/`, rollback). ⚠️ sem ciclo de sono automatizado, sem worker GDPR |
| **Supermemory/cognee** | Perfis, esquecimento (temporal/contradição/ruído), ciclo remember/recall/forget/**improve** | ✅ `profile`, `expire_old`, `feedback`, `forget` |
| **PlugMem** (ICML 2026) | LTM plug-and-play, unidades de conhecimento, Memory Inspector, plugin que reescreve o CLAUDE.md do agente | ⚠️ sem inspetor visual, sem "self-writing" de diretrizes |
| **Hindsight** | Embedding Postgres único, RRF, topo de BEAM@10M; MIT | n/a |

**Leitura**: o nsgdb cobre boa parte do estado da arte em **contrato de memória**
(ADD-only, bi-temporal, proveniência, multi-sinal, 1-hop) e, desde v1.1.10, em
**mecanismo**: decay (`decay_importance`), consolidação por recorrência
(`consolidate_recurrences`), breakdown por sinal (`recall_weighted_full` +
`Hit.score_breakdown`) e auditoria/rollback hash-chain (`sys/audit/`).
Superfícies (WASM/servidor) seguem fora do crate (ADR-0008).

## 3. O futuro incipiente — só estudado (papers 2026)

Sem implantação madura; são as apostas de pesquisa que devem virar produto.

- **Consolidação tipo sono** (neuro-inspirada):
  - **SCM** (2604.20943): NREM consolida (Hebbiano + downscaling), REM gera
    associações novas, **esquecimento intencional** por valor — 90.9% de redução
    de ruído, recall perfeito em 10 turnos.
  - **SleepGate** (2603.14517): forgetting gate sobre KV cache; reduz interferência
    proativa de O(n) para O(log n) — evidência de que esquecimento ativo é
    **arquitetura**, não prompt.
  - **HeLa-Mem** (2604.16839): grafo associativo Hebbiano, **distillation** de hubs
    episódicos em conhecimento semântico, **adaptive forgetting** com 3 critérios
    (peso da aresta < δ, dormência > δ, zero acesso recente).
  - **Auto-Dreamer** (2605.20616): **consolidador offline aprendido (GRPO)** que
    reescreve regiões da memória (region rewriting); abstrai, deduplica e omite —
    banco 12× menor com +7 pts.
  - **RecMem** (2605.16045): consolidação **por recorrência** — só invoca LLM
    quando interações similares recorrem; corta custo de construção em até 87%.
    ✅ **v1.1.10**: `consolidate_recurrences` entrega a versão determinística
    (repetição EXATA de texto normalizado → fato L3 com linhagem causal).
- **Rollback / recuperação pós-falha** (governança — o estágio mais carente do
  ciclo; Always-On survey: rollback = 27/435 trabalhos):
  - **ChronoMem** (2607.27773): version-control + **rollback semântico global**
    (snapshots por escrita; "desfazer em linguagem natural"); QA pós-exposição
    ~+10pts sobre RAG.
  - **MemTxn** (2607.27834): **transação** externa ao modelo — admite só escrita
    suportada por fonte (Ordered PatchTest), resolve versão por cronologia,
    recupera estado completo de snapshot journal.
  - **Dependency-Guided Rollback Repair** (2608.10502): grafo memória→ação,
    preserva estado com suporte independente, **replay seletivo** (85.3% recovery).
  - **FBD** (SIGIR 2026): **auditoria black-box** de esquecimento seletivo em RAG
    (PRE/POST/FILT, exposição no retrieval vs disclosação na resposta).
    ✅ **v1.1.10**: `sys/audit/` (hash-chain FNV-1a, ADR-0006) + `rollback_to`
    (snapshot cognitivo) cobre o núcleo de ChronoMem/MemTxn — sem "desfazer em
    linguagem natural" (a camada superior traduz a intenção para `rollback_to`).
- **Machine unlearning agêntico**: **SBU** (2602.17692) — unlearning síncrono
  **parâmetros + memória** (dependency closure, blocklist, audit log com
  hash-chain). "Memória como superfície de segurança de 1ª classe".
- **Memória envenenada / segurança** (MPBench 2606.04329, MemAudit 2605.23723,
  PIDP 2603.25164, "Coverage Is Not Containment" 2608.16044):
  - 6 classes de ataque de envenenamento; **defesas de prompt injection NÃO
    cobrem** (payload é semanticamente indistinguível).
  - Filtro de ingestão tem **limite teórico**; a saída é detecção por **demanda
    + recência** (quem realmente busca aquele cluster?).
  - **MemAudit**: auditoria causal pós-hoc por replay contrafactual + anomalia
    estrutural — ASR de 70%→0% e 83.3%→0%.
    ✅ **v1.1.10**: write-path hardening (`validate_written`) rejeita chave/
    scope/entidade hostil (traversal, control chars, `#`, overflow) ANTES da
    gravação — a lição de que a defesa é no caminho de escrita, não na leitura.
- **Esquecimento como métrica/benchmark**: MemoryAgentBench (esquecimento seletivo
  é a competência que ninguém domina), MemoryArena (recall passivo cai para
  40–60% em uso agêntico), Context Saturation Gap (Δ) — memória só importa se
  vencer contexto cheio. **Custo de construção/manutenção vira métrica de 1ª
  classe** (2606.06448): escrever barato + manutenção localizada > reorganização
  global (2606.24775).
- **Decay por curva de Ebbinghaus** (MemoryBank) e **saliência**: importância
  que **desce**, não só sobe (mentedb/memrust/HeLa-Mem/SCM usam decay exponencial).

## 4. Visão além do alcance

Direções onde ainda não há produto nem paper consolidado — o "além" do roadmap:

- **Memória multimodal** (imagem/áudio/embodied) — TeleMem (survey Tobias Weiss);
  o nsgdb é text/vector hoje; exigiria sensores no modelo de camada.
- **Retrieval causal, não por similaridade** — "por que essa memória levou àquela",
  contrafactual: as evidências devem ser recuperadas por **causa**, não por
  proximidade de vetor (open challenge do survey 2603.07670).
- **Memória composicional** — compor memórias existentes para resolver problemas
  novos sem gravar nada novo (a "memória além do recall").
- **Estabilidade multi-ano** — memória que dura anos sem degradar o ranking
  (tiered hot→frozen); o AGP do nsgdb (append-only, eras, compactação) é a base.
- **Esquecimento compatível com privacidade** — o "right to be forgotten" vira
  *unlearning verificável* (FBD/SBU) com auditoria, não deleção confiável em bug.
- **Gestão de memória aprendida end-to-end** — RL/GRPO treina o agente a gerir a
  própria memória (Auto-Dreamer, MemRL); memória que aprende a se gerir.
- **Auto-modelo / introspecção** — a memória sabe de si (SCM self-model;
  `health`/`era_report`/`validate` do nsgdb são o embrião) e se consolida ao
  "dormir" (sleep-time compute).
- **Memória paramétrica + não-paramétrica integradas** — unlearning e consolidação
  que atravessam pesos e store juntos (SBU aponta o caminho); o "gap paramétrico"
  que o survey MemoryArena expôs.
- **Telepatia como produto** — a raiz do processo arbitra conflitos com o
  histórico inteiro na mesa e o veredito **vira estado CRDT que converge**
  (`docs/telepathy-pt.md` §4): memória distribuída que decide, não só sincroniza.
- **Memória como substrate de identidade** — o agente continua quem é entre
  máquinas/harnesses (OneBrain/afair: o perfil viaja com você, o harness troca);
  a memória é a identidade durável do agente.

---

## 5. Leitura para o neural-sgdb (prioridade sugerida)

1. ✅ **Decay de importância + saliência** — **entregue em v1.1.10**
   (`decay_importance`, curva Ebbinghaus configurável, estado `Decayed`).
2. ✅ **Consolidação por recorrência** — **entregue em v1.1.10**
   (`consolidate_recurrences`: versão determinística exata do modelo RecMem).
3. ✅ **Breakdown de score por sinal no `Hit`** — **entregue em v1.1.10**
   (`recall_weighted_full` + `Hit.score_breakdown`: sem/rec/imp/conf/src).
4. **WASM + backend IndexedDB/OPFS** (médio) — habilita browser (mnem/Kurumi).
5. ✅ **Trilha de auditoria hash-chain + rollback** — **entregue em v1.1.10**
   (`src/audit.rs` + `sys/audit/`: `audit_checkpoint`/`audit_verify`/
   `rollback_to`, ADR-0006).
6. ✅ **Segurança do write path** — **entregue em v1.1.10** (`validate_written`
   no `remember_semantic`/`remember_text_with`/`set_scope`/`set_entities`/
   `Sgdb::put`). Faltam scoping multi-dim e detecção por demanda (recência).
7. **`nsgdb-embed` in-process** (longo) — candle/ONNX num crate separado
   implementando `Embedder` (ADR-0008); bundled embedder é a norma no ecossistema.

**Tese central**: a corrida virou de *quem guarda mais* para *quem governa
melhor* — esquecer certo, auditar, reverter, provar de onde veio. O nsgdb já está
à frente na base de governança; **decay + consolidação + rollback/auditoria +
breakdown de sinais** foram entregues em v1.1.10, com o **embedder local** como
enabler das superfícies offline (WASM) e do scoping multi-dim ainda em aberto.