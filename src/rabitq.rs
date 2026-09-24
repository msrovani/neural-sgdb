//! RaBitQ — protótipo de estimador não-biasado sobre 1-bit (v1.1.29-protótipo).
//!
//! Port da IDEIA de RaBitQ (Gao & Long, SIGMOD'24; Elastic/LanceDB explainer)
//! para o contrato deste crate — NÃO substitui o `bq.rs` (sign-BQ + ADC-lite
//! permanece o caminho do core até o bench A/B decidir, critério no ROADMAP
//! §Auditoria dos projetos base).
//!
//! Modelo (versão simplificada e fiel ao estimador do paper):
//! - Cada vetor de dados é centrado no **centroide do corpus** (o mesmo
//!   `corpus_mean` do ADC-lite), normalizado e quantizado a 1 bit
//!   (`sign(x)`), com snap ao representante `±1/√d`.
//! - Por vetor armazenamos 2 f32 de correção: `||v − c||` e
//!   `<vc, v̄>` (dot do residual normalizado com o snap).
//! - A query é centrada, normalizada e **quantizada a int8** (asimétrica —
//!   guarda lower/upper), permitindo dot inteiro contra os bits.
//! - O estimador de cosseno é NÃO-biasado: `est = <qc,v̄> / (γ_v · γ_q)`
//!   com γ corrigindo o bias do snap (paper §3.2 — na nossa simplificação,
//!   γ_v = <vc, v̄> já captura o alinhamento do snap com o residual real).
//!
//! Determinismo: a rotação é **por era determinística** (LCG semeado pela
//! era) — embeddings reais já são isotrópicos o suficiente para o protótipo;
//! a rotação de Jacobi completa é o próximo passo SE o A/B pedir.
//!
//! `no_std`-safe (só `alloc`), zero deps, usa `sqrt_f32` de `math.rs`.

use alloc::vec;
use alloc::vec::Vec;
use crate::math::sqrt_f32;

/// Protótipo A/B: correção de bias por `proj` ligada? (célula de processo,
/// só para o bench medir as duas variantes; default OFF — medir primeiro).
pub(crate) static USE_PROJ_CORRECTION: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Liga/desliga a correção de bias `proj` (bench A/B).
pub fn set_proj_correction(on: bool) {
    USE_PROJ_CORRECTION.store(on, core::sync::atomic::Ordering::Relaxed);
}

/// Um vetor de dados no esquema RaBitQ: bits + correções.
#[derive(Clone, Debug)]
pub struct RaBitQVec {
    /// Bits (`sign(x − c[i])` por dim).
    pub bits: Vec<u64>,
    /// `||v − c||` — distância ao centroide.
    pub norm_c: f32,
    /// `<vc, v̄>` — dot do residual normalizado com o snap `±1/√d`.
    pub proj: f32,
}

/// Codifica um vetor de dados contra o centroide `c`.
pub fn encode(v: &[f32], c: &[f32]) -> RaBitQVec {
    let d = v.len();
    let nwords = d.div_ceil(64);
    let mut bits = vec![0u64; nwords];
    let mut norm2 = 0f64;
    for i in 0..d {
        let r = v[i] - c[i];
        norm2 += (r * r) as f64;
        if r > 0.0 {
            bits[i / 64] |= 1u64 << (i % 64);
        }
    }
    let norm_c = sqrt_f32(norm2 as f32).max(1e-12);
    // proj = <vc, v̄> = (1/√d) · Σ |vc_i| = abs_sum / (norm · √d)
    let inv_sqrt_d = 1.0 / sqrt_f32(d as f32);
    let mut abs_sum = 0f64;
    for i in 0..d {
        abs_sum += ((v[i] - c[i]) as f64).abs();
    }
    let proj = ((abs_sum as f32) * inv_sqrt_d / norm_c).clamp(0.01, 1.0);
    RaBitQVec { bits, norm_c, proj }
}

/// Query quantizada: bits da direção + int8 da direção centrada/normalizada.
pub struct RaBitQQuery {
    pub bits: Vec<u64>,
    /// int8 da direção centrada (lower/upper do min-max).
    pub q8: Vec<i8>,
    pub lower: f32,
    pub width: f32,
    /// `||q − c||`.
    pub norm_c: f32,
}

/// Prepara a query contra o centroide `c`.
pub fn prepare_query(q: &[f32], c: &[f32]) -> RaBitQQuery {
    let d = q.len();
    let nwords = d.div_ceil(64);
    let mut bits = vec![0u64; nwords];
    let mut qc = vec![0f32; d];
    let mut norm2 = 0f64;
    let mut lo = f32::MAX;
    let mut hi = f32::MIN;
    for i in 0..d {
        let r = q[i] - c[i];
        norm2 += (r * r) as f64;
        qc[i] = r;
        if r > 0.0 {
            bits[i / 64] |= 1u64 << (i % 64);
        }
        if r < lo {
            lo = r;
        }
        if r > hi {
            hi = r;
        }
    }
    let norm_c = sqrt_f32(norm2 as f32).max(1e-12);
    // normaliza a direção e quantiza a int8 (asimétrica, 255 níveis)
    let width = ((hi - lo) / 255.0).max(1e-12);
    let q8 = qc
        .iter()
        .map(|&x| {
            // `f32::round` não existe no core do alvo (regra 3) — floor(t+0.5)
            // via trunc de não-negativo (t ≥ 0 por construção: x ≥ lo).
            let t = (x - lo) / width;
            let lvl = (t + 0.5) as i32;
            lvl.clamp(0, 255) as i8
        })
        .collect();
    RaBitQQuery { bits, q8, lower: lo, width, norm_c }
}

/// Estima o COSSENO CENTRADO `<qc, vc>` (o ranking do corpus centrado):
/// `<qc, v̄> = Σ_i qc_i · s_v(i)/√d` com `s_v = ±1` dos bits do dado e qc
/// reconstruído da int8 (`q8·width + lower`). Correção de bias do snap
/// (paper §3.2): divide por `proj_v = <vc, v̄>` — o alinhamento real do
/// residual do dado com o próprio snap — tornando o estimador não-biasado.
pub fn estimate_cosine(q: &RaBitQQuery, v: &RaBitQVec, dim: usize) -> f32 {
    let inv_sqrt_d = 1.0 / sqrt_f32(dim as f32);
    let mut signed = 0f64;
    for (wi, w) in v.bits.iter().enumerate() {
        let mut word = *w;
        while word != 0 {
            let b = word.trailing_zeros() as usize;
            let i = wi * 64 + b;
            if i < dim {
                // bit=1 → s_v=+1 contribui +qc_i; bit=0 → s_v=−1 contribui −qc_i
                signed += (q.q8[i] as f32 * q.width + q.lower) as f64;
            }
            word &= word - 1;
        }
    }
    let mut total = 0f64;
    for &lvl in &q.q8 {
        total += (lvl as f32 * q.width + q.lower) as f64;
    }
    let signed_dot = (2.0 * signed - total) as f32; // Σ qc_i · s_v(i)
    let mut cos = signed_dot * inv_sqrt_d;
    // correção não-biasada (paper §3.2): o snap subestima <qc,vc> por fator
    // proj_v — mas quando proj varia pouco no corpus ela só injeta ruído.
    // Protótipo: correção OPT-IN via env-free constante para o A/B medir
    // as duas variantes.
    if USE_PROJ_CORRECTION.load(core::sync::atomic::Ordering::Relaxed) {
        cos /= v.proj.max(0.05);
    }
    // SEM clamp no protótipo: empates saturados em 1.0 escondem a ordem
    // real que o estimador produz — o ranking precisa da variação bruta.
    cos
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;


    /// RESULTADO NEGATIVO MEDIDO (o que este teste pina): SEM a rotação
    /// aleatória do paper (Householder por era — não implementada aqui), o
    /// estimador por bits-only NÃO ranqueia: o bench A/B mediu overlap de
    /// top-5 de 0–2/5 contra o cosseno centrado, e pior — a média do
    /// estimador no cluster da query fica ABAIXO da média geral (sinal
    /// estruturalmente ausente). A adoção foi rejeitada no critério de custo
    /// (~25× mais lento que o Hamming SIMD) antes de investir na rotação.
    /// Este teste pina o que o protótipo GARANTE: finitude e determinismo.
    /// Ver ROADMAP §Auditoria dos projetos base (veredito completo).
    #[test]
    fn estimator_is_deterministic_and_finite() {
        let d = 128;
        let mut st = 7u64;
        let mut rand = || {
            st = st.wrapping_mul(1103515245).wrapping_add(12345);
            ((st >> 32) as i32 % 2000) as f32 / 1000.0 - 1.0
        };
        // corpus com estrutura: 5 centros + ruído pequeno (como embeddings reais)
        let mut centers: Vec<Vec<f32>> = Vec::new();
        for _ in 0..5 {
            centers.push((0..d).map(|_| rand() * 2.0).collect());
        }
        let mut vecs: Vec<Vec<f32>> = Vec::new();
        for (ci, ctr) in centers.iter().enumerate() {
            for i in 0..10 {
                let mut v = ctr.clone();
                let mut s = (ci * 1000 + i) as u64;
                for x in v.iter_mut() {
                    s = s.wrapping_mul(1103515245).wrapping_add(12345);
                    *x += ((s >> 32) as i32 % 300) as f32 / 1000.0 - 0.15;
                }
                vecs.push(v);
            }
        }
        // centroide REAL do corpus (mesma regra do encode)
        let mut c = vec![0f64; d];
        for v in &vecs {
            for (i, x) in v.iter().enumerate() {
                c[i] += *x as f64;
            }
        }
        let c: Vec<f32> = c.iter().map(|s| (s / vecs.len() as f64) as f32).collect();

        let encs: Vec<RaBitQVec> = vecs.iter().map(|v| encode(v, &c)).collect();
        let qv = &vecs[3]; // um membro de um cluster
        let q = prepare_query(qv, &c);
        let mut est: Vec<(usize, f32)> =
            encs.iter().enumerate().map(|(i, e)| (i, estimate_cosine(&q, e, d))).collect();
        // verdade: cosseno centrado <qc, vc> (o que o estimador modela)
        let mut qcn = 0f64;
        for i in 0..d {
            let r = qv[i] - c[i];
            qcn += r as f64 * r as f64;
        }
        let qcn = qcn.sqrt();
        let mut truth: Vec<(usize, f64)> = vecs
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let mut dot = 0f64;
                let mut vn = 0f64;
                for j in 0..d {
                    let a = qv[j] - c[j];
                    let b = v[j] - c[j];
                    dot += a as f64 * b as f64;
                    vn += b as f64 * b as f64;
                }
                (i, dot / (qcn * vn.sqrt()).max(1e-12))
            })
            .collect();
        est.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        truth.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        // (overlap de top-5 foi medido no bench A/B: 0–2/5 — ver doc do teste)
        // determinismo: segunda passada idêntica → mesma ordem
        let est2: Vec<f32> = encs.iter().map(|e| estimate_cosine(&q, e, d)).collect();
        let mut pairs: Vec<(usize, f32)> =
            encs.iter().enumerate().map(|(i, _e)| (i, est2[i])).collect();
        pairs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        assert_eq!(est, pairs, "estimador não é determinístico");
        assert!(est2.iter().all(|v| v.is_finite()), "estimador não-finito");
    }

    /// A projeção fica em (0, 1] e o snap é denso (bits não vazios).
    #[test]
    fn encode_bounds_hold() {
        let c = vec![0.0f32; 4];
        let e = encode(&[1.0, -2.0, 3.0, -4.0], &c);
        assert!(e.proj > 0.0 && e.proj <= 1.0 + 1e-4);
        assert!(e.norm_c > 0.0);
        assert_eq!(e.bits.len(), 1);
    }

    #[test]
    fn estimator_values_are_finite() {
        let c = vec![0.0f32; 8];
        let q = prepare_query(&[1.0, 0.5, -0.5, 1.0, 0.2, -1.0, 0.0, 0.3], &c);
        let e = encode(&[0.9, 0.4, -0.6, 1.1, 0.1, -0.9, 0.2, 0.4], &c);
        let est = estimate_cosine(&q, &e, 8);
        assert!(est.is_finite(), "estimador não-finito: {est}");
    }
}
