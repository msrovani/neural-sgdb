//! Agent doctrine — what the LLM above must know (not install troubleshooting).
//! Source file: `docs/doctrine.md` (keep identical; `include_str` at compile).

/// Canonical storage id (`md/L4/nsgdb/doctrine` + companion `md/L2/nsgdb/doctrine`).
pub const DOCTRINE_KEY: &str = "nsgdb/doctrine";
/// Scope so global recall never treats the manual as user/project evidence.
pub const DOCTRINE_SCOPE: &str = "nsgdb/doctrine";
/// 1-hop strings — same on write and `recall_entities`.
pub const DOCTRINE_ENTITIES: &[&str] = &["doc/protocol", "nsgdb/usage"];

/// Compact protocol injected on MCP `initialize.instructions` and seeded in the DB.
pub const DOCTRINE: &str = include_str!("../docs/doctrine.md");
