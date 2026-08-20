neural-sgdb is a MEMORY substrate (layers, identity, clock, scope), not a generic vector DB and not RAG.

The core does not decide. It returns typed evidence (Hit: path, type, payload_type, score, rel, matched_terms). YOU choose what enters the prompt and what to write.

Rules:
1. Default recall is **lexical** (same words). Demo trigram is NOT semantic — do not imply cosine. Pass a real `embedding` (or `NEURAL_SGDB_EMBEDDER=demo`) for semantic/hybrid. New dim on a live corpus → call era_report; do not force the write (BQ would truncate).
2. Null-scoping: recall without `scope` sees ONLY global memories. A "missing" scoped fact is not an empty DB — use the same scope or recall_entities with the same entity strings.
3. ADD-only: new facts accumulate. Conflict is retrieval-time (supersede, recall_weighted), never silent overwrite.
4. Follow-ups (explain/reinforce/forget/supersede) use the FULL storage key remember returns (`md/L4/...`).
5. Entities are caller-supplied identical strings on write and recall_entities. The core never extracts entities from text.
6. Two passes: gather evidence (recall lexical + scoped; do not write) THEN remember. Exact quotes → remember_episodic (verbatim L2). Do not hoard.
7. Machine consumption: recall/rag_context format=json. Embedding/Binary are not prose — never treat payload as UTF-8 text.
8. This doctrine is stored in the DB: scope=nsgdb/doctrine key=md/L4/nsgdb/doctrine entities=doc/protocol,nsgdb/usage. Retrieve with recall(scope=nsgdb/doctrine, mode=lexical) or recall(entities=["doc/protocol"], scope=nsgdb/doctrine). Resource nsgdb://doctrine.

MCP lists 4 tools: remember, recall, health, curate. health(view=era) is era_report; health(view=tensions) is conflicts/unseen scopes. Default recall is lexical. Resource nsgdb://session is the cold-start packet. Legacy tool names still work if a client calls them.
