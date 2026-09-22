# ADR-0013 — `ScopeDims` is authoritative; the legacy `scope` is its mirror of `user`

Status: **Implemented** (v1.1.24 — write-through in `Sgdb::set_scope`,
`validate` §6, characterization test)

## Context

Two scope mechanisms live side by side in the storage format:

| mechanism | version | field | introduced by |
|---|---|---|---|
| legacy tenant string | MDM1 **v4** | `MemoryMeta.scope: String` | v1.1.4 item 7 (mem0 multi-tenancy) |
| 4-dim scope | MDM1 **v7** | `MemoryMeta.scope_dims: ScopeDims{user,agent,app,run}` | v1.1.14 (host governance) |

The read side had already converged, in two places:

1. `MemoryMeta::decode` promotes the legacy field — *"v1–v6 com scope legado
   alimenta `user` quando dims vazias"* — so old records and peer payloads
   decode with `dims.user` filled.
2. `Engine::effective_scope_dims` does the same promotion for **raw** metas
   (a record written by `set_scope` alone has `dims` empty in storage), and the
   recall filters (`allows_scope_filter`, `recall_*_dims`) all read through it.

The write side had not. `Sgdb::set_scope(key, s)` wrote **only** the legacy
field, while `set_scope_dims` mirrored *both* (`scope = dims.user`). So the two
mechanisms agreed on every read of this crate, and disagreed in the bytes.

That gap was measured before anything was touched (the characterization test
pinned it, then was updated with the change — the before-state is the record):

```text
set_scope("kA", "user/ana")  ⇒  scope_of("kA") == "user/ana"
                                scope_dims_of("kA").user == "user/ana"   (decode promotes)
                                sys/meta/md/L4/kA has dims = {“”,””,””,””}   ← the bytes disagree
                                encode(decode(raw)) != raw               ← storage not canonical
                                validate() reported NOTHING
```

Consequences of that asymmetry:

- **Interop.** NMD1/TKLV are byte-contracts with `neural-os-core` (ADR-0004);
  MDM1 rides the same "side-tables are the extension mechanism" decision
  (ADR-0003) and travels in `MemoryRecord` on replication. A reader that
  decodes `scope_dims` as a field — rather than applying *our* compat
  promotion — sees a globally-scoped memory. Correctness depended on a
  decoder convention instead of on the data.
- **No invariant.** Nothing could tell a healthy record apart from one whose
  two scope fields disagreed, because the API path made disagreement
  unreachable *within* its own reads. Reachability returned the moment a meta
  was written by any other route.

## Decision

**`ScopeDims` is the authoritative scope; `scope` is a mirror of
`scope_dims.user`.** The identity `scope == scope_dims.user` is the invariant,
and it holds in both directions:

1. **Write-through.** `Sgdb::set_scope(key, s)` now writes `m.scope = s` **and**
   `m.scope_dims.user = s`. `set_scope_dims` already wrote `scope = dims.user`,
   so after this change the *persisted* `sys/meta/` bytes are canonical for
   every API path. Clearing the alias (`set_scope(k, "")`) clears `user`
   without disturbing `agent`/`app`/`run`.
2. **Read promotion stays.** The decode compat rule and `effective_scope_dims`
   are unchanged: records written by earlier versions (and peer payloads) keep
   reading correctly. This is a *write-side* canonicalization, not a
   re-interpretation of old bytes — MDM1 does **not** bump version, and no
   byte is reinterpreted.
3. **`validate()` §6 has teeth.** It decodes every `sys/meta/` record for a
   primary (`md/L3|L4|L5/`) and reports
   `legacy scope disagrees with scope_dims.user` when both are non-empty and
   differ. The API cannot produce that state any more; a hand-built meta
   (`Sgdb::put` with `doc.meta`) or an import from a divergent writer can — and
   now it is observable instead of silent.
4. **Documented authority.** `scope_dims_of` is documented as the authoritative
   accessor; `scope_of` as its `user` projection. `scope_distribution_dims`
   keeps projecting the four dimensions in its label (`user/ana///` = user set,
   agent/app/run empty) — the projection is honest, so the label did not change.

## Consequences

- **Behaviour change, deliberately:** `scope_dims_of` (and any host reading the
  raw `sys/meta/` bytes) now sees `user` after a legacy `set_scope`. This is
  the point of the ADR — the API surface never promised the two could disagree.
- No format change: same MDM1 version, same layout, same decode. Only the
  *values* written by one API became consistent.
- `set_scope` now writes one extra string into the meta blob it was already
  re-encoding; no extra record, no extra side-table, no extra tick.
- The characterization test
  (`scope_dims_is_authoritative_and_scope_is_its_alias`) carries the
  before/after in its header comment, so the next reader sees what moved and
  why, rather than a green test with no history.

## Non-goals

- No MDM1 version bump and no migration: the promotion rule is the migration
  (ADR-0004 — never reinterpret bytes, extend the format explicitly).
- Not removing the legacy `scope` field or the legacy accessors: hosts and
  connectors still call them, and removing fields is a format change.
- Not making `validate` an error: it reports, the host decides (the crate's
  standard posture — the core never decides semantic truth).
