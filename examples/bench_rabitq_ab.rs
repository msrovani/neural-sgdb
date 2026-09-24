//! Bench A/B: RaBitQ (protótipo, `src/rabitq.rs`) vs ADC-lite (v1.1.17)
//! nos MESMOS corpora do `bench.rs` (8 clusters × 128 membros, 1024-dim,
//! 40 queries; + corpus espalhado uniforme). Critério de entrada no
//! ROADMAP §Auditoria dos projetos base: recall@5 do filtro ≥ +10 pp com
//! custo/query ≤ +10%. `set_proj_correction` alterna a variante do estimador.
//!
//! O dataset é gerado com o MESMO LCG/seeds do `bench.rs` (fases copiadas
//! literalmente) para os números serem comparáveis às tabelas publicadas.
//!
//! Rodar: `cargo run --release --example bench_rabitq_ab`

use std::collections::BTreeSet;
use std::time::Instant;

use neural_sgdb::bq::BqFlatIndex;
use neural_sgdb::rabitq;

const DIM: usize = 1024;
const VECS: usize = 1024;
const CLUSTERS: usize = 8;

fn main() {
    println!("=== Bench A/B: RaBitQ vs ADC-lite (mesmos corpora do bench.rs) ===\n");

    // ── corpora: copiado do bench.rs (clusters correlacionados + espalhado) ──
    let mut cvectors: Vec<Vec<f32>> = Vec::with_capacity(VECS);
    let mut centers: Vec<Vec<f32>> = Vec::with_capacity(CLUSTERS);
    for c in 0..CLUSTERS {
        let mut st = (c as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
        let mut center = vec![0f32; DIM];
        for x in center.iter_mut() {
            st = st.wrapping_mul(1103515245).wrapping_add(12345);
            *x = ((st >> 32) as i32 % 400) as f32 / 100.0 - 2.0;
        }
        centers.push(center);
    }
    for (c, center) in centers.iter().enumerate() {
        for i in 0..VECS / CLUSTERS {
            let mut v = center.clone();
            let mut st = (c * 100_003 + i) as u64;
            for x in v.iter_mut() {
                st = st.wrapping_mul(1103515245).wrapping_add(12345);
                *x += ((st >> 32) as i32 % 600) as f32 / 1000.0 - 0.3;
            }
            cvectors.push(v);
        }
    }
    // corpus espalhado (uniforme, para medir o custo em regime hostil)
    let mut spread: Vec<Vec<f32>> = Vec::with_capacity(VECS);
    let mut sst = 0xDEAD_BEEFu64;
    for _ in 0..VECS {
        let mut v = vec![0f32; DIM];
        for x in v.iter_mut() {
            sst = sst.wrapping_mul(1103515245).wrapping_add(12345);
            *x = ((sst >> 32) as i32 % 2000) as f32 / 1000.0 - 1.0;
        }
        spread.push(v);
    }

    for (name, vectors) in [("clusters densos", &cvectors), ("espalhado", &spread)] {
        println!("--- corpus: {name} ({} vetores, {DIM}-dim) ---", vectors.len());
        run_corpus(name, vectors);
        println!();
    }
}

fn run_corpus(name: &str, vectors: &[Vec<f32>]) {
    let n = vectors.len();
    // queries idem bench.rs: passo uniforme, 40 queries
    let queries: Vec<usize> = (0..n).step_by(n / 40).collect();

    // ── baseline ADC-lite: BQ flat + média do corpus + dual path ──────────
    let mut bq = BqFlatIndex::new();
    for (i, v) in vectors.iter().enumerate() {
        bq.insert_f32(i as u64, v);
    }
    let dim = DIM;
    let mut cmean = vec![0f64; dim];
    for v in vectors {
        for (d, x) in v.iter().enumerate() {
            cmean[d] += *x as f64;
        }
    }
    let cmean: Vec<f32> = cmean.iter().map(|s| (s / n as f64) as f32).collect();

    // ── RaBitQ: encode com o MESMO centroide ─────────────────────────────
    let t_enc = Instant::now();
    let rq_vecs: Vec<rabitq::RaBitQVec> =
        vectors.iter().map(|v| rabitq::encode(v, &cmean)).collect();
    let enc_us = t_enc.elapsed().as_nanos() as f64 / n as f64 / 1000.0;

    // ── top-5 exato por cosseno FP32 ──────────────────────────────────────
    let cosine = |a: &[f32], b: &[f32]| -> f64 {
        let mut dot = 0.0f64;
        let mut na = 0.0f64;
        let mut nb = 0.0f64;
        for d in 0..dim {
            dot += a[d] as f64 * b[d] as f64;
            na += a[d] as f64 * a[d] as f64;
            nb += b[d] as f64 * b[d] as f64;
        }
        1.0 - dot / (na * nb).sqrt().max(1e-12)
    };
    let exact: Vec<Vec<u64>> = queries
        .iter()
        .map(|&q| {
            let mut scored: Vec<(u64, f64)> = vectors
                .iter()
                .enumerate()
                .map(|(i, v)| (i as u64, cosine(&vectors[q], v)))
                .collect();
            scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            scored.iter().take(5).map(|(id, _)| *id).collect()
        })
        .collect();

    let total = queries.len() * 5;

    // ── A: ADC-lite dual-path, por oversample ─────────────────────────────
    for ov in [1usize, 4, 16] {
        let mut hit = 0usize;
        let t = Instant::now();
        for (i, &q) in queries.iter().enumerate() {
            let mut cand: BTreeSet<u64> = bq
                .top_k_f32(&vectors[q], 5 * ov)
                .into_iter()
                .map(|(id, _)| id)
                .collect();
            cand.extend(
                bq.top_k_f32_minus_mean(&vectors[q], &cmean, 5 * ov)
                    .into_iter()
                    .map(|(id, _)| id),
            );
            hit += exact[i].iter().filter(|id| cand.contains(id)).count();
        }
        let us = t.elapsed().as_nanos() as f64 / queries.len() as f64 / 1000.0;
        println!(
            "ADC-lite  ov={ov:<2}  recall@5={:>3.0}%  {us:>8.1} µs/query",
            hit as f64 * 100.0 / total as f64
        );
    }

    // ── B: RaBitQ, por oversample (2 variantes de correção) ───────────────
    for corr in [false, true] {
        rabitq::set_proj_correction(corr);
        let variant = if corr { "com proj" } else { "sem proj" };
        for ov in [1usize, 4, 16] {
            let mut hit = 0usize;
            let t = Instant::now();
            for (i, &q) in queries.iter().enumerate() {
                let rq = rabitq::prepare_query(&vectors[q], &cmean);
                let mut scored: Vec<(usize, f32)> = rq_vecs
                    .iter()
                    .enumerate()
                    .map(|(j, e)| (j, rabitq::estimate_cosine(&rq, e, dim)))
                    .collect();
                // top-(5×ov) por estimador desc — partial sort barato:
                scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
                scored.truncate(5 * ov);
                hit += exact[i]
                    .iter()
                    .filter(|id| scored.iter().any(|(j, _)| **id == *j as u64))
                    .count();
            }
            let us = t.elapsed().as_nanos() as f64 / queries.len() as f64 / 1000.0;
            println!(
                "RaBitQ ({variant}) ov={ov:<2}  recall@5={:>3.0}%  {us:>8.1} µs/query",
                hit as f64 * 100.0 / total as f64
            );
        }
    }
    rabitq::set_proj_correction(false);
    println!("encode RaBitQ: {enc_us:.2} µs/vetor (one-shot, não entra na query)");
    let _ = name;
}
