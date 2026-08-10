//! Stress test — neural-sgdb sob carga (release).
//!
//! Run: `cargo run --release --example stress`
//!
//! Cobre 100k operações (padrão; reduza via `STRESS_N` para CI rápido):
//! - 100k `remember_semantic` (L4 + companion L2) + 100k `recall`
//! - 100k `remember_exchange` (L1/L2 RAM) + checkpoint + prune + reopen
//! - 100k opens/closes ("processos") com rebuild de índices
//!
//! Métricas: contagens, tempos, ops/s, tamanho do arquivo, recall@1 do filtro
//! BQ e integridade (match exato recuperável com k maior).
//!
//! Obs: com embeddings de DIMS baixas (16), o filtro grosseiro BQ colide em
//! bits — o match exato nem sempre está no top-k pequeno (trade-off conhecido;
//! use 1024 dims / k maior para recall completo).

use std::time::Instant;

use neural_sgdb::{FileStorage, InMemory, BqFlatIndex, Sgdb};
use neural_sgdb::hamming_kernel_name;
use neural_sgdb::Storage;

fn emb(seed: u64) -> Vec<f32> {
    let mut s = seed.wrapping_mul(1103515245).wrapping_add(12345);
    let mut v = vec![0f32; 16];
    for x in v.iter_mut() {
        s = s.wrapping_mul(1103515245).wrapping_add(12345);
        *x = ((s >> 32) as i32 % 200) as f32 / 100.0 - 1.0;
    }
    v
}

/// 1024 dims = 16 words → exercita o loop SIMD (AVX2/AVX-512) de verdade.
fn emb1024(seed: u64) -> Vec<f32> {
    let mut s = seed.wrapping_mul(1103515245).wrapping_add(12345);
    let mut v = vec![0f32; 1024];
    for x in v.iter_mut() {
        s = s.wrapping_mul(1103515245).wrapping_add(12345);
        *x = ((s >> 32) as i32 % 200) as f32 / 100.0 - 1.0;
    }
    v
}

/// Relatório de capacidades SIMD + kernel selecionado (seam hamming_dispatch).
/// Mostra o melhor conjunto que a CPU oferece vs o que o dispatch escolheu.
fn report_cpu_caps() {
    let caps = neural_sgdb::cpu_caps();
    let best = if caps.avx512 {
        "avx512"
    } else if caps.avx2 {
        "avx2_xor"
    } else {
        "scalar"
    };
    println!(
        "CPU caps   : avx2={} avx512={} | melhor disponível: {best} | kernel ativo: {}",
        caps.avx2,
        caps.avx512,
        hamming_kernel_name()
    );
    assert_eq!(
        hamming_kernel_name(),
        best,
        "dispatch deveria escolher o melhor kernel da CPU"
    );
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn report(name: &str, n: usize, t: std::time::Duration) {
    let per = if n > 0 { t.as_secs_f64() * 1e9 / n as f64 } else { 0.0 };
    println!(
        "  {name:<30} n={n:<8} total={:>9.3}s  {:>7.0} ns/op  {:>10.0} ops/s",
        t.as_secs_f64(),
        per,
        n as f64 / t.as_secs_f64()
    );
}

fn main() {
    let n = env_usize("STRESS_N", 100_000);
    let n_reopen = env_usize("STRESS_REOPEN", 100_000);
    let n_del = env_usize("STRESS_DEL", 20_000);
    let n_simd = env_usize("STRESS_SIMD", 10_000);
    let dir = std::env::temp_dir().join("neural_sgdb_stress");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("stress.db");
    let _ = std::fs::remove_file(&path);

    println!("neural-sgdb stress — STRESS_N={n} STRESS_REOPEN={n_reopen}");
    report_cpu_caps();
    println!();

    // ── A: 100k remember_semantic + 100k recall (FileStorage) ─────────────
    {
        let mut db = Sgdb::open(FileStorage::open(&path).unwrap()).unwrap();
        let t_put = Instant::now();
        for i in 0..n {
            let key = format!("stress/{i:06}");
            db.remember_semantic(&key, &format!("mem-{i:06}"), &emb(i as u64))
                .unwrap();
        }
        report("remember_semantic", n, t_put.elapsed());
        assert_eq!(db.scan_prefix("md/L4/").unwrap().len(), n, "L4 indexou tudo");
        let bytes = std::fs::metadata(&path).unwrap().len();
        println!(
            "  log file (append-only, sem compact) : {:.1} MiB",
            bytes as f64 / 1048576.0
        );

        // recall@1 = fração dos queries onde o match exato cai no top-1 do
        // filtro grosseiro BQ (métrica honesta do trade-off, não assert)
        let t_q = Instant::now();
        let mut exact1 = 0usize;
        for i in 0..n {
            let hits = db.recall(&emb(i as u64), 1).unwrap();
            if hits.first().map(|h| h.dist == 0.0).unwrap_or(false) {
                exact1 += 1;
            }
        }
        report("recall(k=1) scan 100k", n, t_q.elapsed());
        println!(
            "  exact match no top-1 (BQ coarse)     : {}/{} ({:.1}%) — dims baixas colidem em bits",
            exact1,
            n,
            exact1 as f64 * 100.0 / n as f64
        );

        // integridade: com k=256 o match exato é recuperável para todos
        let t_integr = Instant::now();
        let mut found = 0usize;
        for i in (0..n).step_by(1000) {
            let hits = db.recall(&emb(i as u64), 256).unwrap();
            if hits.iter().any(|h| h.text == format!("mem-{i:06}")) {
                found += 1;
            }
        }
        let samples = (0..n).step_by(1000).count();
        report("integridade recall(k=256) amostra", samples, t_integr.elapsed());
        assert_eq!(found, samples, "match exato deveria ser recuperável com k=256");
        println!("  match exato recuperado            : {found}/{samples}");
    }

    // ── B: 100k remember_exchange + checkpoint + prune + reopen ────────────
    let t0 = Instant::now();
    {
        let mut db = Sgdb::open(FileStorage::open(&path).unwrap()).unwrap();
        let t_ex = Instant::now();
        for i in 0..n {
            db.remember_exchange(&format!("user {i}"), &format!("ai {i}")).unwrap();
        }
        report("remember_exchange (L1 RAM+L2)", n, t_ex.elapsed());
        let t_ck = Instant::now();
        let flushed = db.checkpoint().unwrap();
        let pruned = db.prune_working_ram().unwrap();
        println!(
            "  checkpoint {flushed} L0/L1 → Storage ({:.3}s), prune {pruned}",
            t_ck.elapsed().as_secs_f64()
        );
        assert_eq!(db.ram_len(), 0, "RAM L0/L1 zerada pós prune");
        assert!(db.scan_prefix("md/L2/").unwrap().len() >= n);
    }
    let t_r = Instant::now();
    {
        let mut db = Sgdb::open(FileStorage::open(&path).unwrap()).unwrap();
        assert!(db.scan_prefix("md/L4/").unwrap().len() >= n);
        assert!(db.scan_prefix("md/L2/").unwrap().len() >= n);
        let hits = db.recall(&emb(42), 256).unwrap();
        assert!(hits.iter().any(|h| h.text == "mem-000042"), "reopen perdeu memória");
        assert!(hits.iter().any(|h| h.dist == 0.0));
    }
    println!("  reopen + rebuild índices        : {:.3}s", t_r.elapsed().as_secs_f64());
    report("seção A+B", 2 * n, t0.elapsed());
    println!();

    // ── C: 100k opens/closes ("processos") com rebuild de índices ─────────
    let small = dir.join("stress_small.db");
    let _ = std::fs::remove_file(&small);
    {
        let mut db = Sgdb::open(FileStorage::open(&small).unwrap()).unwrap();
        for i in 0..50 {
            db.remember_semantic(&format!("s/{i:03}"), &format!("s-{i:03}"), &emb(i))
                .unwrap();
        }
        db.remember_fact("fato stress", 1).unwrap();
        db.checkpoint().unwrap();
    }
    let t_re = Instant::now();
    let mut ok = 0usize;
    for i in 0..n_reopen {
        let mut db = Sgdb::open(FileStorage::open(&small).unwrap()).unwrap();
        if db.scan_prefix("md/L4/").unwrap().len() == 50 {
            ok += 1;
        }
        if i % 10_000 == 0 {
            let _ = db.recall(&emb(7), 1).unwrap();
        }
    }
    report("Sgdb open/close + rebuild", n_reopen, t_re.elapsed());
    println!("  reopens íntegros (L4==50)     : {ok}/{n_reopen}");
    assert_eq!(ok, n_reopen);
    let _ = std::fs::remove_file(&small);
    let _ = std::fs::remove_file(&path);
    println!();

    // ── D: sanity InMemory (path de baixa latência) ────────────────────────
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    let t_im = Instant::now();
    for i in 0..n {
        db.remember_semantic(&format!("m/{i:06}"), "x", &emb(i as u64)).unwrap();
    }
    report("InMemory remember_semantic", n, t_im.elapsed());
    assert_eq!(db.bq_len(), n);
    println!();

    // ── E: SIMD real — BQ top-5 com 1024 dims (16 words = loop AVX2/AVX512) ──
    {
        let mut bq = BqFlatIndex::new();
        for i in 0..n_simd {
            bq.insert_f32(i as u64, &emb1024(i as u64));
        }
        let query = emb1024(0xDEAD_BEEF);
        let t_simd = Instant::now();
        let mut ok = true;
        for _ in 0..200 {
            let r = bq.top_k_f32(&query, 5);
            ok &= !r.is_empty();
        }
        report("BQ top-5 (1024-dim)", 200, t_simd.elapsed());
        println!(
            "  kernel SIMD ativo: {} ({}/vec — loop SIMD real, não tail scalar)",
            hamming_kernel_name(),
            bq.words_per_vec
        );
        assert!(ok);
    }

    // ── F: deleção + tombstones + recovery (Storage cru) ───────────────────
    let del_path = dir.join("stress_del.db");
    let _ = std::fs::remove_file(&del_path);
    {
        let mut st = FileStorage::open(&del_path).unwrap();
        let t_w = Instant::now();
        for i in 0..n_del {
            st.put(format!("k/{i:06}").as_bytes(), format!("v{i}").as_bytes())
                .unwrap();
        }
        report("Storage::put (raw)", n_del, t_w.elapsed());
        let t_d = Instant::now();
        for i in (0..n_del).step_by(2) {
            st.delete(format!("k/{i:06}").as_bytes()).unwrap();
        }
        report("Storage::delete (tombstone)", n_del / 2, t_d.elapsed());
    }
    // recovery: pares deletados não ressuscitam; ímpares vivos sobrevivem
    {
        let mut st = FileStorage::open(&del_path).unwrap();
        let t_v = Instant::now();
        for i in (0..n_del).step_by(2) {
            assert!(
                st.get(format!("k/{i:06}").as_bytes()).unwrap().is_none(),
                "chave {i} deletada ressuscitou (bughunt #1/#2)"
            );
        }
        for i in (1..n_del).step_by(2) {
            assert!(
                st.get(format!("k/{i:06}").as_bytes()).unwrap().is_some(),
                "chave {i} viva sumiu"
            );
        }
        report("reopen + verificação deletes", n_del, t_v.elapsed());
    }
    let _ = std::fs::remove_file(&del_path);

    // ── G: troca de inferência entre 2 instâncias (A grava → B lê → B grava → A relê) ──
    let exch_path = dir.join("stress_exch.db");
    let _ = std::fs::remove_file(&exch_path);
    {
        // A: grava memórias de inferência
        let mut a = Sgdb::open(FileStorage::open(&exch_path).unwrap()).unwrap();
        for i in 0..2000 {
            a.remember_semantic(&format!("a/{i:04}"), &format!("de-A-{i:04}"), &emb(i))
                .unwrap();
        }
        a.checkpoint().unwrap();
        a.prune_working_ram().unwrap();
    }
    {
        // B: nova instância lê a inferência de A e responde
        let mut b = Sgdb::open(FileStorage::open(&exch_path).unwrap()).unwrap();
        let hits = b.recall(&emb(7), 256).unwrap();
        assert!(
            hits.iter().any(|h| h.text == "de-A-0007"),
            "B não leu a inferência de A"
        );
        b.remember_semantic("b/resp-1", "de-B-resposta", &emb(0xCAFE)).unwrap();
        b.checkpoint().unwrap();
    }
    {
        // A: reabre e lê a resposta de B (troca de inferência consumada)
        let mut a2 = Sgdb::open(FileStorage::open(&exch_path).unwrap()).unwrap();
        let hits = a2.recall(&emb(0xCAFE), 256).unwrap();
        assert!(
            hits.iter().any(|h| h.text == "de-B-resposta"),
            "A não leu a resposta de B"
        );
    }
    println!("  troca de inferência A→B→A : ok (2 instâncias, reopen no meio)");
    let _ = std::fs::remove_file(&exch_path);
    println!();
    println!("stress OK — todas as integridades verificadas.");
}
