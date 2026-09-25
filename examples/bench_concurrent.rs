//! Concurrent write+search benchmark — seekdb Gap 0 (2026-09-25).
//!
//! Run: `cargo run --release --example bench_concurrent`
//!
//! O claim central do seekdb (OceanBase) é P99 flat sob write+search
//! concorrente (21,7 ms @ 1.523 QPS). O neural-sgdb NUNCA mediu o seu — a
//! matriz de testes toda é single-thread/single-writer. Este bench fecha o
//! gap de MEDIÇÃO (não de arquitetura):
//!
//! - N threads writers fazendo `remember_text_with` (o caminho quente do
//!   MCP `remember(text=)` → L3) sobre TickvFile, num ÚNICO `Sgdb` protegido
//!   por Mutex (o modelo de uso real: um engine, chamadas concorrentes).
//! - M threads readers fazendo `recall_lexical` (o default MCP) contra o
//!   mesmo engine, medindo latência por chamada.
//! - Percentis P50/P90/P99/P99.9 + jitter (P99/P50).
//!
//! Saída honesta: números brutos, sem comparativo retórico. A leitura
//! (flat vs não-flat) fica para BENCHMARKS.md / seekdb-analysis.md.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use neural_sgdb::{RememberOptions, Sgdb, TickvFile};

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn percentiles(mut samples: Vec<Duration>) -> (Duration, Duration, Duration, Duration) {
    samples.sort();
    let p = |q: f64| {
        let idx = ((samples.len() as f64) * q) as usize;
        samples[idx.min(samples.len().saturating_sub(1))]
    };
    (p(0.50), p(0.90), p(0.99), p(0.999))
}

fn report(name: &str, n: usize, errs: usize, samples: &[Duration]) {
    if samples.is_empty() {
        println!("{name:<10} n=0 errs={errs} (sem amostras)");
        return;
    }
    let (p50, p90, p99, p999) = percentiles(samples.to_vec());
    let total: Duration = samples.iter().sum();
    let jitter = if p50.as_nanos() > 0 {
        p99.as_nanos() as f64 / p50.as_nanos() as f64
    } else {
        f64::INFINITY
    };
    println!(
        "{name:<10} n={n:<8} errs={errs:<4} {:>8.0} ops/s | P50 {:>8.2?}  P90 {:>8.2?}  P99 {:>8.2?}  P99.9 {:>9.2?}  jitter {:>5.1}×",
        n as f64 / total.as_secs_f64().max(1e-9),
        p50,
        p90,
        p99,
        p999,
        jitter,
    );
}

const QUERIES: [&str; 8] = [
    "deployment rollout status",
    "database migration plan",
    "user preference theme dark",
    "api rate limit breach",
    "memory consolidation task",
    "vector index rebuild",
    "agent scope isolation",
    "checkpoint audit trail",
];

fn main() {
    let n_writers = env_usize("BENCH_WRITERS", 4);
    let n_readers = env_usize("BENCH_READERS", 4);
    let secs = env_usize("BENCH_SECS", 5) as u64;
    let dir = std::env::temp_dir().join("neural_sgdb_bench_concurrent");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("concurrent.db");
    let _ = std::fs::remove_file(&path);

    println!(
        "neural-sgdb bench_concurrent — writers={n_writers} readers={n_readers} duration={secs}s (TickvFile, engine único sob Mutex)"
    );

    // Seed: 1k docs para o pool lexical não nascer vazio.
    {
        let storage = TickvFile::open(&path).expect("open seed db");
        let mut db = Sgdb::open(storage).expect("open sgdb");
        for i in 0..1_000 {
            let _ = db.remember_text_with(
                &format!("seed/{i:06}"),
                &format!("seed doc {i} about deployment and memory states"),
                RememberOptions::default(),
            );
        }
        db.checkpoint().expect("seed checkpoint");
        println!("seed: 1000 docs L3");
    }

    let storage = TickvFile::open(&path).expect("open db");
    let db = Arc::new(Mutex::new(Sgdb::open(storage).expect("open sgdb")));

    let stop = Arc::new(AtomicBool::new(false));
    // Passo 0 (instrumentação): latência TOTAL por chamada + a parte que é
    // LOCK-WAIT (Instant antes/depois do lock()) vs OP-TIME (trabalho dentro
    // do lock). total = lock_wait + op_time + (overhead de medida, mínimo).
    // Sem essa separação, qualquer fix do tail é chute: fila do Mutex e
    // trabalho dentro do lock pedem remédios diferentes.
    let write_lat: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));
    let read_lat: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));
    let write_wait: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));
    let read_wait: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));
    let write_err = Arc::new(AtomicUsize::new(0));
    let read_err = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();

    // Readers — P99 do recall é a métrica-alvo.
    for r in 0..n_readers {
        let db = Arc::clone(&db);
        let stop = Arc::clone(&stop);
        let lat = Arc::clone(&read_lat);
        let wait = Arc::clone(&read_wait);
        let errs = Arc::clone(&read_err);
        handles.push(std::thread::spawn(move || {
            let mut i = r;
            while !stop.load(Ordering::Relaxed) {
                let q = QUERIES[i % QUERIES.len()];
                i += 1;
                let t0 = Instant::now();
                let mut guard = db.lock().unwrap();
                let t1 = Instant::now();
                let res = guard.recall_lexical(q, 5);
                let t2 = Instant::now();
                drop(guard);
                if res.is_ok() {
                    lat.lock().unwrap().push(t2 - t0);
                    wait.lock().unwrap().push(t1 - t0);
                } else {
                    errs.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    // Writers — o caminho quente: remember_text_with → L3 (+ índice lexical).
    for w in 0..n_writers {
        let db = Arc::clone(&db);
        let stop = Arc::clone(&stop);
        let lat = Arc::clone(&write_lat);
        let wait = Arc::clone(&write_wait);
        let errs = Arc::clone(&write_err);
        handles.push(std::thread::spawn(move || {
            let mut i = w * 10_000;
            while !stop.load(Ordering::Relaxed) {
                let text = format!(
                    "writer {w} tick {i} concurrent benchmark note {}",
                    QUERIES[i % QUERIES.len()]
                );
                i += 1;
                let t0 = Instant::now();
                let mut guard = db.lock().unwrap();
                let t1 = Instant::now();
                let res = guard.remember_text_with(
                    &format!("w{w}/{i:010}"),
                    &text,
                    RememberOptions::default(),
                );
                let t2 = Instant::now();
                drop(guard);
                if res.is_ok() {
                    lat.lock().unwrap().push(t2 - t0);
                    wait.lock().unwrap().push(t1 - t0);
                } else {
                    errs.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    std::thread::sleep(Duration::from_secs(secs));
    stop.store(true, Ordering::Relaxed);
    for h in handles {
        let _ = h.join();
    }

    println!();
    let w = Arc::try_unwrap(write_lat).ok().and_then(|m| m.into_inner().ok());
    let r = Arc::try_unwrap(read_lat).ok().and_then(|m| m.into_inner().ok());
    let ww = Arc::try_unwrap(write_wait).ok().and_then(|m| m.into_inner().ok());
    let rw = Arc::try_unwrap(read_wait).ok().and_then(|m| m.into_inner().ok());
    let wn = w.as_ref().map(|v| v.len()).unwrap_or(0);
    let rn = r.as_ref().map(|v| v.len()).unwrap_or(0);
    report(
        "write",
        wn,
        write_err.load(Ordering::Relaxed),
        w.as_deref().unwrap_or(&[]),
    );
    report(
        "recall",
        rn,
        read_err.load(Ordering::Relaxed),
        r.as_deref().unwrap_or(&[]),
    );

    // Passo 0: separação lock-wait vs op-time (o remédio depende de qual
    // dos dois domina o tail — fila do Mutex vs trabalho dentro do lock).
    println!("\n— decomposição do tail (passo 0) —");
    for (label, wait, tot) in [
        ("write", &ww, &w),
        ("recall", &rw, &r),
    ] {
        let (Some(ww), Some(t)) = (wait, tot) else { continue };
        let (w50, _, w99, _) = percentiles(ww.clone());
        // op-time = total − wait (aprox: t2−t1 medido, sem refazer chamadas).
        let mut op: Vec<Duration> = t.to_vec();
        let waits = ww.clone();
        for (o, wait) in op.iter_mut().zip(waits) {
            *o = o.saturating_sub(wait);
        }
        let (o50, _, o99, _) = percentiles(op);
        println!(
            "{label:<7} lock-wait P50 {:>8.2?} P99 {:>8.2?} | op-time P50 {:>8.2?} P99 {:>8.2?}",
            w50, w99, o50, o99
        );
    }
    println!("\nleitura: tail dominado por lock-wait → fila do Mutex (remédio: reducir contenção, ex. auto-persist fora do round trip);");
    println!("dominado por op-time → trabalho dentro do lock (remédio: TickvFile buffered / reduzir o próprio op).",);

    // Tail do arquivo (crescimento sob carga).
    if let Ok(md) = std::fs::metadata(&path) {
        println!("\ndb file: {} bytes", md.len());
    }
}
