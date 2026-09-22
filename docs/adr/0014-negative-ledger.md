# ADR-0014 — Negative ledger: remember what was searched and was absent

Status: **Implemented** (v1.1.24 — `Sgdb::{note_absence, forget_absence,
recall_absences, recall_with_ledger, prune_absences, absence_count}`,
`src/negative.rs`, MCP aliases `recall_absences` / `note_absence` /
`forget_absence` / `recall_ledger`)

## Context

The database remembers what was **said**. An agent using it pays for the
inverse question constantly: it probes for a fact, gets nothing, and then has no
record of having asked. Three concrete costs, all measured in this repo's own
harnesses:

- **Re-probing is paid per turn.** `examples/agent_sim.rs` shows the recall cost
  per turn; the empty probe is the worst case — it pays the full candidate scan
  and returns nothing to reason about.
- **"No hits" is ambiguous.** The crate already teaches (`docs/doctrine.md`, the
  MCP `recall_empty_hint`) that an empty global recall may mean *null-scoping*
  (the memory exists in another tenant) rather than *absence*. Without a record,
  the second and third identical probes re-learn nothing.
- **Negative lessons are per-fact, not per-query.** The harness has
  `mom/anti-pattern` (ADR-0010) — a *positive* memory carrying a negative
  lesson. That answers "what went wrong", not "what have I already ruled out".

`recall_adaptive` (ADR-0012) made the *effort* of a probe a consequence of the
top-k boundary; nothing in the crate remembered the *result* of the probe.

## Decision

A **negative ledger** — an append-only-by-query side-table of verified
absences.

### 1. Storage: one side-table, no wire type, no format change

| item | value |
|---|---|
| key | `sys/negative/<fnv1a64(scope ‖ 0x1f ‖ norm_query):016x>` |
| value | `NDG1` magic(4) · ver u8 · `probes` u32le · `first_tick` u64le · `last_tick` u64le · `scope` u16len+bytes · `query` u16len+bytes |
| class | side-table (like `sys/validity/`, `sys/ttl/`, `sys/event/`) — **not** a wire type |

The 16-hex digest gives every key the same width, so no key is a prefix of
another (ADR-0005, ART rule 4). `0x1f` (unit separator) stops `("ab","c")` from
colliding with `("a","bc")`. NMD1/TKLV are untouched, so ADR-0004 is intact —
the side-table *is* the extension mechanism (ADR-0003).

### 2. Identity is the normalized query, and it is scoped

`normalize_query` is the BM25 tokenizer re-joined (`lexical::tokenize`), the
same normalization `consolidate_recurrences` uses. Therefore:

- case and punctuation do **not** fragment a record (`CHA, verde!` ≡ `cha verde`);
- paraphrases and accent variants **do** (`café` ≠ `cafe` — the tokenizer does
  not fold diacritics, the same known limitation as BM25);
- scope is part of the key, so one tenant's absence is never served as another's
  evidence (the null-scoping rule of ADR-0010, extended to negatives).

As with `entities` and the `Embedder` seam, the contract is "same canonical
string on write and query" — the core never guesses that two strings mean the
same thing.

### 3. Reinforce, never duplicate

Re-probing the same query+scope **bumps `probes` and moves `last_tick` on the
same key**. `first_tick` is preserved, so the record answers both "how many
times did I look" and "since when do I know it is not there". `u32` saturates.

### 4. `recall_with_ledger`: probe and ledger in one call

`Sgdb::recall_with_ledger(query, k, scope, now)` runs a **lexical** probe (the
embedding-free default of ADR-0008, and the same tokenization as the ledger
key) and reconciles the ledger with the outcome:

- **no hits** → registers/reinforces the absence, `recorded = true`, and returns
  the updated entry, so the caller sees "already probed N times";
- **hits** → **removes** the absence (`recorded = false`): the fact now exists,
  the negative record would be a lie. This is the self-healing path.

The default `recall` is untouched: mutating a read is exactly the kind of
implicit side effect the crate refuses, so the ledger only moves through APIs
whose names say they write.

### 5. Bounded by policy, not by hope

`prune_absences(now, max_age_ticks, max_remove)` is the retention hook for a
host scheduler; `max_age_ticks == 0` **disables** pruning (same convention as
`DecayConfig.half_life_ms`), `max_remove` bounds one pass. `absence_count()` is
cheap enough for `health`. `forget_absence` clears one entry explicitly.

### 6. MCP surface: aliases with their own shape

Four aliases on `tools/call` (the 4-tool `tools/list` contract is unchanged):
`recall_ledger`, `recall_absences`, `note_absence`, `forget_absence`. The
`format=json` response of the ledger is its **own** shape
(`{query, scope, probes, first_tick, last_tick}`), never the hit list — a
consumer parsing `format=json` hits sees no new field, and the ledger's JSON is
not mistakeable for a hit list.

## Consequences

- **New state to reason about.** The ledger grows with distinct probed queries,
  not with re-probes. It is the host's call how aggressively to prune; the crate
  ships the mechanism and documents the growth instead of hiding an eviction
  policy inside a write.
- **Fragmentation is visible, not silent.** Two paraphrases of the same question
  are two records; that is a documentation and prompting problem (use a
  canonical phrasing), not something the core fixes by guessing.
- **Absence is not a claim about the world.** A record means "this query, in
  this scope, was probed and matched nothing" — with the current index, the
  current scope filter and the current text of the query. It is evidence for the
  host, not truth, and the ledger says so by carrying `probes`/`first`/`last`.
- No dependency, no `std` requirement, no wire type: fuzzed by truncation tests
  in `src/negative.rs` (every prefix decode is `None`, no panic).

## Alternatives considered

- **Recording absences as memories** (`mom/absent` entities). Rejected: it
  pollutes the recall/entity indices with records that must never be returned as
  hits, and re-probes would grow the corpus.
- **An in-RAM-only set.** Rejected: the whole value is surviving the session
  boundary — a process restart is precisely when the agent re-probes.
- **Auto-recording every empty recall.** Rejected: silent mutation of a read,
  and it would fill the ledger with the null-scoping probes the doctrine tells
  agents to run deliberately.
