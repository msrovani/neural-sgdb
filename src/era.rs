//! Model-era awareness (ADR-0007): detection, advisory plan and cost estimate
//! for embedding-model migration.
//!
//! The embedding model is an **era invariant per corpus** (ADR-0007): the S1
//! guard checks DIMENSIONS, not model identity, and `BqFlatIndex` locks
//! `words_per_vec` on the first insert — writing a different-dim embedding into
//! a live BQ silently truncates it (bughunt #11). This module gives the layer
//! above (the managing LLM) the MATERIAL to decide: a structured report of the
//! corpus era state and a cost estimate for the re-embed migration, computed by
//! applying the measured benchmark formula (BENCHMARKS.md §Era migration) to
//! the real record count.
//!
//! The core NEVER decides — it reports. `Sgdb::era_report()` builds the report
//! from the live engine state; this module holds the pure, `no_std`-safe
//! estimator and the report data types.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Companion-text read cost per doc (BENCHMARKS.md §Era migration, measured
/// on AMD Ryzen 7 5750G): NMD1 decode, no meta attach.
pub const READ_NS_PER_DOC: u64 = 1_100;
/// Rewrite cost per doc, P50 (FileStorage append + meta + index, 256→384 dims).
pub const REWRITE_NS_PER_DOC: u64 = 72_000;
/// Rebuild cost per doc amortized (51.7 ms / 4000 docs).
pub const REBUILD_NS_PER_DOC: u64 = 12_900;

/// Cost estimate for the re-embed era migration (ADR-0007).
#[derive(Clone, Debug)]
pub struct EraEstimate {
    /// Total embedding-declared docs (L4/L5) that must be re-embedded.
    pub docs_to_reembed: usize,
    /// Total bytes of preserved text (companions `/L2/`) to re-embed.
    pub text_bytes: usize,
    /// Estimated DB-side processing time (read + rewrite + rebuild), ns.
    pub db_side_ns: u128,
    /// The formula used — the LLM can recompute or plug model-side cost.
    pub formula: &'static str,
}

/// Applies the era-migration cost formula to the real record count.
///
/// DB-side estimate: `N × (read + rewrite + rebuild)` with the published
/// benchmark constants. Model-side cost (inference/API throughput, price) is
/// EXTERNAL — the caller (the managing LLM) owns the model and multiplies
/// `docs_to_reembed`/`text_bytes` by its own numbers.
pub fn estimate_era_migration(docs: usize, text_bytes: usize) -> EraEstimate {
    let n = docs as u128;
    let db_side_ns = n * READ_NS_PER_DOC as u128
        + n * REWRITE_NS_PER_DOC as u128
        + n * REBUILD_NS_PER_DOC as u128;
    EraEstimate {
        docs_to_reembed: docs,
        text_bytes,
        db_side_ns,
        formula: "db = N*(1.1us read + 72us rewrite + 12.9us rebuild) = N*86us; model cost external",
    }
}

/// Structured model-era report for the managing LLM (`Sgdb::era_report`).
#[derive(Clone, Debug)]
pub struct EraReport {
    /// Dimensionalities currently indexed (L4/L5 embedding-declared).
    pub indexed_dims: Vec<usize>,
    /// Record count per dimensionality, sorted by dim.
    pub docs_per_dim: Vec<(usize, usize)>,
    /// BQ width lock (words per vector).
    pub bq_words_per_vec: usize,
    /// Fraction of embedding-declared docs with preserved `/L2/` text.
    pub companion_coverage: f64,
    /// Total preserved text bytes.
    pub text_bytes: usize,
    /// `"empty"` (no embeddings yet — first write defines the era) |
    /// `"ok"` (single era) | `"mixed_dims"` (needs migration or a new base).
    pub verdict: &'static str,
    /// Migration plan (ADR-0007) when needed; empty when `ok`.
    pub plan: Vec<&'static str>,
    /// Re-embed migration cost estimate.
    pub estimate: EraEstimate,
}

/// Convenience for MCP/CLI formatting: one line per field.
pub fn era_report_lines(r: &EraReport) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!("verdict: {}", r.verdict));
    out.push(format!("indexed_dims: {:?}", r.indexed_dims));
    let per: Vec<String> = r
        .docs_per_dim
        .iter()
        .map(|(d, c)| format!("{d}dim={c}"))
        .collect();
    out.push(format!("docs_per_dim: [{}]", per.join(", ")));
    out.push(format!("bq_words_per_vec: {}", r.bq_words_per_vec));
    out.push(format!(
        "companion_coverage: {:.3} ({}/{} preserved)",
        r.companion_coverage,
        (r.companion_coverage * r.estimate.docs_to_reembed as f64 + 0.5) as usize,
        r.estimate.docs_to_reembed
    ));
    out.push(format!("text_bytes: {}", r.text_bytes));
    if !r.plan.is_empty() {
        out.push("plan:".into());
        for (i, p) in r.plan.iter().enumerate() {
            out.push(format!("  {}. {p}", i + 1));
        }
    }
    out.push(format!(
        "estimated db-side: {:.1} ms for {} docs (rewrite+rebuild, BENCHMARKS.md constants)",
        r.estimate.db_side_ns as f64 / 1e6,
        r.estimate.docs_to_reembed
    ));
    out.push(format!("formula: {}", r.estimate.formula));
    out.push(
        "model-side cost is EXTERNAL: multiply docs/text by your model throughput+price".into(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn estimate_scales_linearly_with_record_count() {
        let e1k = estimate_era_migration(1_000, 400_000);
        let e10k = estimate_era_migration(10_000, 4_000_000);
        // 10x docs ⇒ ~10x db-side estimate (formula is linear in N)
        let ratio = e10k.db_side_ns as f64 / e1k.db_side_ns as f64;
        assert!(
            (ratio - 10.0).abs() < 1e-6,
            "fórmula linear em N (ratio={ratio})"
        );
        assert_eq!(e10k.docs_to_reembed, 10_000);
        assert_eq!(e10k.text_bytes, 4_000_000);
        // sane magnitude: 10k docs ≈ 10k × 86µs ≈ 0.86 s
        assert!((e10k.db_side_ns as f64 / 1e9 - 0.86).abs() < 0.1);
    }

    #[test]
    fn estimate_reports_formula_and_counts() {
        let e = estimate_era_migration(48_231, 22_000_000);
        assert_eq!(e.docs_to_reembed, 48_231);
        assert_eq!(e.text_bytes, 22_000_000);
        assert!(e.formula.contains("86us"), "formula exposta: {}", e.formula);
        assert!(e.db_side_ns > 0);
    }

    #[test]
    fn era_report_lines_render_every_section() {
        let r = EraReport {
            indexed_dims: vec![256, 384],
            docs_per_dim: vec![(256, 40_000), (384, 8_231)],
            bq_words_per_vec: 4,
            companion_coverage: 0.97,
            text_bytes: 22_000_000,
            verdict: "mixed_dims",
            plan: vec!["re-embed text", "rewrite payload", "rebuild BQ"],
            estimate: estimate_era_migration(48_231, 22_000_000),
        };
        let lines = era_report_lines(&r);
        assert!(lines.iter().any(|l| l.starts_with("verdict: mixed_dims")));
        assert!(lines.iter().any(|l| l.contains("docs_per_dim: [256dim=40000, 384dim=8231]")));
        assert!(lines.iter().any(|l| l.starts_with("plan:")));
        assert!(lines.iter().any(|l| l.contains("estimated db-side:")));
        assert!(lines.iter().any(|l| l.contains("model-side cost is EXTERNAL")));
    }
}