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

## Fechamentos posteriores no mesmo release

- **D4/D5**: onboarding e `cold_start.steps` viram `{step,text}` tipados
  (o ordinal estava dentro da string, duplicando o índice do array);
  `unseen_scopes` virou tupla `{label,count}` — era `"scope(count)"`, uma
  string que o consumidor reparseia.
- **D9**: `Sgdb::set_tie_margin(Some(m))` / `tie_margin_of()` — a margem
  de empate do state-first (`SCORE_TIE_MARGIN=50`) vira calibrável pelo
  host; `None` restaura o default, `0` desliga o state-first aproximado
  (só score exato empata). Seam de PROCESSO (ranking, não estado do banco).

- **v1.1.28.1**: `temporal_relation` aceita `a_key/b_key` (resolve
  `created_tick_of` + janelas de validade) e devolve os 7 valores do paper
  (`during`/`contains`/`overlaps` por interseção de janelas); o stop de
  `evidence_sufficient` incorpora `c_d` (contradição não-resolvida) e `m_d`
  (`required_keys` faltando) — Eq. 21 completa do Jev-Mem.

## Consequências de contrato

- `MCP_CONTRACT_VERSION` → **1.1.28**: 5º tool listado (`EXPECTED_MCP_TOOL_COUNT=5`),
  `ALIAS_SURFACE` +1 (`recall_candidates`), vocabulário de prosa/JSON mudou de
  VALOR (`Lexical`→`lexical` etc.), `dist` pode ser `null`.
- `Sgdb::resolve_known_key` agora `pub`; novos: `recall_adaptive_lexical`,
  `validity_window_of`, `recall_candidates`, `CandidateSignals`.
- Formato intocado (NMD1/TKLV/MDM1) — só superfície.
