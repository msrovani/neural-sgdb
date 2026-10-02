TL;DR — 5 tools: remember / recall / health / curate / decide. Default recall: `lexical` WITHOUT a vector; `semantic` if you pass `embedding=`; `hybrid` (RRF) if a REAL host embedder is set. Recall BEFORE you write. Scopes never leak into global recall. ADD-only.

Embedding (via B, offline): `NEURAL_SGDB_EMBEDDER=multilingual` (ou `candle`/`onnx`/`local`) → modelo `paraphrase-multilingual-MiniLM-L12-v2` (384-dim, in-process, `crates/nsgdb-embed`); o `model_id` da era é auto-declarado no `remember`. Fallback: `demo` (trigram, NÃO semântico) → `embedder_http` (via C, ollama/llama.cpp) → caller `embedding=` (via A) → `lexical` (mesmas palavras). Nunca misture dims/models: `health(view=era)` decide.

neural-sgdb is a MEMORY substrate (layers, identity, clock, scope), not a generic vector DB and not RAG.

The core does not decide. It returns typed evidence (Hit: path, type, payload_type, score, rel, matched_terms). YOU choose what enters the prompt and what to write.

Rules:
1. Default recall (P0.1): `semantic` if you passed `embedding=`; `hybrid` (RRF: semantic+lexical) if a REAL host embedder is configured (not `demo`/`none`); else `lexical` (same words, ADR-0008). Demo trigram is NOT semantic — do not imply cosine. Pass a real `embedding` (or `NEURAL_SGDB_EMBEDDER=demo`) for explicit semantic/hybrid. New dim on a live corpus → `health(view=era)`; do not force the write (BQ truncates).
2. Null-scoping: recall without `scope` sees ONLY global memories. A "missing" scoped fact is not an empty DB — use the same scope or recall_entities with the same entity strings.
3. ADD-only: new facts accumulate. Conflict is retrieval-time (supersede, recall_weighted), never silent overwrite.
4. Follow-ups (explain/reinforce/forget/supersede) use the FULL storage key remember returns (`md/L4/...`).
5. Entities are caller-supplied identical strings on write and recall_entities. The core never extracts entities from text.
6. Two passes: gather evidence (recall lexical + scoped; do not write) THEN remember. Exact quotes → remember_episodic (verbatim L2). Do not hoard.
7. Machine consumption: recall/rag_context format=json. Embedding/Binary are not prose — never treat payload as UTF-8 text.
8. This doctrine is stored in the DB: scope=nsgdb/doctrine key=md/L4/nsgdb/doctrine entities=doc/protocol,nsgdb/usage. Retrieve with recall(scope=nsgdb/doctrine, mode=lexical) or recall(entities=["doc/protocol"], scope=nsgdb/doctrine). Resource nsgdb://doctrine.
9. Maintenance (fim de ciclo): the DB informs (health(view=staleness|tensions|era)), the HOST decides and runs curate: expire_old → decay → consolidate_recurrences → audit_checkpoint (or forget to archive). Structure your memory for curation: episodes go to L2 (`remember_episodic`) so consolidate has something to merge; set validity/TTL to give expire_old work; decay needs `created_tick` in the SAME timebase as `now` (wall-clock ms) — do NOT decay records written with a counter tick, it ages everything. Release logs written as L3 `mcp/...` are append-history: archive (forget) the old ones, don't consolidate them.

MCP lists 5 tools: remember, recall, health, curate, decide. health(view=era) is era_report; health(view=tensions) is conflicts/unseen scopes/scope_issues; health(view=staleness) is TTL/Decay/contradicts/aging (read-only — curate manually). Resource nsgdb://session is the cold-start packet (read `cold_start.next_actions`). Legacy tool names still work if a client calls them; `contract.json` lists stable vs deprecated aliases.

Full self-program (any LLM/IDE): docs/agent-self-program.md — cold-start ritual, memory packet, capacity map, user-rule/system-prompt snippets. MOM entity roles on write/recall: mom/constraint, mom/decision, mom/fact, mom/pattern, mom/anti-pattern, mom/learning, mom/pref (identical strings). End of atomic task: curate(op=commit_run) (ADR-0010). Harness prompts: docs/harness-prompts.md. Obsolescence = supersede/forget — never MemoryState::Deprecated.
