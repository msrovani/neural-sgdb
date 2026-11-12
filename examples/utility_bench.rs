//! utility_bench — medição de utilidade real (Fase 3)
//!
//! Compara lexical/semantic/hybrid/path por *qualidade de decisão*, não hit count.
//! Cenários: perguntas exatas, paráfrases, evidências conflitantes, escopos, evolução temporal.
//! Corpora de pelo menos dois domínios (anônimos/representativos).
//! Registra latência e volume de candidatos junto à qualidade.

use neural_sgdb::{InMemory, Sgdb};
use std::time::Instant;

#[derive(Clone)]
struct Case {
    domain: &'static str,
    query: &'static str,
    gold_keys: &'static [&'static str],
    need: usize,
}

fn emb(seed: u64) -> Vec<f32> {
    let mut s = seed.wrapping_mul(1103515245).wrapping_add(12345);
    let mut v = Vec::with_capacity(32);
    for _ in 0..32 {
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

fn main() {
    println!("=== utility_bench (Fase 3) — decisão, não hit count ===");
    
    // Domínio 1: infra/ops
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    db.set_default_scope(Some("project/infra".into()));
    
    // memória base
    db.remember_text_with("ops/runbook/deploy", 
        "Deploy: usar pipeline azul-verde, rollback em 5min se health falhar", 
        neural_sgdb::RememberOptions { scope: Some("project/infra"), entities: &["runbook/deploy"], content_type: None, scope_dims: None, model_id: None, index_key: false }).unwrap();
    db.remember_semantic_with("ops/fact/rollback", 
        "Rollback automático após 3 falhas consecutivas de health check", 
        &emb(seed_from("rollback health")), 
        neural_sgdb::RememberOptions { scope: Some("project/infra"), entities: &["runbook/rollback"], content_type: None, scope_dims: None, model_id: None, index_key: false }).unwrap();
    // evidência conflitante
    db.remember_text_with("ops/old/deploy", 
        "Deploy: reiniciar todos os pods (método antigo, depreciado)", 
        neural_sgdb::RememberOptions { scope: Some("project/infra"), entities: &["runbook/deploy"], content_type: None, scope_dims: None, model_id: None, index_key: false }).unwrap();
    // escopo diferente (não deve vazar)
    db.remember_text_with("ops/other/secret", 
        "Token interno do projeto X", 
        neural_sgdb::RememberOptions { scope: Some("project/other"), entities: &["secret/token"], content_type: None, scope_dims: None, model_id: None, index_key: false }).unwrap();
    
    let cases = vec![
        Case { domain: "infra", query: "como fazer deploy?", gold_keys: &["ops/runbook/deploy"], need: 3 },
        Case { domain: "infra", query: "deploy procedure", gold_keys: &["ops/runbook/deploy"], need: 3 }, // paráfrase
        Case { domain: "infra", query: "rollback health check", gold_keys: &["ops/fact/rollback"], need: 3 },
        Case { domain: "infra", query: "deploy strategy", gold_keys: &["ops/runbook/deploy"], need: 3 },
    ];
    
    println!("\n[Domínio 1: infra/ops]");
    for c in &cases {
        let t0 = Instant::now();
        let hits_lex = db.recall_lexical_scoped(c.query, c.need, "project/infra").unwrap();
        let lex_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let t1 = Instant::now();
        let hits_sem = db.recall_scoped(&emb(seed_from(c.query)), c.need, "project/infra").unwrap();
        let sem_ms = t1.elapsed().as_secs_f64() * 1000.0;
        
        let lex_ok = hits_lex.iter().any(|h| c.gold_keys.contains(&h.key.as_str()));
        let sem_ok = hits_sem.iter().any(|h| c.gold_keys.contains(&h.key.as_str()));
        
        // `domain` entra na linha: era campo nunca lido (dead_code sob `-D warnings`)
        // e o rotulo pertence a saida — dois dominios no mesmo relatorio.
        println!("  [{}] q='{}' | lexical: {} ({:.2}ms, {}cands) | semantic: {} ({:.2}ms, {}cands)",
            c.domain, c.query, if lex_ok {"OK"} else {"MISS"}, lex_ms, hits_lex.len(),
            if sem_ok {"OK"} else {"MISS"}, sem_ms, hits_sem.len());
    }
    
    // Domínio 2: produto/docs
    let mut db2 = Sgdb::open(InMemory::new()).unwrap();
    db2.remember_text_with("docs/api/auth", 
        "Auth: usar Bearer token, tokens expiram em 1h", 
        neural_sgdb::RememberOptions { scope: Some("product/docs"), entities: &["api/auth"], content_type: None, scope_dims: None, model_id: None, index_key: false }).unwrap();
    db2.remember_text_with("docs/api/rate", 
        "Rate limit: 100 req/min por chave", 
        neural_sgdb::RememberOptions { scope: Some("product/docs"), entities: &["api/ratelimit"], content_type: None, scope_dims: None, model_id: None, index_key: false }).unwrap();
    
    println!("\n[Domínio 2: produto/docs]");
    let q2 = "como autenticar na api?";
    let t0 = Instant::now();
    let h2 = db2.recall_lexical_scoped(q2, 3, "product/docs").unwrap();
    let ms2 = t0.elapsed().as_secs_f64() * 1000.0;
    let ok2 = h2.iter().any(|h| h.key == "docs/api/auth");
    println!("  q='{}' | lexical: {} ({:.2}ms, {}cands)", q2, if ok2 {"OK"} else {"MISS"}, ms2, h2.len());
    
    // Teste de isolamento de escopo
    let global = db.recall_lexical("token interno", 5).unwrap();
    let leaked = global.iter().any(|h| h.key == "ops/other/secret");
    println!("\n[Isolamento de escopo] global recall não vaza escopo: {}", if !leaked {"OK"} else {"LEAK"});
    
    println!("\n=== Fim ===");
}
