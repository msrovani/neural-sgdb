# Aquisição Jev-Mem — ideias e melhores práticas (arXiv 2609.23986)

**Data:** 2026-09-23 · **Origem:** Jev-Mem (Jiang/Li/Li, UT-Dallas, 21/set/2026) +
Jev (TypeSafe AI) e Laya — modelos de decisão tipados, não generativos.

## A tese que adotamos

> "Many memory-control operations are semantic but not generative… these
> decisions typically produce bounded outputs such as labels, probabilities, or
> scores rather than free-form text."

Decisões de controle de memória são **classificações**. Usar um LLM
autoregressivo nelas é pagar geração + formatação + parsing por uma resposta
de espaço fechado. A interface é `𝒥(S, 𝒬)`: estado estruturado + **lote** de
perguntas explícitas → probabilidades ou distribuição sobre alternativas
mutuamente exclusivas. LLM generativo (System Two) só para síntese.

**Posição do nsgdb:** o core **já é** o plano de dados System-One —
determinístico, sem geração, microssegundos por decisão. O que o paper chama
de controlador aprendido (Jev) é um CONSUMIDOR externo do core; o papel do
nsgdb é dar a esse controlador o MATERIAL tipado das decisões, nunca decidir
por ele (doutrina: "the core does not decide").

## Mapeamento mecanismo a mecanismo (paper → nsgdb)

| Mecanismo no paper | nsgdb | Estado |
|---|---|---|
| Memory typing `t(v)` = 4 eixos sobrepostos (Eq. 7) | `TypeScores::derive` (ADR-0016, v1.1.27) | ✅ implementado — sobreposto, derivado na leitura |
| Multi-relational graph: semantic/temporal/causal/entity (Eq. 6) | `sys/rel/` 6 kinds (RelatedTo/Causes/Supports/Contradicts/DerivedFrom/Supersedes) + validity + entities 1-hop | ✅ supernos — 6 tipos de aresta > 4 views |
| Preserve canonical observations (não store-or-discard) | ADD-only BQ + `remember_episodic` (v1.1.4) | ✅ igual |
| Candidates por sinal determinístico (Eq. 8) | `recall_oversampled`/`rrf_fuse`/`recall_entities`/validity | ✅ mas **decomposto por sinal falta** (ver gaps) |
| Anchor retrieval RRF κ=60 (Eq. 16) | `recall_hybrid_rrf` — literalmente `k_rrf=60.0` | ✅ idêntico |
| Adaptive stopping (Eq. 17–22) | `recall_adaptive` + `RecallProbe::saturated` (ADR-0012) | ✅ parcial — gap no critério (ver) |
| Query routing por view com probabilidade (Eq. 11–12) | — | ❌ gap: recall é estático nos paths; o AGENTE roteia (lexical/semantic/entities são chamadas separadas) |
| Budget allocation por view (Eq. 13–14) | `recall_adaptive` escala global 1→4→8→16 | ⚠️ escala global, não por-relação |
| Score de candidato com 5 sinais (Eq. 23) | `recall_weighted_full` breakdown (sem/imp/conf/src) + rerank lexical | ⚠️ falta novelty e edge weight |
| Recency `ρ=1/(1+Δt/dia)` (Eq. 24) | `recall_weighted` w_rec sobre `/ts/` | ✅ equivalente |
| Redundancy/contradiction/obsolescence na manutenção | `supersede`/`contradicts`/`resolve_conflict`/`consolidate_recurrences`/`expire_old` | ✅ supernos — operações explícitas |
| Lote de perguntas por invocação | — | ❌ gap: cada recall = 1 round trip |
| Ledger de negativos | **nsgdb tem; o paper NÃO** | ➕ vantagem nossa |
| Bi-temporalidade (validity window) | `sys/validity/` + `recall_temporal` | ➕ vantagem nossa |

## Gaps reais adquiríveis (priorizados)

1. ✅ **FECHADO (v1.1.28)** — `recall_candidates` no core + MCP. Original: **`recall_candidates` — prefetch de candidatos com sinais DECOMPOSTOS**
   (Eq. 8/23 do paper). Dado um fato novo, devolver top-K pares
   `(candidato, {similaridade, overlap lexical, entidades compartilhadas,
   Δtemporal})` numa chamada. É o input exato de `𝒥` para julgar relação —
   hoje o agente precisa de N chamadas para compor esses sinais. **P0**.
2. ✅ **FECHADO (v1.1.28/28.1)** — tool `decide` (5º tool listado), lote, espaços fechados; `temporal_relation` com os 7 valores; stop com c_d/m_d. Original: **Decide em lote (`𝒥(S,𝒬)`)**: N perguntas com espaços de resposta
   fechados → probabilidades, num round trip. As perguntas respondíveis
   deterministamente hoje: suficiência de evidência (o probe do ADR-0012
   vira resposta tipada), relevância relativa de candidatos, relação temporal
   no vocabulário de 7 valores (before/after/during/contains/overlaps/
   same_time/unknown — timestamps do core respondem direto). **P0/P1**.
3. **Budget por relação** (Eq. 13–14): o escalonamento do recall_adaptive é
   global; o paper aloca por view ativa. Só compensa quando houver
   multi-hop de verdade no core (hoje as relações são consultadas por
   superfícies próprias). **P2** — registrar, não implementar agora.
4. ✅ **FECHADO (v1.1.28.1)** — c_d/m_d no `evidence_sufficient`. Original: **Critério de stop enriquecido** (Eq. 21): `saturated()` cobre u_d
   (utilidade marginal); falta modelar m_d (evidência obrigatória faltando)
   e c_d (contradição não-resolvida) como sinais de stop — o core JÁ tem
   `contradicts`/conflicts, falta alimentar o stop com eles. **P1**.

## O que NOT adotar

- **Controlador aprendido dentro do core** — viola a doutrina "the core does
  not decide" e a regra zero-dep/no_std. O controller é host-side.
- **Persistir type_scores** — segunda fonte de verdade (ADR-0016 já decidiu).
- **Duplicar observações por view** — o paper mesmo rejeita; nós temos um
  storage canônico com views derivadas.

## Consequência de contrato

Nenhum formato muda. Os gaps 1–2 são superfícies MCP novas (tools/alias) e
exigem bump de contrato no release em que pousarem.
