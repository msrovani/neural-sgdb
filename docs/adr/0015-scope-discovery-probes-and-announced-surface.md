# ADR-0015 — scope discovery: probes with provenance, and the announced surface is served

Status: **Implemented** (v1.1.26 — `Sgdb::scope_probes`, classification fix in
`scope_distribution`, dims routing in the MCP `recall`, `scopes_to_probe_dims`)

## Context

Scope has two mechanisms (ADR-0013): the legacy tenant string (`MemoryMeta.scope`,
MDM1 v4) and the 4-dim `ScopeDims` (MDM1 v7). ADR-0013 made them agree in the
bytes. This ADR is about a different failure — **not disagreement between the two
fields, but a scope that no reader could name.**

The repository's own harness (ADR-0010) writes durable facts with
`commit_run`, whose natural call is `curate(op=commit_run, scope_run="release-x")`.
That produces a doc with `ScopeDims{user:"", agent:"", app:"", run:"release-x"}`.
Because the legacy field mirrors `user`, `scope == ""` — and `scope_distribution`
classified scoped-ness with exactly that field:

```rust
let scope = m.scope;
if scope.is_empty() { global += 1 } else { *scoped.entry(scope)… }
```

Measured against a real DB (the probe that became the regression test), for a doc
written with `scope_run` alone:

```text
dims = ///release-v1.1.24        scope_of = ""
scope_distribution        → global_count = 1        ← counted as GLOBAL
                            scoped = [nsgdb/*]      ← not listed
scope_distribution_dims   → ///release-v1.1.24 = 1  ← the only viewer, NOT exposed
scopes_to_probe           → [nsgdb/doctrine, …]     ← cannot name it
recall_lexical_scoped("release-v1.1.24") → 0 hits
recall (global)                          → 0 hits   ← null-scoping filters it
recall_lexical_dims(run=…)               → 1 hit    ← the only route
```

So the memory was not merely unsolicited-but-reachable: it was unreachable by
**every documented route**. Three distinct defects compounded:

1. **Wrong classification.** A doc that is not global by dims (and which the
   global recall correctly filters out) was counted as global, so
   `global_memory_count` lied and `scope_labels` omitted it.
2. **No discovery surface.** `scope_distribution_dims` — the one function that
   sees the doc — was called by no MCP payload. The cold-start's
   `scopes_to_probe` came from `health.scope_labels` (top-8 by *display*), so a
   run/agent/app scope could not be named, and neither could the 9th most
   populous legacy scope.
3. **Announced ≠ served.** The `recall` schema has advertised
   `scope_user/scope_agent/scope_app/scope_run` since v1.1.14, and the handler
   read only `scope`: `recall(scope_run="x")` answered identically to
   `recall()` — 0 hits, `isError: false`, plus a prose hint telling the caller to
   use `recall(scope=…)`, which also returned 0. Worse, the *`entities`* sub-mode
   was routed to an arm that **did** honour dims, so two uses of the same tool
   behaved differently. This is the v1.1.23 finding (`view=index` served but
   absent from the announced `enum`) in the opposite direction, and it is the
   same root cause: *the model reads the schema, not the code.*

A manual repair had been applied earlier (rewriting the durable facts into
`nsgdb/release`, the scope the cold-start already probes). That fixed one batch
and left the trap armed: any `scope_run`-only write falls into it again.

## Decision

**Discovery is a core concern: the core classifies scopes with provenance and
publishes every route, and a parameter the schema announces must be served (or
refused loudly — never ignored).**

1. **Classification is by dims, not by the legacy mirror.** Global means
   `scope == ""` **and** `scope_dims.is_global()`. Anything else is scoped. This
   is the identity ADR-0013 established, applied to the statistics.
2. **One scan, one classification rule.** `Sgdb::scan_scope_metas` walks
   `sys/meta/` once and splits counts into `global`, `legacy`
   (`scope != ""`, reachable by `recall(scope=…)`) and `dims_only`
   (`scope == ""`, dims non-global — reachable only by the multi-dim filter).
   `scope_distribution` and the new `scope_probes` are both projections of it.
   The rule had been copied into three functions and the copies diverged; the
   label formatting (`ScopeDims::label`) had been duplicated too.
3. **Provenance travels, because shape cannot carry it.** `scope_probes()`
   returns `ScopeProbes{legacy, dims_only}` — two lists, two routes. The label
   strings alone are ambiguous: a legacy `scope` may contain `/`
   (`tenant/x/agent/y/workspace/z`, as the connectors use), so a dims label and
   a legacy label are indistinguishable by form. Whichever produced it knows.
4. **`nsgdb://session` publishes both.** `scopes_to_probe` stays the list of
   legacy labels (plus `default_scope` and the doctrine scope) and is built from
   the **full** distribution, not from the display-truncated `scope_labels`;
   `scopes_to_probe_dims` carries one descriptor per dims-only scope
   (`{label, user, agent, app, run, count}`) already shaped like `recall`'s
   arguments. A `2b` step in the cold-start tells the agent to probe them and
   that `hybrid`/`temporal` refuse dims.
5. **Announced parameters are served.** `recall_for_mcp` routes to
   `recall_lexical_dims` / `recall_scoped_dims` when any dim is present (dims
   win over the legacy `scope` — the multi-dim filter is strictly more
   specific), and the reply echoes the effective `scope_dims` so the caller can
   see the request was honoured. Where the core has no dims route (`hybrid`
   RRF, `temporal`), the call is **refused with an actionable error** rather
   than answered with the global pool, which would leak scope — the same
   posture as the dimension mismatch guard (S1).
6. **The write-side invariant is restored.** `finish_remember` promoted the
   legacy `scope` into `dims.user` only when the dims were *entirely* empty, so
   `remember(scope="proj/x", scope_run="r1")` wrote `dims.user == ""` after
   `set_scope`'s write-through had just set it — `set_scope_dims` clobbered it.
   `effective_scope_dims` prefers non-global dims, so the `user` dimension was
   *lost*: filtering by `scope_user` missed the memory and filtering by `run`
   found it under a different user. The promotion now fills an empty `user`
   whenever `scope` is non-empty, which is what ADR-0013 promised.

## Consequences

- **Behaviour change, deliberately, in a visible field:**
  `global_memory_count` no longer counts dims-scoped memories, and those
  memories now appear in `scope_labels` under their dims label. A corpus written
  entirely with `user`-style scopes is unaffected (labels are unchanged); a
  corpus with `run`/`agent`/`app` scopes changes. Because the *value* of a
  visible field changed and a previously-ignored parameter now does work, the MCP
  contract moves to **1.1.26**.
- `scopes_to_probe` is no longer truncated to the `health` display limit —
  discovery must not be bounded by a formatting decision.
- No format change: NMD1/TKLV untouched, MDM1 no bump. Also no *stored value*
  change beyond the write-side invariant fix, which was unreachable-by-API-
  correctness rather than a new representation.
- Extra cost: `scope_probes` is one more `sys/meta/` walk per session read,
  where the previous code reused the already-computed health payload. Accepted:
  correctness of discovery over one scan, and the walk is the same O(meta) as
  `health` itself.
- Guard: the hot test asserts behaviourally that `recall(scope_run=…)` finds a
  doc written with `scope_run`, that a wrong run does not, that the global path
  does not, that `hybrid`+dims refuses, and that `global_memory_count` does not
  move when such a doc is written. The lib test
  (`run_only_scope_is_scoped_not_global_and_is_discoverable`) was mutated —
  reverting the classification fails it with
  `global_count: 1, scoped: []`.

## Non-goals

- No dims route for `hybrid` or `temporal`. Inventing fusion/filter semantics
  for a filtered pool is a design decision with its own measurements
  (ADR-0012's lesson); until then the refusal is explicit.
- Not removing the legacy `scope` parameter or the legacy labels.
- Not deciding scope semantics for the host: the core publishes routes, the host
  chooses what to probe.
