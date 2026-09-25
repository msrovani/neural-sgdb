//! Micro-bench do delete vs clock_index — isola o custo O(N) por delete.
//!
//! Run: `cargo run --release --example bench_delete_cost`
//!
//! Hipótese (levada do bench_long_db): `engine::delete` varre o
//! `clock_index` INTEIRO por delete (`clock_index.iter().filter(...)`),
//! então deletar D docs de um banco com N docs custa O(N·D) — o bench de
//! DB longo mediu 30k deletes em 265 s (8.8 ms/delete) com N=60k, enquanto
//! writes custam ~37 µs. Se confirmado, o fix é o índice reverso
//! sk → [(nó, contador)] (mantido no put), tornando o delete O(1) amortizado.

use std::time::Instant;

use neural_sgdb::{FileStorage, Sgdb};

fn emb(seed: u64) -> Vec<f32> {
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

fn main() {
    // Três tamanhos: se o custo POR delete cresce com N, a hipótese O(N·D) confirma.
    for n in [5_000usize, 10_000, 20_000] {
        let dir = std::env::temp_dir().join("neural_sgdb_bench_delete_cost");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("del_{n}.db"));
        let _ = std::fs::remove_file(&path);

        let storage = FileStorage::open(&path).expect("open db");
        let mut db = Sgdb::open(storage).expect("open sgdb");
        for i in 0..n {
            db.remember_semantic(&format!("doc/{i:08}"), &format!("doc {i}"), &emb(i as u64))
                .expect("write");
        }
        // Deleta os mesmos 200 docs em cada N: o custo POR delete deve ser
        // constante se o delete for O(1); cresce com N se varre o clock_index.
        const DELETES: usize = 200;
        let t = Instant::now();
        for i in 0..DELETES {
            let _ = db.delete(&format!("md/L4/doc/{i:08}")).expect("delete");
        }
        let dt = t.elapsed();
        println!(
            "N={n:<7} deletes={DELETES:<5} total={:>8.2?}  {:>9.2} ms/delete",
            dt,
            dt.as_secs_f64() * 1e3 / DELETES as f64
        );
        let _ = std::fs::remove_file(&path);
    }
    println!("\nms/delete CONSTANTE com N = delete O(1) (são as side-tables + índices).");
    println!("ms/delete CRESCENDO com N = varredura O(N) por delete (clock_index) — fix: índice reverso sk→[(nó,contador)] mantido no put.");
}
