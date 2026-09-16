---
name: nsgdb-full-usage
description: >
  Full-capacity usage of neural-sgdb (MCP remember/recall/health/curate): cold-start,
  null-scoping, lexical-first, eras, multi-agent scopes, telepathy 2-DB, what memory
  to load. Use when the user or tools mention neural-sgdb, NSGDB, nsgdb://session,
  nsgdb://doctrine, MCP memory, remember/recall/curate, or agent memory substrate.
---

# neural-sgdb — full usage skill

## Instructions

1. Read and follow the portable playbook:
   `docs/agent-self-program.md` (canonical self-program for any LLM/IDE).
2. Canonical short doctrine (also MCP `initialize.instructions`):
   `docs/doctrine.md` / resource `nsgdb://doctrine`.
3. **Every session** with the neural-sgdb MCP: execute the cold-start ritual
   in the playbook §2 (`nsgdb://session` → probe all `scopes_to_probe` → then act).
4. Load the memory packet in playbook §3 (entities `pref/*`, `adr/index`,
   `roadmap/non-goals`, `docs/telepathy`, `adr/0009`, doctrine).
5. Default `recall` mode = **lexical**. Semantic/hybrid only with a real
   `embedding` (or host embedder). Use `format=json` for machine hits.
6. Never hoard. Gather evidence then `remember` short facts + entities.
   Follow-ups need full keys `md/L4/...`.
7. Shared MCP DB file ≠ CRDT telepathy; one writer per file; two nodes →
   `telepathy_two_db` / `p2p`.

## Quick cold-start

```text
resources/read nsgdb://session
resources/read nsgdb://doctrine
health(view=tensions)
for scope in cold_start.scopes_to_probe:
  recall(mode=lexical, scope=scope, k=5, format=json)
```

## References

- Install: `docs/MCP.md`
- ADRs: `docs/adr/` (0001–0009)
- Telepathy: `docs/telepathy-pt.md`
- Project rule: `.cursor/rules/nsgdb-agent.mdc`
