# CLAUDE.md — neural-sgdb

Guide for Claude Code working in this repo. **Read `AGENTS.md` (rules + quick
API), `codemap.md` (atlas), `docs/api.md` (contract) and
`docs/architecture/README.md` (system design) before editing code.**

## Repository Map

A full codemap is available at `codemap.md` in the project root.

Before working on any task, read `codemap.md` to understand:
- Project architecture and entry points
- Directory responsibilities and design patterns
- Data flow and integration points between modules

For deep work on a specific folder, also read that folder's `codemap.md`
(`src/codemap.md`, `examples/codemap.md`).

## Executive summary

Memory database for AI agents (**memories, not data**): L0–L7 layers, semantic
recall BQ + FP32 (no FAISS/HNSW), O(k) ART index, pluggable storage, CRDT p2p,
MCP server (4 tools, lexical-first ADR-0008). Zero deps, `no_std` + `std`,
MIT OR Apache-2.0. Interop with neural-os-core via byte-identical NMD1 and
TKLV formats.

## Essential rules (details in AGENTS.md)

1. **Zero deps in `[dependencies]`** — only `alloc`/`std`; dev-deps only for examples
2. **`cargo check --no-default-features --target x86_64-unknown-none`** must pass
3. **`f32::sqrt` does not exist in core** for x86_64-unknown-none — use `sqrt_f32`
4. **NMD1/TKLV formats are byte-identical contracts with the OS** — do not change layout
5. **Verification**: `cargo test` (+ `--features p2p`) and both `cargo check`

## Commands

```bash
cargo test                                   # 235+1 tests
cargo test --features p2p                    # 275+1
cargo test --no-default-features             # 181+1 (no_std core, host harness)
cargo run --release --example bench          # benchmarks
cargo run --release --example mcp_server     # MCP server (4 tools, lexical default)
cargo run --release --example mcp_client     # HOT TEST (84/0 checks)
cargo run --release --example two_ai_protocol # machine→machine contract (16/16)
cargo check --no-default-features --target x86_64-unknown-none
```
