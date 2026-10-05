// v1.4.4 (Lote F) — curva de FOOTPRINT: RSS × tamanho do corpus.
//
// Por que existe: o datapoint de produção (0,6 MB / 6% CPU) é UM ponto, sem
// curva — e número sem contexto não é comparável (doutrina do BENCHMARKS.md).
// Esta curva responde "quanto custa o próximo 10k de memórias", que é a
// pergunta real de quem roda isto em Ring 0.
//
// RSS SEM DEPENDENCIA NOVA (regra 1 do repo: zero deps na lib; exemplos usam
// só dev-deps ja existentes):
//   Linux   → /proc/self/statm (página × page_size)
//   Windows → GetProcessMemoryInfo(psapi) via `extern "system"`
//   outros  → "unavailable" (ausência declarada, nunca número inventado)
//
// ROMPE NADA no hot test: mede, não altera contrato. `BENCH_N` limita o maior
// ponto (default 50k; use BENCH_N=10000 p/ iteração rápida).

use neural_sgdb::{RememberOptions, Sgdb};
use std::time::Instant;

// ── RSS por plataforma ───────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn rss_bytes() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = s.split_whitespace().nth(1)?.parse().ok()?;
    // page_size = 4 KiB em x86_64/aarch64 Linux; o campo é o mesmo p/ qualquer
    //页_size suportada, então lemos do próprio kernel seria o ideal — 4 KiB é
    // o valor em 100% dos alvos que este crate suporta.
    Some(pages * 4096)
}

#[cfg(windows)]
fn rss_bytes() -> Option<u64> {
    use std::ffi::c_void;
    #[repr(C)]
    #[derive(Default)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
    }
    #[link(name = "psapi")]
    extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }
    let mut c = ProcessMemoryCounters::default();
    // SAFETY: `c` é um POD com layout correto e cb = sizeof; a API só escreve
    // dentro do buffer informado. Processo corrente é sempre um handle válido.
    unsafe {
        if GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut c as *mut ProcessMemoryCounters,
            std::mem::size_of::<ProcessMemoryCounters>() as u32,
        ) == 0
        {
            return None;
        }
    }
    Some(c.working_set_size as u64)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn rss_bytes() -> Option<u64> {
    None
}

fn fmt_rss(v: Option<u64>) -> String {
    match v {
        Some(b) if b >= 1 << 20 => format!("{:.2} MB", b as f64 / (1u64 << 20) as f64),
        Some(b) => format!("{:.0} kB", b as f64 / 1024.0),
        None => "unavailable (leia na ferramenta do SO)".to_string(),
    }
}

// ── Driver ───────────────────────────────────────────────────────────────────

/// Ponto da curva: N memórias -> RSS + custo de write.
fn point(n: usize, base_rss: Option<u64>) {
    let t = Instant::now();
    let mut db = Sgdb::open(neural_sgdb::InMemory::new()).expect("open InMemory");
    let open_ms = t.elapsed().as_secs_f64() * 1000.0;

    let t = Instant::now();
    for i in 0..n {
        // texto com o `key` no corpo: é o que um agente realmente grava (o
        // lexical indexa; o BQ fica fora deste bench por ser lexical-only)
        db.remember_text_with(
            &format!("fp/{i:08}"),
            &format!("memoria de benchmark numero {i} com texto suficiente para o indice lexical"),
            RememberOptions::default(),
        )
        .expect("remember_text_with");
    }
    let write_ms = t.elapsed().as_secs_f64() * 1000.0;

    let rss = rss_bytes();
    let delta = match (base_rss, rss) {
        (Some(b), Some(r)) if r > b => Some(r - b),
        _ => None,
    };
    let per_doc = delta.map(|d| d as f64 / n as f64);

    println!(
        "{:>7} docs | RSS {:>28} | delta {:>16} | {:>8.1} B/doc | write {:>7.1} ms ({:>6.1} us/doc) | open {:.2} ms",
        n,
        fmt_rss(rss),
        fmt_rss(delta),
        per_doc.unwrap_or(0.0),
        write_ms,
        write_ms * 1000.0 / n as f64,
        open_ms,
    );
    // segurar `db` viva até aqui: soltar antes mediria o drop, não o footprint
    std::hint::black_box(db.health().doc_count);
}

fn main() {
    let max_n: usize = std::env::var("BENCH_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50_000);
    // A curva pedida: 1k/10k/50k. BENCH_N corta o topo (50k ja demora).
    let points: Vec<usize> = [1_000, 10_000, 50_000]
        .into_iter()
        .filter(|n| *n <= max_n)
        .collect();
    let points = if points.is_empty() {
        vec![max_n]
    } else {
        points
    };

    println!("neural-sgdb — footprint (v1.4.4, Lote F)");
    println!("RSS via /proc/self/statm (linux) | GetProcessMemoryInfo (windows) | sem dep nova\n");
    println!(
        "{:>7}      | {:>32} | {:>23} | {:>14} | {:>28}",
        "corpus", "RSS (absoluto)", "delta vs base", "B/doc", "write / open"
    );

    let base = rss_bytes();
    for n in points {
        point(n, base);
    }

    println!(
        "\nleitura honesta: em InMemory o corpus vive no heap do processo, entao o\n\
         RSS cresce ~1:1 com o volume (e o que o embedded paga ate o checkpoint\n\
         L0/L1 ou o flush). Para o custo de ARQUIVO por corpus, ver\n\
         bench_index_snapshot.rs; para o datapoint de processo do agente MCP,\n\
         a secao 'Real-world footprint' do BENCHMARKS.md."
    );
}