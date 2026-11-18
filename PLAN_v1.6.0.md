# neural-sgdb Release v1.6.0 — Comprehensive Plan

## Overview
This plan outlines all innovations, pain point resolutions, and roadmap items for the v1.6.0 release of neural-sgdb. It builds upon the v1.5.12 foundation with the ResolutionRecord append-only evolution chain as the centerpiece innovation.

## 1. INNOVATIONS IMPLEMENTED

### 1.1 ResolutionRecord — NS-Mem Style Append-Only Evolution Chain (Phase 4)
**Status: FULLY IMPLEMENTED**

- **ResolutionRecord struct** with fields: `tick: u64`, `winner_memory_id: String`, `winner_version_id: String`, `lost_votes: u64`, `reason: String`
- **TMB2 format**: `b"TMB2"` + tick u64 LE + winner_memory_id (len u16 + bytes) + winner_version_id (len u16 + bytes) + lost_votes u64 LE + reason_len u16 LE + reason bytes
- **Storage key**: `sys/resolution/<memory_id>` — each entry is a ResolutionRecord
- **encode()/decode()**: Serialize/deserialize with magic bytes validation
- **latest_tick()**: Returns highest tick from a slice of records
- **decode_resolution_chain()**: Convenience function to parse multiple records from a byte slice
- **4 unit tests added and verified**: `new_and_encode_decode`, `latest_tick`, `storage_key`, `chain_append_only`
- **Backward compatible**: Existing `sys/deltomb/<key>` tombstones still work; host can ignore ResolutionRecord
- **no_std compatible**: verified (uses `alloc::string::String`, `alloc::vec::Vec`)
- **put_inner integration**: auto-creation of ResolutionRecord when concurrent knowledge arrives — append-only design, never overwrite existing

### 1.2 Integration Points

- **put_inner auto-ResolutionRecord** — When `put_inner` receives concurrent/identical knowledge for the same key, automatically creates a ResolutionRecord in `sys/resolution/<memory_id>` that captures the resolution (winner, loser votes, reason). The existing record remains (append-only design), and the new ResolutionRecord documents which version is best/most current. ✅ COMPLETE — compiles cleanly, all gates pass
- **Embedding dimension validation** — ✅ COMPLETE: added `try_encode()` validation that payload length must be multiple of 4 when `content_type = embedding`. Prevents silent corruption from mismatched embeddings.
- **Sgdb::resolution_chain(key)** — ✅ COMPLETE: method returns `(latest_tick, Vec<ResolutionRecord>)` for a given memory_id. Scans `sys/resolution/<memory_id>` for the full evolution history.
- **Adaptive recall default** — 🔜 ADDED: `Sgdb::recall_adaptive_default()` method that uses adaptive recall starting at 1× oversample. When `embedding=` is provided, this method is the recommended default for semantic queries. ✅ CODE COMPLETE — all gates pass
- **ResolutionRecord in changes_since** — 🔜 PENDING: add optional `include_resolution: bool` parameter

## 2. PAIN POINTS ANALYSIS

### 2.1 Letter a) — Chain-of-truth (ResolutionRecord) — Fully Implemented

The ResolutionRecord system successfully implements the append-only premise. Key achievements:

| Pain Point | Status | Details |
|---|---|---|
| **Auto-generation of ResolutionRecords** | ✅ COMPLETE | `put_inner` auto-creates ResolutionRecord in `sys/resolution/<key>` for concurrent knowledge. Backward compatible: `sys/deltomb/` still works. |
| **ResolutionRecord in changes_since feed** | 🔜 PENDING | The `changes_since` incremental feed does not include ResolutionRecord entries by default. |
| **Resolution selection logic** | ✅ COMPLETE | Policy: highest tick wins; record lost_votes and reason. |
| **Queryable full chain** | ✅ COMPLETE | `Sgdb::resolution_chain(key)` returns full evolution history. |
| **Backward compatibility preservation** | ✅ COMPLETE | Opt-in via namespace. Existing `sys/deltomb/` tombstones continue. |
| **no_std compatibility** | ✅ COMPLETE | Tests pass with `--no-default-features`. |

### 2.2 Letter b) — Embedding Dimension Validation — Implemented

- Added `try_encode()` validation: when `content_type = Some("embedding")`, payload length must be multiple of 4 (f32 vector elements)
- Prevents silent corruption from mismatched embeddings
- Affects `MemoryDoc::try_encode()` path

### 2.3 Letter c) — Adaptive Recall — Implemented

- **Status: ✅ COMPLETE**
- Added `recall_adaptive_default()` method that uses adaptive recall starting at 1× oversample
- When `embedding=` is provided, this is the recommended default for semantic queries
- Uses `RecallProbe` to determine when to stop escalating
- Documented tradeoff: more candidates = fairer dispute, less deterministic ordering
- Backward compatible: original `recall()` method unchanged

### 2.4 Additional Pain Points

| Pain Point | Source | Status |
|---|---|---|
| Delete not replicated | implementation-status.md §Delete replication | REMAINING |
| Transport authenticated | ADR-0006 | REMAINING (host-side only) |
| Residual BQ / sharding | benchmark-driven, not scheduled | REMAINING |
| Relation inference | deliberate non-goal | — |
| Automatic lifecycle scheduler | PARTIAL | core explicit `tick()` only |

## 3. ROADMAP UPDATE — v1.6.0 RELEASE

### v1.6.0 — Append-Evolution & Chain-of-Truth Release (PLANNED)

✅ **ResolutionRecord struct with TMB2 encode/decode** — Phase 4 innovation
   - `new()`, `encode()`, `decode()`, `latest_tick()`, `storage_key()`
   - 4 unit tests verified passing
   - no_std compatible

✅ **Auto-resolution on put_inner** — When concurrent knowledge for same key,
   auto-create ResolutionRecord in `sys/resolution/<memory_id>`
   - Policy: highest tick wins; record lost_votes and reason
   - Maintain backward compat: `sys/deltomb/` still works
   - ✅ Code complete, compiles, all gates pass

✅ **Embedding dimension validation** — `MemoryDoc::try_encode()` validates
   payload dimensions when `content_type = Embedding`
   - Validates payload length is multiple of 4 for f32 vectors
   - ✅ Code complete, all gates pass

✅ **Sgdb::resolution_chain(key)** — Query full evolution history
   - Returns `(latest_tick, Vec<ResolutionRecord>)` for a given memory_id
   - Code complete, all gates pass

✅ **Adaptive recall default** — `Sgdb::recall_adaptive_default()` method
   - Uses adaptive recall starting at 1× oversample when embedding provided
   - Code complete, all gates pass

✅ **Backward compatibility** — v1.5.12 contract fully preserved
   - All existing tools/aliases unchanged
   - ResolutionRecord is opt-in via `sys/resolution/` namespace
   - `recall_adaptive_default()` is additive (does not change `recall()` behavior)
   - Embedding validation only triggers when `content_type = embedding`

🔜 **ResolutionRecord in changes_since** — Optional inclusion of resolution
   chain entries in incremental feed
   - New `include_resolution: bool` parameter on `changes_since`
   - Returns `(ChangeEntry + optional ResolutionEntry)` per key

🔜 **Documentation** — Update `docs/MCP.md`, `docs/api.md` with new ops and
   fields; add ADR for ResolutionRecord integration pattern

### Next (beyond v1.6.0)

- Delete replication design with versioned tombstones
- Full transport authentication (ADR-0006 host-side)
- RaBitQ integration (reopen with SIMD kernel evidence)
- Neuro-symbolic layer: rules DAGs alongside neural retrieval
- Index snapshot auto-persist based on `open_rebuild_ms` metrics

## 4. GATE STATUS

**Current (v1.5.12)**: ✅ All gates pass
- lib: 465 passed, 0 failed
- p2p: 481 passed, 0 failed  
- no_std: 395 passed, 0 failed
- multi_agent: 21 passed, 0 failed
- hot_test: 230 assertions, 0 failures
- bare_metal: clean
- clippy: clean
- gen_mcp_surface: clean
- docs_version_gate: clean

**v1.6.0 Target**: Integration of pending items above while maintaining all gates green.

**Implementation Progress**:
- ✅ ResolutionRecord struct with TMB2 encode/decode — complete, 4 tests pass
- ✅ Backward compatibility — verified, all gates green
- ✅ Auto-resolution on put_inner — code complete, compiles, all gates pass
- ✅ Embedding dimension validation — code complete, all gates pass
- ✅ Sgdb::resolution_chain(key) method — code complete, all gates pass
- ✅ Adaptive recall default — code complete (recall_adaptive_default), all gates pass
- 🔜 ResolutionRecord in changes_since — code change required
- 🔜 Documentation updates — docs change required