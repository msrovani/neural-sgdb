//! Local embedder (v1.1.14 §4): implements `neural_sgdb::Embedder` out of the
//! lib (ADR-0001 zero-deps intact). Baseline is a deterministic 384d hash
//! (same contract as `DemoEmbedder`, new era dim); swap `embed()` for an
//! ONNX `ort/tract` `all-MiniLM-L6-v2` inference without touching the core.

extern crate alloc;
use alloc::vec::Vec;
use neural_sgdb::{Embedder, SgdbError};

/// Dimensão do embedder local (compat `all-MiniLM-L6-v2` 384d).
pub const LOCAL_EMBED_DIM: usize = 384;
/// `model_id` gravado em MDM1 v7 (`Sgdb::set_model_id`).
pub const LOCAL_MODEL_ID: &str = "local-hash-384";

/// Baseline local determinística (hash, sem rede/modelo). Troque por ONNX.
pub struct LocalEmbedder;

impl Embedder for LocalEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, SgdbError> {
        Ok(local_embed(text))
    }
}

/// Hash trigram FNV-1a → 384d normalizado (position-independent).
pub fn local_embed(text: &str) -> Vec<f32> {
    use alloc::vec;
    let mut v = vec![0f32; LOCAL_EMBED_DIM];
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return v;
    }
    let windows: Vec<&[u8]> = if bytes.len() < 3 {
        bytes.iter().map(core::slice::from_ref).collect()
    } else {
        bytes.windows(3).collect()
    };
    for w in windows {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for &b in w {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
        let idx = (h % LOCAL_EMBED_DIM as u64) as usize;
        v[idx] += if (h >> 8) & 1 == 1 { 1.0 } else { -1.0 };
    }
    let mut n = 0f32;
    for x in &v {
        n += x * x;
    }
    // Newton sqrt (no_std-safe, mesmo ponteiro do core).
    let mut g = if n > 0.0 { n } else { 1.0 };
    for _ in 0..12 {
        g = 0.5 * (g + n / g.max(1e-12));
    }
    let norm = g.max(1e-8);
    for x in v.iter_mut() {
        *x /= norm;
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_embed_deterministic_384() {
        let a = local_embed("gato no sofa");
        let b = local_embed("gato no sofa");
        assert_eq!(a.len(), 384);
        assert_eq!(a, b);
    }
}
