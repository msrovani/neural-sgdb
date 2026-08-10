//! Benchmarks publicados (roadmap item 4) — zero-dep, host (`std`).
//!
//! Rode com: `cargo run --release --example bench`
//!
//! Mede:
//! - ART: insert/get/scan_prefix latency (P50/P99)
//! - BQ: recall top-k latency (P50/P99)
//! - Recall BQ vs FP32 puro: recall@k (fração dos verdadeiros top-k FP32
//!   encontrados pelo BQ) — o trade-off da quantização binária, medido.

use std::time::{Duration, Instant};

use neural_sgdb::art::ArtIndex;
use neural_sgdb::bq::BqFlatIndex;
use neural_sgdb::hamming_dispatch::{select_best_hamming_kernel, path_name};
use neural_sgdb::{InMemory, Sgdb};

/// P50/P99 de um conjunto de amostras (já preenchido, ordenado na cópia).
fn percentiles(mut samples: Vec<Duration>) -> (Duration, Duration) {
    samples.sort();
    let p = |q: f64| {
        let idx = ((samples.len() as f64) * q) as usize;
        let idx = idx.min(samples.len().saturating_sub(1));
        samples[idx]
    };
    (p(0.50), p(0.99))
}

fn main() {
    select_best_hamming_kernel();
    println!("neural-sgdb bench — SIMD kernel: {}", path_name());

    // ── ART: 100k inserts + gets ────────────────────────────────────────────
    const N: usize = 100_000;
    let mut art = ArtIndex::new();
    let mut t_insert = Vec::with_capacity(N);
    for i in 0..N {
        let key = format!("md/L2/{:016x}", i);
        let t0 = Instant::now();
        art.insert(&key, i as u64);
        t_insert.push(t0.elapsed());
    }
    let (p50, p99) = percentiles(t_insert);
    println!("ART insert  {N} keys : P50={:?} P99={:?} len={}", p50, p99, art.len);

    let mut t_get = Vec::with_capacity(N);
    let mut hits = 0usize;
    for i in 0..N {
        let key = format!("md/L2/{:016x}", i);
        let t0 = Instant::now();
        if art.get(&key).is_some() {
            hits += 1;
        }
        t_get.push(t0.elapsed());
    }
    let (p50, p99) = percentiles(t_get);
    println!("ART get     {N} keys : P50={:?} P99={:?} hits={hits}/100000", p50, p99);

    // ── BQ: 10k vetores × 1024 dims, recall top-5 ───────────────────────────
    const VECS: usize = 10_000;
    const DIM: usize = 1024;
    let mut bq = BqFlatIndex::new();
    for i in 0..VECS {
        let mut v = vec![0f32; DIM];
        // vetores pseudo-aleatórios determinísticos (LCG — paridade host)
        let mut state = (i as u64).wrapping_mul(1103515245).wrapping_add(12345);
        for x in v.iter_mut() {
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            *x = ((state >> 32) as i32 % 200) as f32 / 100.0 - 1.0;
        }
        bq.insert_f32(i as u64, &v);
    }
    let query: Vec<f32> = {
        let mut q = vec![0f32; DIM];
        let mut state = 0xDEADBEEFu64;
        for x in q.iter_mut() {
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            *x = ((state >> 32) as i32 % 200) as f32 / 100.0 - 1.0;
        }
        q
    };
    let t0 = Instant::now();
    for _ in 0..100 {
        let r = bq.top_k_f32(&query, 5);
        assert!(!r.is_empty());
    }
    let avg = t0.elapsed() / 100;
    println!("BQ top-5    {VECS} vec x {DIM} dims : {avg:?} avg/query (kernel={})", path_name());

    // ── Recall BQ vs FP32: recall@5 ────────────────────────────────────────
    // Baseline HONESTO: cosseno FP32 real sobre os vetores f32 originais
    // (não hamming sobre os mesmos bits quantizados — aquilo é tautológico).
    let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(VECS);
    for i in 0..VECS {
        let mut v = vec![0f32; DIM];
        let mut state = (i as u64).wrapping_mul(1103515245).wrapping_add(12345);
        for x in v.iter_mut() {
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            *x = ((state >> 32) as i32 % 200) as f32 / 100.0 - 1.0;
        }
        vectors.push(v);
    }
    // top-5 exato por cosseno FP32
    let mut fp32_scores: Vec<(u64, f64)> = Vec::with_capacity(VECS);
    for (i, v) in vectors.iter().enumerate() {
        let mut dot = 0.0f64;
        let mut nq = 0.0f64;
        let mut nv = 0.0f64;
        for d in 0..DIM {
            dot += query[d] as f64 * v[d] as f64;
            nq += query[d] as f64 * query[d] as f64;
            nv += v[d] as f64 * v[d] as f64;
        }
        let denom = (nq * nv).sqrt().max(1e-12);
        let cos = dot / denom;
        fp32_scores.push((i as u64, 1.0 - cos));
    }
    fp32_scores.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let exact_top5: Vec<u64> = fp32_scores.iter().take(5).map(|(id, _)| *id).collect();
    let bq_top5: Vec<u64> = bq.top_k_f32(&query, 5).into_iter().map(|(id, _)| id).collect();
    let hit = exact_top5.iter().filter(|id| bq_top5.contains(id)).count();
    let recall = hit as f64 / 5.0;
    println!(
        "recall@5    BQ vs FP32-cosine-exact: {:.0}% ({hit}/5 top-5 coincidem; trade-off real da quantização 1-bit)",
        recall * 100.0
    );

    // ── End-to-end Sgdb (demo do uso real) ─────────────────────────────────
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    let t0 = Instant::now();
    for i in 0..1000 {
        db.remember_exchange(&format!("user msg {i}"), &format!("ai resp {i}"))
            .unwrap();
    }
    let e2e = t0.elapsed();
    println!("Sgdb 1k exchanges: {e2e:?} total");

    println!("\nBench completo. Kernel SIMD: {}", path_name());
}
