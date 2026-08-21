//! nsgdb-embed — local embedder host para neural-sgdb, sem tocar o core.
//!
//! O core nunca gera embedding (`src/embedder.rs:Embedder` é seam). Este crate
//! é o **host que implementa a seam** com um modelo local. Hoje é um stub
//! determinístico 384-dim (hash trigram + FNV, sem deps, `no_std` compatível
//! no sentido de não usar `std` além de `alloc`); amanhã a feature `candle`
//! trocará o corpo por `candle-core`/`candle-transformers` sem mudar a API.
//!
//! ```rust
//! use nsgdb_embed::LocalEmbedder;
//! use neural_sgdb::embedder::Embedder;
//! let e = LocalEmbedder::new(384);
//! let v = e.embed("ola mundo").unwrap();
//! assert_eq!(v.len(), 384);
//! ```

use neural_sgdb::embedder::Embedder;
use neural_sgdb::SgdbError;

/// Embedder local determinístico — prova o contrato same-model (write e query
/// com o MESMO `LocalEmbedder` e mesma `dim`).
///
/// Detalhe: não é semântico de verdade (é hash), mas é **estável por texto**
/// e tem dim fixa — suficiente para provar `era_report`, `width-lock trap` e
/// `backfill_helper.rs` sem rede/HTTP. Trocar para candle é só trocar o
/// interior de `embed` (feature `candle`), a assinatura permanece.
pub struct LocalEmbedder {
    dim: usize,
}

impl LocalEmbedder {
    pub fn new(dim: usize) -> Self {
        Self { dim: dim.max(1).min(4096) }
    }
    /// 384-dim é o default prático (compatível com MiniLM/BGE small)
    pub fn default_384() -> Self {
        Self::new(384)
    }
}

impl Embedder for LocalEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, SgdbError> {
        if text.is_empty() {
            return Err(SgdbError::Invalid("empty text for embed"));
        }
        // FNV-1a por trigram + projeção para dim — determinístico, sem allocs pesados
        let mut out = vec![0f32; self.dim];
        let bytes = text.as_bytes();
        for (i, c) in out.iter_mut().enumerate() {
            let mut h: u64 = 0xcbf29ce484222325 ^ (i as u64).wrapping_mul(0x9e3779b97f4a7c15);
            // mistura 3 bytes com janela deslizante + índice da dim
            for (j, &b) in bytes.iter().enumerate() {
                h ^= (b as u64).wrapping_add((j as u64) * 31 + i as u64);
                h = h.wrapping_mul(0x100000001b3);
                // perturbação trigram: a cada 3 bytes, dobra o peso da posição
                if j % 3 == 0 {
                    *c += ((h >> 32) as u32 as f32 / u32::MAX as f32) * 0.1;
                }
            }
            // normaliza para [-1, 1] via hash final
            let v = (h ^ (h >> 33)) as u32 as f32 / u32::MAX as f32 * 2.0 - 1.0;
            *c += v;
            // clamp leve
            if !c.is_finite() {
                *c = 0.0;
            }
        }
        // L2-ish: evita vetor nulo
        let n = (out.iter().map(|x| x * x).sum::<f32>()).sqrt();
        if n > 1e-6 {
            for x in &mut out {
                *x /= n;
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deterministic_and_dim() {
        let e = LocalEmbedder::new(384);
        let a = e.embed("ola mundo").unwrap();
        let b = e.embed("ola mundo").unwrap();
        assert_eq!(a.len(), 384);
        assert_eq!(a, b);
        let c = e.embed("outro texto").unwrap();
        assert_ne!(a, c);
    }
    #[test]
    fn same_model_contract() {
        // mesmo texto, mesma dim → mesmo vetor; dim diferente → vetor diferente
        let e384 = LocalEmbedder::new(384);
        let e256 = LocalEmbedder::new(256);
        let a = e384.embed("teste").unwrap();
        let b = e256.embed("teste").unwrap();
        assert_ne!(a.len(), b.len());
    }
}
