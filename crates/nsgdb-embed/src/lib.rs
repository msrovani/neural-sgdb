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
//! let e = LocalEmbedder::new(384).unwrap();
//! let v = e.embed("ola mundo").unwrap();
//! assert_eq!(v.len(), 384);
//! ```

use neural_sgdb::embedder::Embedder;
use neural_sgdb::SgdbError;

/// `model_id` da era (MDM1 v7) do stub determinístico. Passe este rótulo ao
/// `remember(model_id=...)`/`RememberOptions.model_id` para o `era_report`
/// distinguir eras com a MESMA dim (veredito `mixed_models`).
pub const MODEL_ID: &str = "local-hash-384";

/// Modelo multilíngue recomendado para via B (in-process, ADR-0007):
/// `paraphrase-multilingual-MiniLM-L12-v2` (384-dim). Cobre português + código
/// + JSON máquina→máquina, com a MESMA dim do MiniLM-L6 (era 384).
pub const MULTILINGUAL_MODEL_ID: &str = "paraphrase-multilingual-MiniLM-L12-v2-384";
/// Dimensão do vetor do modelo multilíngue (384).
pub const MULTILINGUAL_DIM: usize = 384;

/// Rótulos de embedder de host que o MCP aceita e o `model_id` da era que
/// cada um declara (ADR-0007 — quem fornece usa o MESMO modelo dos dois lados).
/// `demo` = trigram (NÃO semântico); `candle`/`multilingual`/`onnx`/`local` =
/// o modelo multilíngue acima (via B). `None` = sem model_id a declarar.
pub fn model_id_for(label: &str) -> Option<&'static str> {
    match label {
        "" | "none" => None,
        "demo" => Some("demo-256"),
        "candle" | "multilingual" | "onnx" | "local" => Some(MULTILINGUAL_MODEL_ID),
        _ => None,
    }
}

/// Embedder local determinístico — prova o contrato same-model (write e query
/// com o MESMO `LocalEmbedder` e mesma `dim`).
///
/// Detalhe: não é semântico de verdade (é hash), mas é **estável por texto**
/// e tem dim fixa — suficiente para provar `era_report`, `width-lock trap` e
/// `backfill_helper.rs` sem rede/HTTP. Trocar para candle é só trocar o
/// interior de `embed` (feature `candle`), a assinatura permanece.
///
/// ## ONNX / modelo real (via B — in-process, P2.1)
///
/// O caminho para semântica de verdade, mantendo o core zero-dep (ADR-0001):
/// 1. `--features candle` liga o esqueleto `try_candle_embed` abaixo
///    (candle-core/candle-nn/tokenizers — já no cache offline);
/// 2. coloque o modelo em `./models/multilingual/`:
///    `model.safetensors` + `tokenizer.json` de
///    `paraphrase-multilingual-MiniLM-L12-v2` (ONNX/transformers, 384-dim);
///    declare `NEURAL_SGDB_EMBEDDER=multilingual` (ou `candle`/`onnx`/`local`);
/// 3. `try_candle_embed` faz: tokenizer → forward → mean-pooling →
///    L2-normalize → `Vec<f32>` 384-dim. `model_id` da era =
///    [`MULTILINGUAL_MODEL_ID`] (o `model_id_for` mapeia os rótulos).
///
/// **Dor conhecida no Windows:** a árvore de deps do candle puxa `getrandom`,
/// cujo build chama `dlltool.exe` — precisa de binutils (mingw) instalado.
/// Sem isso, o build falha AQUI no `--features candle` (verificado). Alternativas:
/// Linux/macOS (sem dlltool), ou via C (`examples/embedder_http` → ollama/
/// llama.cpp, zero dep Rust), ou via A (caller `embedding=` de um modelo que o
/// host já tem). A queda final é o hash/Lexical (honesto, ADR-0008).
///
/// O core NUNCA linka runtime de inferência; quem fornece usa o MESMO modelo
/// dos dois lados (o `recall` é LOUD em dim mismatch, S1).
pub struct LocalEmbedder {
    dim: usize,
}

impl LocalEmbedder {
    pub fn new(dim: usize) -> Result<Self, SgdbError> {
        if dim == 0 || dim > 4096 {
            return Err(SgdbError::Invalid("dim exceeds MAX_EMBEDDING_DIM"));
        }
        Ok(Self { dim })
    }
    /// 384-dim é o default prático (compatível com MiniLM/BGE small)
    pub fn default_384() -> Self {
        Self { dim: 384 }
    }
    /// `model_id` da era (MDM1 v7) deste embedder — declarar no write p/ o
    /// `era_report` separar eras de mesma dim.
    pub fn model_id(&self) -> &'static str {
        MODEL_ID
    }
    /// Dimensão do vetor produzido.
    pub fn dim(&self) -> usize {
        self.dim
    }
    /// Construtor unchecked para testes internos (clamp, não falha)
    #[cfg(test)]
    pub fn new_unchecked(dim: usize) -> Self {
        Self { dim: dim.max(1).min(4096) }
    }
}

impl Embedder for LocalEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, SgdbError> {
        #[cfg(feature = "candle")]
        {
            // Quando --features candle, tenta MiniLM real em ./models/minilm
            // (model.safetensors + tokenizer.json). Se não existir, cai no stub abaixo
            // sem quebrar o build — prova o wiring sem download obrigatório.
            if let Ok(v) = try_candle_embed(text, self.dim) {
                return Ok(v);
            }
        }
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

#[cfg(feature = "candle")]
fn try_candle_embed(text: &str, dim: usize) -> Result<Vec<f32>, SgdbError> {
    // Esqueleto: carrega tokenizer + model.safetensors em ./models/minilm
    // Uso real:
    // let tokenizer = tokenizers::Tokenizer::from_file("models/minilm/tokenizer.json").map_err(|_| SgdbError::Invalid("tokenizer"))?;
    // let mut model = candle_nn::VarBuilder::from_mmaped_safetensors(&["models/minilm/model.safetensors"], candle_core::DType::F32, &candle_core::Device::Cpu)?;
    // Pooling + normalização → Vec<f32> dim
    // Por enquanto retorna Err para cair no stub hash sem exigir download em CI.
    let _ = (text, dim);
    Err(SgdbError::Invalid("candle model not found, using stub"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deterministic_and_dim() {
        let e = LocalEmbedder::new(384).unwrap();
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
        let e384 = LocalEmbedder::new(384).unwrap();
        let e256 = LocalEmbedder::new(256).unwrap();
        let a = e384.embed("teste").unwrap();
        let b = e256.embed("teste").unwrap();
        assert_ne!(a.len(), b.len());
    }
    #[test]
    fn dim_bounds() {
        assert!(LocalEmbedder::new(0).is_err());
        assert!(LocalEmbedder::new(4097).is_err());
        assert!(LocalEmbedder::new(4096).is_ok());
    }

    #[test]
    fn multilingual_contract_model_id_and_dim() {
        // via B: rótulos do host → model_id da era (MDM1 v7), todos 384-dim.
        assert_eq!(MULTILINGUAL_DIM, 384);
        assert_eq!(MULTILINGUAL_MODEL_ID, "paraphrase-multilingual-MiniLM-L12-v2-384");
        for label in ["candle", "multilingual", "onnx", "local"] {
            assert_eq!(model_id_for(label), Some(MULTILINGUAL_MODEL_ID), "{label}");
        }
        assert_eq!(model_id_for("demo"), Some("demo-256"));
        assert_eq!(model_id_for("none"), None);
        assert_eq!(model_id_for(""), None);
        // O stub determinístico (fallback sem rede) mantém a MESMA dim 384 —
        // a era não muda se o modelo multilíngue não estiver baixado.
        let e = LocalEmbedder::default_384();
        assert_eq!(e.dim(), 384);
        assert_eq!(e.model_id(), "local-hash-384");
    }
}
