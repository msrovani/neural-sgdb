# ADR-0016 — Scores de tipo SOBREPOSTOS derivados na leitura

**Data:** 2026-09-23 · **Status:** aceito · **Release:** v1.1.27

## Contexto

Consumidores tipados de decisão (Jev, Laya — modelos System-One que respondem
com probabilidades sobre espaços fechados, não texto; Jev-Mem arXiv 2609.23986)
anotam cada nó de memória com **quatro scores sobrepostos**: episódico,
semântico, procedural e preferência. O nsgdb tem as camadas L0–L7, que são
**exclusivas**: um fato é L3 *ou* L4 — categoria decidida na ESCRITA por
heurística da camada superior.

Para um controlador que pontua, a camada exclusiva é uma decisão já tomada
(prematuramente) e tomaada uma vez. A informação de que um verbatim
consolidado em L3 é *também* parcialmente semântico se perde.

## Decisão

Derivar os quatro scores **na leitura**, nunca persisti-los:

```rust
pub struct TypeScores { episodic, semantic, procedural, preference: f32 }
TypeScores::derive(layer, entities) -> Self   // função ÚNICA, pura
Hit.type_scores: Option<TypeScores>           // None sem meta
```

- **Sobreposição é o ponto**: L3 pontua episodic=0.7 **e** semantic=0.4 —
  fiel ao paper, onde os eixos não são mutuamente exclusivos.
- **Preferência** vem da convenção de entidade `pref/*` (o repo já a usa,
  ex. `pref/theme`) e de L7 (0.8) — sinal declarado, não extraído de texto
  (mesmo contrato de `entities`/`Embedder`: o core não faz NLU).
- **Nunca persistido** — NMD1/MDM1 intocados, sem bump de formato. O mesmo
  corpus deriva sempre os mesmos scores (determinístico).
- **`fields()`** é a tabela única do vocabulary (`episodic`, `semantic`,
  `procedural`, `preference`) — o serializador MCP consome ela, nunca o
  `Debug` do Rust (lição D1/D8 da auditoria de linguagem de máquina).

## Consequências

- Consumidores System-One recebem `type_scores` no `format=json` sem
  re-interpretar camadas; a escolha de weighting é deles (o core não decide).
- A heurística de derivação é simples por design: é um HINT de tipo, não uma
  classificação — a escrita continua soberana.
- Mudar a tabela de derivação não quebra formato, mas muda VALOR de campo
  visível ⇒ bump de contrato MCP.

## Alternativas rejeitadas

- **Persistir os scores** (novo campo MDM1): write-amplification + segunda
  fonte de verdade que diverge quando a heurística melhora.
- **Trocar camadas por 4 eixos exclusivos na escrita**: quebra ADR-0060/0063
  e o contrato byte-idêntico com o OS.
