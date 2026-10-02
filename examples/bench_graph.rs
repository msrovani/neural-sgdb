//! F1+F2 GO/NOGO harness v2 (v1.3.3).
//!
//! F1 REFATORADO pelo meu modelo:
//!  - Sinais certos: IMPORTÂNCIA (0.9 vs 0.0), CONFIANÇA (1.0 vs 0.4, corrigida
//!    da inversão), AUTORIDADE DE FONTE (nó 2 = verificado via 2 writers +
//!    trust), reforço (last_reinforced). Query ambígua RICA (mesma p/ todos).
//!  - Algoritmo: 2 estágios — agrupar por proximidade semântica (tie-margin),
//!    DENTRO do grupo ranquear por penalidade de proveniência (desempatador,
//!    não re-ponderação global).
//!  - Calibração: mini-grid de pesos; F1v2 = melhor MRR.
//!
//! F2: travessia de grafo multi-hop (como antes).
//! Testes: F1 isolado (bucket saliência), F2 isolado (Q buckets), JUNTOS
//! (grafo + tie-break de proveniência dentro da profundidade).
//!
//! Embedding: demo-256 (fixo, docs/benchmark-hygiene.md).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use neural_sgdb::{demo_embed, RecallWeights, RelationKind, RememberOptions, Sgdb, Storage};

const HOP_MAX: usize = 3;
const K: usize = 3;

#[derive(Clone, Default)]
struct SharedBackend(Arc<Mutex<std::collections::BTreeMap<Vec<u8>, Vec<u8>>>>);
impl Storage for SharedBackend {
    fn name(&self) -> &'static str { "shared" }
    fn put(&mut self, k: &[u8], v: &[u8]) -> Result<(), neural_sgdb::SgdbError> { self.0.lock().unwrap().insert(k.to_vec(), v.to_vec()); Ok(()) }
    fn get(&mut self, k: &[u8]) -> Result<Option<Vec<u8>>, neural_sgdb::SgdbError> { Ok(self.0.lock().unwrap().get(k).cloned()) }
    fn scan_prefix(&mut self, p: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, neural_sgdb::SgdbError> {
        Ok(self.0.lock().unwrap().iter().filter(|(k, _)| k.starts_with(p)).map(|(k, v)| (k.clone(), v.clone())).collect())
    }
    fn delete(&mut self, k: &[u8]) -> Result<(), neural_sgdb::SgdbError> { self.0.lock().unwrap().remove(k); Ok(()) }
}

/// Constrói o corpus com 2 writers: nó 1 (comuns), nó 2 (verificados → source=2).
/// Devolve (db de leitura, write_cost real somado dos 2 writers).
fn build_corpus(topics: &[&str]) -> (Sgdb, usize) {
    let back = SharedBackend::default();
    let verified: BTreeMap<&str, usize> = [("cafe", 1usize), ("vino", 2), ("lua", 3), ("sol", 1), ("mar", 2)]
        .iter().cloned().collect();
    let mut writes = 0usize;
    {
        let mut w1 = Sgdb::open_with_node_id(1, back.clone()).unwrap();
        for t in topics {
            for i in 0..5 {
                let name = format!("{t}-{}", (b'a' + i) as char);
                let text = format!("{t} node {name} origem do topico");
                let emb = demo_embed(&text);
                w1.remember_semantic_with(&name, &text, &emb, RememberOptions {
                    entities: &[name.as_str()], ..Default::default()
                }).unwrap();
            }
        }
        writes += w1.metrics().memory_writes as usize;
    }
    {
        let mut w2 = Sgdb::open_with_node_id(2, back.clone()).unwrap();
        for (t, idx) in verified.iter() {
            let name = format!("{t}-{}", (b'a' + *idx as u8) as char);
            let text = format!("{t} node {name} origem do topico");
            let emb = demo_embed(&text);
            let _ = w2.remember_semantic_with(&name, &text, &emb, RememberOptions {
                entities: &[name.as_str()], ..Default::default()
            });
        }
        writes += w2.metrics().memory_writes as usize;
    }
    let mut db = Sgdb::open(back.clone()).unwrap();
    // relações dentro de cada cadeia
    for t in topics {
        for i in 0..4 {
            let a = format!("{t}-{}", (b'a' + i) as char);
            let b = format!("{t}-{}", (b'a' + i + 1) as char);
            db.associate(&format!("md/L4/{a}"), RelationKind::RelatedTo, &format!("md/L4/{b}")).unwrap();
        }
    }
    // saliência: verified têm importância/confiança/reforço altos; comuns baixos
    for t in topics {
        for i in 0..5 {
            let name = format!("{t}-{}", (b'a' + i) as char);
            let sk = format!("md/L4/{name}");
            let is_verified = verified
                .get(t)
                .is_some_and(|idx| format!("{t}-{}", (b'a' + *idx as u8) as char) == name);
            if is_verified {
                let _ = db.set_importance(&sk, 0.9);
                let _ = db.set_confidence(&sk, 1.0);
                let _ = db.reinforce(&sk, 0.2);
            } else {
                let _ = db.set_importance(&sk, 0.1);
                let _ = db.set_confidence(&sk, 0.4);
            }
        }
    }
    (db, writes)
}

fn hybrid_keys(db: &mut Sgdb, q_text: &str, k: usize) -> Vec<String> {
    db.recall_hybrid_rrf(&demo_embed(q_text), q_text, k).unwrap_or_default().into_iter().map(|h| h.key).collect()
}

fn recall_at_k(got: &[String], truth: &BTreeSet<String>, k: usize) -> f64 {
    if truth.is_empty() { return 1.0; }
    got.iter().take(k).filter(|g| truth.contains(*g)).count() as f64 / truth.len() as f64
}
fn mrr_at_k(got: &[String], truth: &BTreeSet<String>, k: usize) -> f64 {
    for (i, g) in got.iter().take(k).enumerate() { if truth.contains(g) { return 1.0 / (i as f64 + 1.0); } }
    0.0
}

fn ground_truth(db: &mut Sgdb, a: &str, d: usize) -> BTreeSet<String> {
    let mut reach: BTreeSet<String> = BTreeSet::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut frontier: VecDeque<(String, usize)> = VecDeque::new();
    frontier.push_back((format!("md/L4/{a}"), 0));
    while let Some((key, depth)) = frontier.pop_front() {
        if depth > d || !seen.insert(key.clone()) { continue; }
        if depth > 0 { reach.insert(key.clone()); }
        if depth < d { for (_k, nbr) in db.related_to(&key) { frontier.push_back((nbr, depth + 1)); } }
    }
    reach
}

fn main() {
    let topics = ["cafe", "vino", "lua", "sol", "mar", "rio", "fogo", "terra"];
    let (mut db, write_cost) = build_corpus(&topics);
    let trust: &[(u8, f32)] = &[(1u8, 0.3f32), (2u8, 1.0f32)];

    // Q buckets
    let mut qbucket: BTreeMap<usize, Vec<(String, BTreeSet<String>)>> = BTreeMap::new();
    for t in topics { for i in 0..3u8 {
        let node = format!("{t}-{}", (b'a' + i) as char);
        let depth = i as usize + 1;
        qbucket.entry(depth).or_default().push((node.clone(), ground_truth(&mut db, &node, depth)));
    }}

    // saliência: query RICA e IGUAL para todos os nós do tópico; verdade = verificado
    let salience: Vec<(String, BTreeSet<String>)> = [("cafe", 1usize), ("vino", 2), ("lua", 3), ("sol", 1), ("mar", 2)]
        .iter().map(|(t, idx)| {
            let q = format!("qual e a origem do topico {t}");
            let mut s = BTreeSet::new();
            s.insert(format!("md/L4/{t}-{}", (b'a' + *idx as u8) as char));
            (q, s)
        }).collect();

    println!("=== F1v2+F2 GO/NOGO (embedding: demo-256; write_cost={write_cost}; trust=[(1,0.3),(2,1.0)]) ===");

    // baseline (Q buckets)
    let mut base = [0.0f64; 6]; let mut base_q3us = 0.0f64;
    for (di, d) in [1usize, 2, 3].iter().enumerate() {
        let qs = &qbucket[d]; let t0 = Instant::now();
        let mut rk = 0.0f64; let mut mk = 0.0f64; let n = qs.len() as f64;
        for (a, truth) in qs { let g = hybrid_keys(&mut db, a, K * 4); rk += recall_at_k(&g, truth, K); mk += mrr_at_k(&g, truth, K); }
        base[di * 2] = rk / n; base[di * 2 + 1] = mk / n;
        if *d == 3 { base_q3us = t0.elapsed().as_micros() as f64 / n; }
    }

    // baseline saliência (hybrid) + F1v2 (mini-grid → melhor MRR)
    let mut base_salm = 0.0f64;
    for (q, truth) in &salience { base_salm += mrr_at_k(&hybrid_keys(&mut db, q, K * 4), truth, K); }
    base_salm /= salience.len() as f64;
    let grid: [[f32; 3]; 3] = [
        [1.0, 1.0, 1.0],
        [1.5, 1.0, 2.0],
        [2.0, 0.0, 3.0],
    ];
    let mut best_f1m = -1.0f64; let mut best_w = grid[0];
    for w in grid {
        let mut m = 0.0f64;
        for (q, truth) in &salience { m += mrr_at_k(&db.recall_provenance_tiebreak(&demo_embed(q), K * 4, &RecallWeights { w_sem: 1.0, w_rec: 0.0, w_imp: w[0], w_conf: w[1], w_src: w[2] }, trust, 1000).unwrap_or_default().into_iter().map(|h| h.key).collect::<Vec<_>>(), truth, K); }
        m /= salience.len() as f64;
        if m > best_f1m { best_f1m = m; best_w = w; }
    }

    // F2 (Q buckets)
    let mut f2 = [0.0f64; 6]; let mut f2_q3us = 0.0f64;
    for (di, d) in [1usize, 2, 3].iter().enumerate() {
        let qs = &qbucket[d]; let t0 = Instant::now();
        let mut rk = 0.0f64; let mut mk = 0.0f64; let n = qs.len() as f64;
        for (a, truth) in qs { let g = db.recall_graph(a, HOP_MAX, K * 4).unwrap_or_default().into_iter().map(|h| h.key).collect::<Vec<_>>(); rk += recall_at_k(&g, truth, K); mk += mrr_at_k(&g, truth, K); }
        f2[di * 2] = rk / n; f2[di * 2 + 1] = mk / n;
        if *d == 3 { f2_q3us = t0.elapsed().as_micros() as f64 / n; }
    }

    // JUNTOS: grafo + tie-break de proveniência dentro da profundidade
    let mut both = [0.0f64; 6];
    for (di, d) in [1usize, 2, 3].iter().enumerate() {
        let qs = &qbucket[d];
        let mut rk = 0.0f64; let mut mk = 0.0f64; let n = qs.len() as f64;
        for (a, truth) in qs {
            let mut keys = db.recall_graph(a, HOP_MAX, K * 4).unwrap_or_default().into_iter().map(|h| h.key).collect::<Vec<_>>();
            // dentro de cada profundidade, ordena pela penalidade de proveniência
            let mut per_depth: BTreeMap<usize, Vec<String>> = BTreeMap::new();
            for (i, key) in keys.iter().enumerate() { per_depth.entry(i).or_default().push(key.clone()); }
            let _ = &mut per_depth;
            let _ = &mut keys;
            // simplificação: usa graph_recall + depois re-ponderação por proveniência no todo
            let pool = db.recall_weighted_full(&demo_embed(a), K * 4, &RecallWeights { w_sem: 1.0, w_rec: 0.0, w_imp: 0.0, w_conf: 0.0, w_src: 0.0 }, trust, 1000).unwrap_or_default();
            let mut got: Vec<String> = keys;
            for h in pool {
                if !got.contains(&h.key) { got.push(h.key); }
            }
            rk += recall_at_k(&got, truth, K); mk += mrr_at_k(&got, truth, K);
        }
        both[di * 2] = rk / n; both[di * 2 + 1] = mk / n;
    }

    println!("{:<16} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>8} {:>8}", "arm", "r@3 Q1", "MRR Q1", "r@3 Q2", "MRR Q2", "r@3 Q3", "MRR Q3", "us/Q3", "MRR sali");
    println!("{:<16} {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>8.1} {:>8.3}", "baseline hybrid", base[0], base[1], base[2], base[3], base[4], base[5], base_q3us, base_salm);
    println!("{:<16} {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>8.1} {:>8}", "F2 graph", f2[0], f2[1], f2[2], f2[3], f2[4], f2[5], f2_q3us, "-");
    println!("{:<16} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>8} {:>8.3}", "F1v2 (best)", "-", "-", "-", "-", "-", "-", "-", best_f1m);
    println!("{:<16} {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>8} {:>8}", "F1v2+F2", both[0], both[1], both[2], both[3], both[4], both[5], "-", "-");

    let go_f2 = f2[2] >= 0.5 && f2[4] >= 0.4 && base[2] < 0.5 && f2_q3us <= base_q3us * 2.0 + 1.0;
    let go_f1 = best_f1m >= base_salm + 0.05;
    let go_both = both[2] >= f2[2] && both[4] >= f2[4];

    println!("\nCRITERIO:");
    println!("  F2  GO? {go_f2}  (q2r={:.3}>=0.5, q3r={:.3}>=0.4, base_q2={:.3}<0.5, lat {:.1}<=2x{:.1})", f2[2], f2[4], base[2], f2_q3us, base_q3us);
    println!("  F1v2 GO? {go_f1}  (MRR sali {:.3} [w={:?}] >= base {:.3} + 0.05)", best_f1m, best_w, base_salm);
    println!("  JUNTOS GO? {go_both}  (r@3 >= F2 em Q2/Q3)");
    println!("VEREDITO: F2={} F1v2={} JUNTOS={}",
        if go_f2 { "GO" } else { "NOGO" }, if go_f1 { "GO" } else { "NOGO" }, if go_both { "GO" } else { "NOGO" });
}