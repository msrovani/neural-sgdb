//! Pluggable text→embedding seam (AUDIT v1.1 P4, ADR-0008).
//!
//! The core NEVER generates embeddings — `remember_semantic`/`recall` take
//! `&[f32]` from the caller (layer above owns the model). This module gives
//! that layer a small, `no_std`-safe, zero-dep trait.
//!
//! ADR-0008: product default retrieval is **lexical**. `DemoEmbedder` (trigram
//! hash) is for tests / explicit `NEURAL_SGDB_EMBEDDER=demo`, not implied
//! semantics. Real cosine recall is a host HTTP client to a local model
//! server — never linked into this crate.

use alloc::vec;
use alloc::vec::Vec;

/// Converts text into a normalized embedding vector.
///
/// Implementations must be deterministic for the same input (the caller may
/// cache/persist them) and must not panic on any input. Errors → `SgdbError`.
pub trait Embedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, crate::SgdbError>;
}

/// Shipped default: deterministic character-trigram hash → normalized
/// 256-dim vector. Position-independent (each trigram always lands in the
/// same bin). Good for short-text keyword recall; NOT a semantic model —
/// plug a real one via the trait.
pub struct DemoEmbedder;

impl Embedder for DemoEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, crate::SgdbError> {
        Ok(demo_embed(text))
    }
}

/// Pure function behind [`DemoEmbedder`] (also usable directly).
///
/// HOT-TEST FIX (2026-08-13): o seed era position-dependent (`seed` mutado a
/// cada janela) — o mesmo trigrama em posições diferentes caía em bins
/// diferentes e o recall de palavras-chave falhava (query "integridade banco"
/// vs doc "...integridade do banco" → d≈1.0). Agora o hash é position-
/// independent: cada trigrama cai SEMPRE na mesma bin.
pub fn demo_embed(text: &str) -> Vec<f32> {
    const DIM: usize = 256;
    let mut v = vec![0f32; DIM];
    let bytes = text.as_bytes();
    // text < 3 bytes: no trigrams → degenerate zero vector; fallback by
    // individual bytes (fix #10)
    let windows: Vec<&[u8]> = if bytes.len() < 3 {
        bytes.iter().map(core::slice::from_ref).collect()
    } else {
        bytes.windows(3).collect()
    };
    for w in windows {
        // FNV-1a sobre o n-grama (sem seed posicional — HOT-TEST FIX)
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for &b in w {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
        let idx = (h % DIM as u64) as usize;
        v[idx] += if (h >> 8) & 1 == 1 { 1.0 } else { -1.0 };
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>();
    let norm = crate::sgdb::sqrt_f32(norm).max(1e-8);
    v.iter_mut().for_each(|x| *x /= norm);
    v
}

/// Dimensão do [`DemoEmbedder`] / [`demo_embed`] (256 — position-independent trigram).
pub const DEMO_EMBED_DIM: usize = 256;

/// Nota de contrato — o demo NÃO é modelo semântico; dims devem casar na era (ADR-0007).
pub const DEMO_EMBED_NOTE: &str =
    "DemoEmbedder (256-dim trigram hash) is NOT a semantic model; use the same dims on write and query.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_embed_deterministic_and_normalized() {
        let a = demo_embed("integridade banco de dados");
        let b = demo_embed("integridade banco de dados");
        assert_eq!(a, b, "mesmo texto → mesmo embedding");
        let n = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((n - 1.0).abs() < 1e-5, "normalizado: {n}");
    }

    #[test]
    fn demo_embed_position_independent_keywords() {
        // HOT-TEST FIX: o mesmo trigrama em posições diferentes cai na MESMA
        // bin — recall de palavras-chave funciona (d pequeno).
        let a = demo_embed("integridade do banco");
        let b = demo_embed("banco de dados integridade");
        let dot: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum::<f32>();
        let d = 1.0 - dot.clamp(-1.0, 1.0);
        assert!(d < 0.35, "trigramas compartilhados → distância pequena (d={d})");
    }

    #[test]
    fn demo_embed_short_and_hostile_inputs_do_not_panic() {
        for t in ["", "ab", "a", " ", "🙂", "\u{0}", "xy\nz"] {
            let v = demo_embed(t);
            // textos vazios produzem vetor zero (sem trigramas) — não panic;
            // os demais devem sair normalizados e finitos
            if t.is_empty() {
                assert!(v.iter().all(|x| *x == 0.0), "texto vazio → vetor zero");
                continue;
            }
            let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!(n.is_finite() && n > 0.0, "input {t:?}: finito normalizado");
        }
    }

    #[test]
    fn demo_embedder_trait_roundtrip() {
        let e = DemoEmbedder;
        let v = e.embed("alvo").unwrap();
        assert_eq!(v, demo_embed("alvo"));
    }
}