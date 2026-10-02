# Benchmark / eval hygiene (F5, v1.3.3)

Regras obrigatórias para QUALQUER medição de memória no neural-sgdb. Fonte:
MemDelta (arXiv 2606.29914) — avaliações de memória frequentemente confundem o
método com o **modelo de embedding**, o backbone e o pipeline de retrieval.

## Números medidos (MemDelta, n=500, LongMemEval-S)

| Variável isolada | Delta |
|---|---|
| Trocar SÓ o modelo de embedding num pipeline idêntico | **+6.2 pp** (p=0.004) |
| Verbatim RAG vs contexto cheio (GPT-4o-mini) | 47.2% vs 49.8% (**p=0.34** — não significativo) |
| Memória auto-gravada pelo agente vs retrieval básico | 42% vs **47%** (retrieval vence) |
| Ordenação entre modelos (RAG vs full-context) | **inverte** entre Gemini/Sonnet/GPT — confound de backbone |

Lição: **se o embedding não for fixo, qualquer "ganho de arquitetura" pode ser
só o embedding.** Por isso a seam (ADR-0008) e o `model_id` (MDM1 v7) são
parte do protocolo, não um detalhe.

## Regras (checklist de qualquer bench/example que reporte recall/task)

1. **Pinar embedding**: reportar `model_id` + `dim` em TODA medição. O MCP já
   auto-declara `model_id` no `remember` (`embedder_model_id_for`); o bench
   sintético deve imprimir `embedding-model: none (synthetic)` — nunca inferir
   qualidade de um modelo real a partir dele.
2. **Fixar embedding entre comparações**: A/B (ex.: ADC-lite, state-first,
   RRF, futura travessia de grafo) só valem se ambos os lados usam o MESMO
   modelo/dim. O `recall@5` do `examples/bench.rs` usa vetores sintéticos
   correlacionados — mede QUANTIZAÇÃO, não semântica.
3. **Estratificar por modelo**: se medir com 2 backbones, reportar separado e
   só concluir se o ranking se mantém.
4. **Reportar custo de escrita**: `metrics.memory_writes` (o MemDelta mostra
   escrita podendo ser >80% do tempo do agente). Benchmarks que só medem
   retrieval escondem o custo dominante.
5. **`model_id` em todo resultado**: veredito `mixed_models` do `era_report`
   = a medição não é válida para comparar; migre (backfill) antes.

## Implementação corrente
- `examples/bench.rs`: imprime `embedding-model: none (synthetic)` no cabeçalho.
- MCP: `health.semantic_ready`/`retrieval_default` e `era_report` expõem a era.
- `scripts/install-embedding.ps1` provê o modelo real (via B) com `model_id`
  fixo — a medição com modelo real DEVE usar o label `multilingual`/`candle`.