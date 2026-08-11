//! Published benchmarks (roadmap item 4) — zero-dep, host (`std`).
//!
//! Run with: `cargo run --release --example bench`
//!
//! Measures:
//! - ART: insert/get/scan_prefix latency (P50/P99)
//! - BQ: recall top-k latency (P50/P99)
//! - Recall BQ vs pure FP32: recall@k (fraction of the true FP32 top-k
//!   found by BQ) — the binary quantization trade-off, measured.

use std::time::{Duration, Instant};

use neural_sgdb::art::ArtIndex;
use neural_sgdb::bq::BqFlatIndex;
use neural_sgdb::hamming_dispatch::{select_best_hamming_kernel, path_name};
use neural_sgdb::storage::crc32;
use neural_sgdb::{InMemory, Sgdb, TickvFile};
use neural_sgdb::Storage as _;

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

    // ── Before/after: bounded heap (k=5) vs full sort (k=N) ──────────────
    // Maturation P3: top_k usa bounded max-heap (O(N·D/64 + N log k)) em vez
    // de full sort (O(N·D/64 + N log N)). Mede os dois caminhos no mesmo index.
    let t_heap = Instant::now();
    for _ in 0..100 {
        let r = bq.top_k_f32(&query, 5);
        assert!(!r.is_empty());
    }
    let heap_avg = t_heap.elapsed() / 100;
    let t_full = Instant::now();
    for _ in 0..100 {
        let r = bq.top_k_f32(&query, VECS); // k >= len → full sort path
        assert_eq!(r.len(), VECS);
    }
    let full_avg = t_full.elapsed() / 100;
    println!("BQ top-k    heap(k=5)={heap_avg:?} vs full-sort(k=N)={full_avg:?} — bounded heap evita o O(N log N) do ranking");

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

    // ── CRC32: throughput (custo de todo put + recovery de storage) ─────────
    let buf: Vec<u8> = (0..1024 * 1024).map(|i| (i % 251) as u8).collect();
    let t_crc = Instant::now();
    let mut acc = 0u32;
    for _ in 0..100 {
        acc ^= crc32(&buf);
    }
    let per = t_crc.elapsed() / 100;
    println!(
        "crc32   1MiB x100       : {per:?} avg  {:.0} MiB/s (acc={acc:08x})",
        1.0 / per.as_secs_f64()
    );

    // ── TickvFile: open fast-mount (ckpt TKCK) vs full-scan — sob CHURN ────
    // Cenário onde o ckpt compensa: 20k puts + 15k deletes (tombstones) →
    // volume com 35k records, live set de 5k. O ckpt indexa só os vivos; o
    // fast-mount não re-processa os 15k tombstones (o full-scan faz).
    const TV_N: usize = 20_000;
    let tv_path = std::env::temp_dir().join("neural_sgdb_bench_tickv.db");
    let _ = std::fs::remove_file(&tv_path);
    {
        let mut tv = TickvFile::open(&tv_path).unwrap();
        for i in 0..TV_N {
            tv.put(format!("md/L2/{i:06}").as_bytes(), b"payload").unwrap();
            if i % 4 != 0 {
                tv.delete(format!("md/L2/{i:06}").as_bytes()).unwrap(); // 15k tombstones
            }
        }
        tv.checkpoint().unwrap(); // ckpt indexa apenas os 5k vivos
    }
    let _ = TickvFile::open(&tv_path).unwrap(); // warm
    let t0 = Instant::now();
    drop(TickvFile::open(&tv_path).unwrap());
    let fast_mount = t0.elapsed();
    // torna o ckpt stale (append pós-ckpt) → open cai em scan completo
    {
        let mut tv = TickvFile::open(&tv_path).unwrap();
        tv.put(b"stale/marker", b"1").unwrap();
    }
    let t0 = Instant::now();
    drop(TickvFile::open(&tv_path).unwrap());
    let full_scan = t0.elapsed();
    let _ = std::fs::remove_file(&tv_path);
    println!(
        "tickv open (churn: 35k recs, 5k live) : fast-mount(ckpt)={fast_mount:?}  full-scan(stale)={full_scan:?}  ({:.1}x)",
        full_scan.as_secs_f64() / fast_mount.as_secs_f64().max(1e-9)
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
