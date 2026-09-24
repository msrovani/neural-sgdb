//! Bench do fast-mount IDX1 (v1.1.29, ADR-0009 §3) — `open()` legado vs
//! `open_with_snapshot()`.
//!
//! Mede, por N writes (docs = 2×N: L4 + companion L2), três fases:
//! 1. build: escreve N docs + persist_index_snapshot (com o custo do IO extra);
//! 2. open() legado (full rebuild) — o baseline;
//! 3. open_with_snapshot() (fast-mount) — o candidato.
//!
//! Valida o invariante do ADR-0011 no fim: fp(fast-mount) == fp(rebuild).
//! Determinístico (LCG, sem LLM). Exit 0 sse os invariantes seguram.
//!
//! Rodar: `cargo run --release --example bench_index_snapshot`

use neural_sgdb::{FileStorage, Sgdb};
use std::time::Instant;

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

/// LCG determinístico para embeddings (mesmo gerador dos demais harnesses).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 32
    }
    fn emb(&mut self, dim: usize) -> Vec<f32> {
        (0..dim)
            .map(|_| ((self.next() % 2000) as f32 - 1000.0) / 1000.0)
            .collect()
    }
}

fn build(path: &std::path::Path, n_writes: usize) {
    let _ = std::fs::remove_file(path);
    let mut db = Sgdb::open(FileStorage::open(path).unwrap()).unwrap();
    let mut rng = Lcg(0xC0FF_EE00_5EED_0001);
    for i in 0..n_writes {
        let e = rng.emb(64);
        db.remember_semantic(&format!("bn/{i:06}"), "corpo do doc do bench de snapshot", &e)
            .unwrap();
    }
    db.persist_index_snapshot(now_ms() as u64 + 10).unwrap();
}

fn main() {
    println!("=== Bench open() vs open_with_snapshot() (IDX1, ADR-0009 §3) ===\n");
    println!(
        "{:>10} {:>12} {:>14} {:>14} {:>10} {:>12}",
        "N writes", "docs", "open legado", "fast-mount", "speedup", "persist idx"
    );

    let dir = std::env::temp_dir().join("nsg_bench_idx1");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut failures = 0usize;
    // Teto honesto do V1: o snapshot é UM valor no storage (`MAX_VLEN` =
    // 1 MiB) — acima disso a escrita falha com `Storage("limits")`.
    // ~7k writes (14k docs, dim 64) encostam no teto. Paginação do snapshot
    // em múltiplos valores é trabalho futuro (ver BENCHMARKS §Open cost).
    for &n in &[400usize, 1_600, 4_000] {
        let path = dir.join(format!("mem_{n}.db"));
        let t0 = Instant::now();
        build(&path, n);
        let build_ms = t0.elapsed().as_millis();

        // persist idx cost: re-persiste num DB aberto e mede só a chamada
        let (persist_ms, fp_ref) = {
            let mut db = Sgdb::open(FileStorage::open(&path).unwrap()).unwrap();
            let t = Instant::now();
            db.persist_index_snapshot(now_ms() as u64 + 20).unwrap();
            (t.elapsed().as_millis(), db.index_fingerprint())
        };

        // baseline: open() legado (full rebuild)
        let (legacy_ms, fp_legacy) = {
            let t = Instant::now();
            let db = Sgdb::open(FileStorage::open(&path).unwrap()).unwrap();
            (t.elapsed().as_millis(), db.index_fingerprint())
        };

        // candidato: open_with_snapshot (fast-mount)
        let (fast_ms, fp_fast) = {
            let t = Instant::now();
            let db = Sgdb::open_with_snapshot(1, FileStorage::open(&path).unwrap()).unwrap();
            (t.elapsed().as_millis(), db.index_fingerprint())
        };

        // invariantes ADR-0011: os três caminhos produzem o MESMO índice.
        if fp_ref != fp_legacy {
            eprintln!("  FALHA: fp da sessão de coleta != fp do rebuild legado");
            failures += 1;
        }
        if fp_fast != fp_legacy {
            eprintln!("  FALHA: fp do fast-mount != fp do rebuild legado");
            failures += 1;
        }

        let speedup = legacy_ms as f64 / fast_ms.max(1) as f64;
        println!(
            "{:>10} {:>12} {:>10} ms {:>10} ms {:>9.1}x {:>8} ms",
            n,
            2 * n,
            legacy_ms,
            fast_ms,
            speedup,
            persist_ms
        );
        let _ = build_ms;
    }

    let _ = std::fs::remove_dir_all(&dir);
    if failures == 0 {
        println!("\nRESULTADO: PASSOU (fingerprint idêntico nos 3 caminhos)");
    } else {
        println!("\nRESULTADO: FALHOU ({failures} invariante(s) violado(s))");
        std::process::exit(1);
    }
}
