# Research 2026-10 — memória para agentes (achado arXiv mapeado p/ nsgdb)

Levantamento do agente de pesquisa (2026-10-02) sobre inferências em memória
para agentes de IA, e o que cada achado significa para o nsgdb. Medido onde há
número; marcado como **não sei** onde não há.

## Achados

| # | Achado (ref) | O que é | Para o nsgdb |
|---|---|---|---|
| 1 | **"Memória merece motor próprio"** — FluctlightDB (arXiv 2608.12365) | Episódio + proveniência + saliência + ativação por pista (`experience()`/`activate()`), consolidação estilo-sono, recall ponderado por proveniência. Reivindica 99% recall LoCoMo / 97.6% LME-S (**métricas autorais, não verificadas**) | Valida a tese "memórias, não dados": nsgdb já tem episódicos L2, `importance/confidence/source`, `recall_weighted_full`, consolidação, checkpoint. **Não adota** além do que já tem (custo ~0). |
| 2 | **Grafo multi-relacional + travessia guiada** — MAGMA (ACL 2026), survey Graph Memory (2602.05665), SAGE (2605.12061) | Memória = grafos semântico/temporal/causal/entidade; retrieval = travessia guiada por política, não só similaridade. MAGMA vence LoCoMo/LongMemEval (**sem delta exato publicado no nosso resumo**) | **ADOTADO (F2 GO, v1.4.0)**: `Sgdb::recall_graph` — BFS multi-hop sobre `related_to` (L6). Medido no nsgdb: recall@3 multi-hop 0.000→0.750–1.000, 12× mais rápido. Causal/temporal como view de retrieval = próximos degraus. |
| 3 | **Grafo condicionado à query / latente** — LGM (2609.18461), GraphMemix (2608.26983) | Grafo construído POR QUERY (topologia induzida), sparse autoencoder + GNN; evidência fraca espalhada compõe | **OUT (fora das premissas)**: exige modelo treinado + GNN; core zero-dep e "nunca gera vetor" (ADR-0002). |
| 4 | **Memória auto-evolutiva/auto-programável** — MemCodex (2609.39765), survey "Second Half" (2602.06052) | Camadas de memória como programas executáveis que evoluem; memória latente kv via memory-foundation-model. **+10.1% SR, 3.4× menos tokens** (números deles, não transferíveis) | **AFEITO (v1.4.1)**: o DB expõe os verbos/sinais (`supersede`, `consolidate_recurrences`, `expire_old`, `recall_graph`); a "auto-evolução" é política do HOST. Sonda `examples/self_policy_probe.rs` MEDIDA: B1 (verbos+sinais) **SR 3/3** vs B0 (passivo) **0/3** → **AFFORDANCE GO**; `memory_arena_eval` (naive 0/3 vs protocolo 3/3) mostra que a COMPETÊNCIA exige ENSINO → **autonomia fica no host**. |
| 5 | **Crise de avaliação** — MemDelta (2606.29914), MemoryArena (2602.16313), LME-V2 (2605.12493) | Trocar SÓ o embedding = **+6.2pp** (n=500, p=0.004); verbatim RAG ≈ full-context (p=0.34); memória auto-gravada 42% < retrieval 47%; custo de escrita pode ser >80% do tempo | **ADOTADO (F5, v1.3.3)**: `docs/benchmark-hygiene.md` — pinar `model_id`/dim, fixar embedding entre A/B, reportar `memory_writes`. `bench_graph`/`bench` seguem a regra. |

## Mapas nsgdb

| Capacidade | nsgdb | Status |
|---|---|---|
| Episódio/proveniência/saliência | L2, `importance/confidence/source`, `recall_weighted_full` | IMPLEMENTED |
| Consolidação | `consolidate_recurrences`, checkpoint | IMPLEMENTED |
| Bi-temporal / invalidar-não-deletar | `sys/validity`, `recall_temporal` | IMPLEMENTED |
| Grafo relacional (L6) | `associate`/`related_to`/`recall_entities` (1-hop) | IMPLEMENTED |
| **Travessia multi-hop** | **`recall_graph` (v1.4.0)** | IMPLEMENTED (F2 GO) |
| **Tie-break por proveniência** | **`recall_provenance_tiebreak` (v1.4.0)** | IMPLEMENTED (F1v2 GO) |
| Causal/temporal como view de retrieval | só temporal (re-rank) | REMAINING |
| Grafo condicionado à query / latente | — | OUT (premissas) |
| Auto-evolução da política | host-side (doutrina/harness) | PARTIAL (sonda não medida) |
| Higiene de avaliação | `docs/benchmark-hygiene.md` (v1.3.3) | IMPLEMENTED |

## Honestidade
- Deltas de papers (MemDelta +6.2pp, MemCodex +10.1%) são MEDIDOS mas em
  setups deles; no nsgdb, o único delta medido é o do `bench_graph` (F2:
  0.000→0.750–1.000; F1v2: 0.000→0.467).
- "MAGMA vence" e "FluctlightDB 99%" são alegações dos autores; não foram
  re-verificadas aqui.