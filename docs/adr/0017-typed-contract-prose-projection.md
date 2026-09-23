# ADR-0017 — A estrutura tipada é o contrato; a prosa é projeção

**Data:** 2026-09-23 · **Status:** aceito · **Release:** v1.1.28

## Contexto

Auditoria do wire real nsgdb ↔ consumidor (Jev/Laya são modelos de decisão
tipados; LLMs generativos leem prosa) achou **nove defeitos** — todos do mesmo
padrão: dois renders paralelos (prosa e JSON) do mesmo struct, e renders
paralelos derivam (já tinham derivado: `type=Text` vs `"text"`).

## Princípio

**A estrutura tipada é o contrato; a prosa é uma projeção unidirecional dela.**
O `{:?}` do Rust sai do wire de vez — renomear variante Rust não pode mais
mudar o contrato publicado. Prosa permanece (LLMs generativos e a doutrina
vivem dela); o consumidor de máquina para de pagar o pedágio do modo.

## Decisões (os 4 movimentos)

1. **Vocabulário único**: `state_label`/`path_label`/`layer_label`/`stable_label`
   em `ctype.rs` são as ÚNICAS tabelas; prosa (`state=active`) e JSON
   (`"state":"active"`) consomem as mesmas. `dist: null` no caminho lexical
   (era constante `0.0` — a armadilha do "match perfeito"). Hot test agarra
   prosa e JSON pela mesma asserção.
2. **`decide` (5º tool, 𝒥(S,𝒬))**: N perguntas com espaços fechados →
   respostas tipadas num round trip. `evidence_sufficient` (adaptive stop do
   ADR-0012 vira resposta), `temporal_relation` (before/after/same_time por
   timestamp), `valid_at` (janela bi-temporal), `candidate_relevance`
   (sinais decompostos). Erro por item não falha o lote.
3. **`recall_candidates`** (core + alias MCP): prefetch do write path
   (Eq. 8/23 do Jev-Mem) — pool híbrido RRF com `sim_vec`, `lex_overlap`,
   `shared_entities`, `recency`, `rrf` DECOMPOSTOS. O core não decide.
4. **Gêmeo tipado de `validate`**: `structuredContent{healthy, issue_count,
   issues[]}` — a view mais acionável deixa de ser só prosa.
   + **D6**: `k=0` é erro `-32602` tipado, não sucesso vazio.

## Consequências de contrato

- `MCP_CONTRACT_VERSION` → **1.1.28**: 5º tool listado (`EXPECTED_MCP_TOOL_COUNT=5`),
  `ALIAS_SURFACE` +1 (`recall_candidates`), vocabulário de prosa/JSON mudou de
  VALOR (`Lexical`→`lexical` etc.), `dist` pode ser `null`.
- `Sgdb::resolve_known_key` agora `pub`; novos: `recall_adaptive_lexical`,
  `validity_window_of`, `recall_candidates`, `CandidateSignals`.
- Formato intocado (NMD1/TKLV/MDM1) — só superfície.
