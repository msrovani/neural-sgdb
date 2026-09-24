# ADR-0018 — Budget allocation per view: deferred (Jev-Mem P2)

Status: Accepted *(deferred — registered, not implemented; no code in this
commit by design)*

## Context

Jev-Mem (arXiv 2609.23986, Eq. 13–14) allocates retrieval budget **per active
relation/view**: each relation type gets its own probe depth and stopping
signal, instead of one global pool. The nsgdb equivalent,
`recall_adaptive` (ADR-0012), escalates **globally** `1→4→8→16` while the
top-k boundary stays ambiguous — the scale step is a property of the query,
not of each relation.

Per-view allocation only pays off when the core actually traverses multiple
relations in one logical query (multi-hop). Today it does not: relations are
consulted through their own surfaces (`recall_entities` for 1-hop entities,
`sys/rel/` scans for causal/graph edges, `recall_temporal` for bi-temporal),
each with an explicit API. There is no `recall_graph(query, hops=2)` to split
a budget across. Building the machinery now would allocate a budget to
nothing — the house rule demonstrated repeatedly in this repo (v1.1.17, the
adaptive-recall bench veredict) is: **measure the use case before building
the instrument**. `docs/jev-mem-adoption.md` already marked this gap as
**P2 — registrar, não implementar agora**; this ADR is that registration.

## Decision

1. **Defer.** Do not implement per-view budget allocation in any release
   until multi-hop retrieval exists in the core.
2. **Reactivation criterion** (both required):
   - the core exposes multi-hop traversal over `sys/rel/` + entities
     (e.g. `recall_graph(query, hops≥2)` returning fused candidates across
     relation kinds), **and**
   - measured evidence that per-relation depth differs materially in real
     workloads (one relation dominating probe depth while others starve —
     the analogue of the ADR-0012 bench that exposed global over-escalation).
3. **When reactivated**, the escalation becomes per active view: each
   relation kind owns its own `RecallProbe` depth and stop signal, fused at
   the end (RRF, κ=60 — same fusing discipline as hybrid). The global ladder
   stays as the fallback for single-view queries.
4. **Out of scope forever:** making the core *decide* which views to activate
   (that is the agent's job — "the core does not decide"); the core only
   reports per-view probe/cost so the caller can route (same posture as
   `RecallProbe` and `decide` in ADR-0017).

## Consequences

- Positive: the Jev-Mem gap inventory (`docs/jev-mem-adoption.md` gap 3) is
  closed formally as "deliberately deferred" instead of silently forgotten;
  future agents hitting Eq. 13–14 find the reasoning and the reactivation
  bar instead of re-litigating.
- Negative / cost: none now — no code, no surface change.
- Contract impact: none (no API, wire, or MCP change; no
  `MCP_CONTRACT_VERSION` bump from this ADR alone). If reactivated, it is an
  additive core API + MCP opt-in, following the ADR-0012 pattern (opt-in,
  default recall untouched).

## References

- Jev-Mem adoption map: `docs/jev-mem-adoption.md` (gaps; item 3 = P2).
- Global adaptive recall: ADR-0012 (`recall_adaptive`, `RecallProbe`).
- Typed decision surface: ADR-0017 (`decide`, 𝒥(S,𝒬)).
- Relation storage: `sys/rel/` six kinds; entities 1-hop (v1.1.4 item 10).
