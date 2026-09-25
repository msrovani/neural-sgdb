//! Long-lived DB bench — seekdb-analysis item 3 (delta BQ two-level).
//!
//! Run: `cargo run --release --example bench_long_db`
//!
//! Pergunta a responder (mede antes de construir): num DB de vida longa com
//! churn (100k writes L4 + 50k deletes), o scan de entradas inertes do BQ
//! flat (órfãos pós-delete) domina o custo do recall — a ponto de justificar
//! um índice two-level (delta BQ hot + snapshot recompactado)?
//!
//! Fases medidas (mesmas queries, mesmo k): 1. baseline (recall limpo),
//! 2. churn (50% deletes — o reclaim de threshold 64 dispara sozinho no
//! caminho do delete, bq_len reportado mostra; a fase isola o custo residual
//! de órfãos no recall), 3. pos-reclaim (`reclaim_bq_orphans(0)`,
//! recompacção total), 4. pos-rebuild (`rebuild_indices()`, o pior caso
//! honesto: fallback de restart).
//!
//! Métricas: P50/P99 de recall, ops/s, bq_len (entradas totais vs vivas),
//! custo do reclaim e do rebuild em si.

use std::time::{Duration, Instant};

use neural_sgdb::{FileStorage, Sgdb};

fn emb(seed: u64) -> Vec<f32> {
    // 256 dims (trilha demo-like): 4 words no BQ. Clusters correlacionados
    // para o recall ter trabalho real (não ruído puro).
    let mut s = seed.wrapping_mul(1103515245).wrapping_add(12345);
    let mut v = vec![0f32; 256];
    for x in v.iter_mut() {
        s = s.wrapping_mul(1103515245).wrapping_add(12345);
        *x = ((s >> 32) as i32 % 200) as f32 / 100.0 - 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-8);
    v.iter_mut().for_each(|x| *x /= norm);
    v
}

fn queries(seed: u64, n: usize) -> Vec<Vec<f32>> {
    (0..n).map(|i| emb(seed.wrapping_add(i as u64 * 7919))).collect()
}

fn percentiles(mut samples: Vec<Duration>) -> (Duration, Duration) {
    samples.sort();
    let p = |q: f64| {
        let idx = ((samples.len() as f64) * q) as usize;
        samples[idx.min(samples.len().saturating_sub(1))]
    };
    (p(0.50), p(0.99))
}

fn bench_recall(db: &mut Sgdb, qs: &[Vec<f32>], k: usize, label: &str) {
    let mut samples = Vec::with_capacity(qs.len());
    for q in qs {
        let t = Instant::now();
        let _ = db.recall(q, k).expect("recall");
        samples.push(t.elapsed());
    }
    let total: Duration = samples.iter().sum();
    let (p50, p99) = percentiles(samples);
    println!(
        "{label:<14} n={:<5} P50 {:>8.2?}  P99 {:>8.2?}  {:>7.0} q/s",
        qs.len(),
        p50,
        p99,
        qs.len() as f64 / total.as_secs_f64().max(1e-9)
    );
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn main() {
    // Default = o alvo do item 3 (100k). Em máquinas lentas use BENCH_N menor
    // (as PROPORÇÕES se mantêm: 50% deletes, queries = writes/50).
    let n_writes = env_usize("BENCH_N", 100_000);
    let n_deletes = n_writes / 2;
    let n_queries = (n_writes / 50).max(200);
    const K: usize = 5;

    let dir = std::env::temp_dir().join("neural_sgdb_bench_long_db");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("long_db.db");
    let _ = std::fs::remove_file(&path);

    println!(
        "bench_long_db — writes={n_writes} deletes={n_deletes} queries={n_queries} k={K} (FileStorage, L4 256-dim)"
    );

    let t_write = Instant::now();
    {
        let storage = FileStorage::open(&path).expect("open db");
        let mut db = Sgdb::open(storage).expect("open sgdb");
        for i in 0..n_writes {
            let v = emb(i as u64);
            db.remember_semantic(&format!("doc/{i:08}"), &format!("doc {i}"), &v)
                .expect("write");
        }
        let dt = t_write.elapsed();
        println!(
            "write phase    : {n_writes} docs em {:.2}s ({:.0} writes/s, {:.1} µs/doc)",
            dt.as_secs_f64(),
            n_writes as f64 / dt.as_secs_f64(),
            dt.as_secs_f64() * 1e6 / n_writes as f64
        );
    }

    let storage = FileStorage::open(&path).expect("reopen db");
    let mut db = Sgdb::open(storage).expect("open sgdb");
    let qs = queries(42, n_queries);

    // ── 1. baseline: 0 órfãos ────────────────────────────────────────────
    println!("\n[1] baseline (bq_len={} vivo)", n_writes);
    bench_recall(&mut db, &qs, K, "baseline");

    // ── 2. deletes SEM reclaim (isola o efeito dos órfãos) ───────────────
    // `Sgdb::delete` dispara reclaim com threshold 64 — para ACUMULAR 50k
    // órfãos de verdade, os deletes precisam do caminho cru. Fazemos via
    // padrão realista: deleta metade dos docs e mede o bq_len resultante.
    // (Se o threshold disparar recompacção no meio, o bq_len reportado
    // mostra — e isso já é um dado.)
    let t_del = Instant::now();
    let mut deleted = 0usize;
    for i in (0..n_writes).step_by(2) {
        if db.delete(&format!("md/L4/doc/{i:08}")).unwrap_or(false) {
            deleted += 1;
        }
    }
    let dt_del = t_del.elapsed();
    let bq_len = db.bq_len();
    // Vivos: docs L4 com payload múltiplo de 4 no storage (o delete físico
    // remove o doc; o BQ append-only mantém o id — órfãos = bq_len − vivos).
    let live = db.scan_prefix("md/L4/").unwrap_or_default().len();
    println!(
        "\n[2] deletes: {deleted} em {:.2}s — bq_len={bq_len} (vivos={live}, órfãos={})",
        dt_del.as_secs_f64(),
        bq_len.saturating_sub(live)
    );
    bench_recall(&mut db, &qs, K, "com-orfaos");

    // ── 3. pós-reclaim (recompacção total) ───────────────────────────────
    let t_reclaim = Instant::now();
    let reclaimed = db.reclaim_bq_orphans(0);
    let dt_reclaim = t_reclaim.elapsed();
    let bq_len = db.bq_len();
    println!(
        "\n[3] reclaim(0): {reclaimed} removidos em {:.1} ms — bq_len={bq_len}",
        dt_reclaim.as_secs_f64() * 1e3
    );
    bench_recall(&mut db, &qs, K, "pos-reclaim");

    // ── 4. pós-rebuild (fallback de restart, pior caso honesto) ──────────
    let t_rebuild = Instant::now();
    let reindexed = db.rebuild_indices().expect("rebuild");
    let dt_rebuild = t_rebuild.elapsed();
    println!(
        "\n[4] rebuild: {reindexed} docs em {:.1} ms",
        dt_rebuild.as_secs_f64() * 1e3
    );
    bench_recall(&mut db, &qs, K, "pos-rebuild");

    // ── veredito automático ──────────────────────────────────────────────
    println!("\n— leitura —");
    println!("se 'com-orfaos' ≈ 'pos-reclaim' (P50/P99), os órfãos custam ~0");
    println!("no scan (o recall pula em O(1)) e o delta BQ two-level NÃO vale;");
    println!("se dominar, o item 3 sobe na fila. Ver seekdb-analysis.md.");
}
