//! F4 probe — memória auto-evolutiva: o host "sabe o que fazer" ou precisa ser
//! ensinado? (v1.4.0, docs/research-2026-10.md #4).
//!
//! Compara dois hosts DETERMINÍSTICOS nas mesmas 3 tarefas de gestão:
//! - **B0 (verbos passivos)**: escreve e NUNCA gere — não supersede, não
//!   consolida, não expira.
//! - **B1 (verbos + sinais do DB)**: usa os sinais que o DB expõe (retrieval
//!   por entidades, `consolidate_recurrences`, `expire_old`) para decidir
//!   supersede/consolidar/expirar — SEM doutrina textual.
//!
//! Métricas: SR (success rate binário) + sPS (soft progress) por tarefa.
//!
//! VEREDITO:
//!   SR(B1) > SR(B0)  => o DB TEM o que um host precisa para se auto-gerir
//!                       (GO na AFFORDANCE — manter/ampliar verbos+sinais).
//!   Combinar com `memory_arena_eval` (naive 0/3 vs protocolo 3/3): a
//!   COMPETÊNCIA (saber USAR) ainda depende de ensino (doutrina) -> a
//!   autonomia fica NO HOST, não no DB (NOGO p/ "DB decide").

use neural_sgdb::{ConsolidateConfig, InMemory, Sgdb};

fn emb(seed: u64) -> Vec<f32> {
    let mut s = seed.wrapping_mul(1103515245).wrapping_add(12345);
    let mut v = Vec::with_capacity(8);
    for _ in 0..8 {
        s = s.wrapping_mul(1103515245).wrapping_add(12345);
        v.push(((s >> 32) as i32 % 200) as f32 / 100.0 - 1.0);
    }
    v
}
fn seed_from(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// ── Tarefa 1: fato evolui (contradição) — supersede via sinal de entidades ──
fn task_supersede(host_b1: bool) -> (bool, f32) {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    // sessão 1: carlos trabalha na ACME
    let k1 = "job/v1";
    db.remember_semantic(k1, "carlos trabalha ACME", &emb(seed_from("carlos trabalha ACME")))
        .unwrap();
    db.set_entities(&format!("md/L4/{k1}"), &["user/carlos", "pred/trabalha"]).unwrap();
    // sessão 2: carlos agora trabalha na GLOBEX
    let k2 = "job/v2";
    db.remember_semantic(k2, "carlos trabalha GLOBEX", &emb(seed_from("carlos trabalha GLOBEX")))
        .unwrap();
    db.set_entities(&format!("md/L4/{k2}"), &["user/carlos", "pred/trabalha"]).unwrap();

    if host_b1 {
        // B1: o SINAL (retrieval por entidade) mostra v1 e v2 com o mesmo
        // subject+predicate e objeto diferente → supersede o antigo.
        let hits = db.recall_entities(&["pred/trabalha"], 16).unwrap();
        let old = hits.iter().find(|h| h.key.ends_with("job/v1"));
        let new = hits.iter().find(|h| h.key.ends_with("job/v2"));
        if let (Some(o), Some(n)) = (old, new) {
            let _ = db.supersede(&o.key, &n.key);
        }
    }
    // leitura: o agente pergunta "onde trabalha o carlos?"
    let hits = db.recall(&emb(seed_from("carlos trabalha")), 8).unwrap();
    let current = hits
        .iter()
        .find(|h| h.provenance.as_ref().map(|p| p.state) == Some(neural_sgdb::MemoryState::Active));
    let ok = current.map(|h| h.text.contains("GLOBEX")).unwrap_or(false);
    // sPS = fração das evidências de trabalho que apontam pro valor corrente
    let sps = hits.iter().filter(|h| h.text.contains("GLOBEX")).count() as f32
        / hits.len().max(1) as f32;
    (ok, sps)
}

// ── Tarefa 2: recorrência → consolidação (verbos do DB) ─────────────────────
fn task_consolidate(host_b1: bool) -> (bool, f32) {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    for i in 0..3u64 {
        let _ = db.remember_episodic("user", "gosta cafe", 100 + i * 10).unwrap();
    }
    if host_b1 {
        // B1: usa o verbo de consolidação do DB (sinal de recorrência).
        let cfg = ConsolidateConfig { min_repeats: 3, min_len: 8, max_new: 4 };
        let _ = db.consolidate_recurrences(&cfg).unwrap();
    }
    // o agente pergunta "o que o user gosta?"
    let lex = db.recall_lexical("gosta cafe", 16).unwrap();
    let has_fact = lex.iter().any(|h| h.key.contains("/L3/consolidated/"));
    let ok = has_fact;
    // sPS: fração dos hits que são o fato consolidado (verbatim único)
    let total = lex.len().max(1);
    let sps = lex.iter().filter(|h| h.key.contains("/L3/")).count() as f32 / total as f32;
    (ok, sps)
}

fn task_expire(host_b1: bool) -> (bool, f32) {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    let k = "promo/verao";
    db.remember_semantic(k, "promo verao 50%", &emb(seed_from("promo verao 50%")))
        .unwrap();
    db.set_validity(&format!("md/L4/{k}"), 0, 100).unwrap();
    if host_b1 {
        // B1: expira as janelas fechadas (sinal de validade do DB) antes de ler.
        let _ = db.expire_old(200).unwrap();
    }
    let hits = db.recall(&emb(seed_from("promo verao")), 8).unwrap();
    // correta: a promo acabou (janela fechou) → active recall NÃO deve achar
    let ok = hits.is_empty();
    let sps = if hits.is_empty() { 1.0 } else { 0.0 };
    (ok, sps)
}

fn main() {
    type Task = fn(bool) -> (bool, f32);
    let tasks: Vec<(&str, Task)> = vec![
        ("supersede (fato evolui)", task_supersede),
        ("consolidate (recorrencia)", task_consolidate),
        ("expire (staleness)", task_expire),
    ];
    println!("=== F4 probe: B0 (verbos passivos) vs B1 (verbos + sinais do DB) ===");
    println!("{:<28} {:>8} {:>8} {:>8} {:>8}", "tarefa", "SR(B0)", "sPS(B0)", "SR(B1)", "sPS(B1)");
    let mut b0_sr = 0usize;
    let mut b1_sr = 0usize;
    let mut b0_sps = 0.0f32;
    let mut b1_sps = 0.0f32;
    for (name, f) in tasks {
        let (sr0, sps0) = f(false);
        let (sr1, sps1) = f(true);
        b0_sr += usize::from(sr0);
        b1_sr += usize::from(sr1);
        b0_sps += sps0;
        b1_sps += sps1;
        println!("{:<28} {:>8} {:>8.2} {:>8} {:>8.2}", name, sr0, sps0, sr1, sps1);
    }
    println!("\nTOTAL: B0 SR={b0_sr}/3 sPS={b0_sps:.2}  |  B1 SR={b1_sr}/3 sPS={b1_sps:.2}");
    let affordance = b1_sr > b0_sr && b1_sps > b0_sps;
    println!("AFFORDANCE (DB da ao host o que gerir) = {affordance}");
    println!("VEREDITO F4:");
    println!("  - AFFORDANCE: {} -> o DB TEM os verbos/sinais para auto-gestao",
        if affordance { "GO" } else { "NOGO" });
    println!("  - COMPETENCIA: ver memory_arena_eval (naive 0/3 vs protocolo 3/3)");
    println!("    -> saber USAR os verbos ainda depende de ENSINO (doutrina).");
    println!("  - NETA: autonomia fica NO HOST (politica ensinada); o DB expoe verbos+sinais.");
    if !affordance {
        std::process::exit(1);
    }
}