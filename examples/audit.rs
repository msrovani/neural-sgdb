//! Cognitive QA audit — the mission test: "returns MEMORIES, not data".
//!
//! Run: `cargo run --release --example audit`
//!
//! Three batteries, each a delivery:
//!   1. ATTACK — hostile inputs against the cognitive layer (this file's main
//!      focus): malformed keys (`/`, `#`, prefix collisions), hostile
//!      embeddings (NaN/Inf/empty/wrong-dims), invalid states, side-table
//!      overwrite, relations with missing keys.
//!   2. CORRUPTION — end-to-end bit-rot: corrupt bytes mid-file, then
//!      `rebuild_indices` reconciles and `validate()` reports.
//!   3. FIDELITY — recall returns MEMORIES (provenance/state/layer/lineage),
//!      `forget` archives (history kept, recall ignores), `supersede` builds a
//!      DAG, validity window gates recall, importance ranks.
//!
//! Every assertion prints PASS/FAIL; exit code 0 iff all pass.

use std::time::Instant;

use neural_sgdb::{FileStorage, MemoryState, Sgdb};

/// Deterministic embeddings for reproducible attacks.
fn emb(seed: u64) -> Vec<f32> {
    let mut s = seed.wrapping_mul(1103515245).wrapping_add(12345);
    let mut v = vec![0f32; 64];
    for x in v.iter_mut() {
        s = s.wrapping_mul(1103515245).wrapping_add(12345);
        *x = ((s >> 32) as i32 % 200) as f32 / 100.0 - 1.0;
    }
    v
}

struct Report {
    checks: Vec<(String, bool, String)>,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: String) {
        println!("{} {}", if ok { "PASS" } else { "FAIL" }, name);
        if !ok {
            println!("      {detail}");
        }
        self.checks.push((name.to_string(), ok, detail));
    }
    fn finish(self) -> i32 {
        let fails = self.checks.iter().filter(|(_, ok, _)| !ok).count();
        println!(
            "\n=== AUDIT (battery 1: ATTACK) ===\nasserções: {} total, {} falhas",
            self.checks.len(),
            fails
        );
        i32::from(fails > 0)
    }
}

fn main() {
    let dir = std::env::temp_dir().join("neural_sgdb_audit");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("attack.db");
    let _ = std::fs::remove_file(&path);

    println!("=== AUDIT battery 1: ATTACK (camada cognitiva) ===");
    let mut rep = Report { checks: Vec::new() };
    let t = Instant::now();

    let mut db = Sgdb::open(FileStorage::open(&path).unwrap()).unwrap();

    // ── 1.1 embeddings hostis ───────────────────────────────────────────────
    rep.check("NaN embedding rejeitado", db.remember_semantic("h/nan", "x", &[f32::NAN, 0.0]).is_err(), "".into());
    rep.check("Inf embedding rejeitado", db.remember_semantic("h/inf", "x", &[f32::INFINITY, 1.0]).is_err(), "".into());
    rep.check("-Inf embedding rejeitado", db.remember_semantic("h/ninf", "x", &[f32::NEG_INFINITY]).is_err(), "".into());
    rep.check("embedding vazio rejeitado", db.remember_semantic("h/empty", "x", &[]).is_err(), "".into());
    rep.check("recall com NaN rejeitado", db.recall(&[f32::NAN], 5).is_err(), "".into());
    // query vazia = sem candidatos → Ok(vazio) (deliberado, sem panic)
    let r = db.recall(&[], 5);
    rep.check("recall vazio → Ok(vazio)", r.as_ref().map(|h| h.is_empty()).unwrap_or(false), format!("{r:?}"));
    rep.check("rag_context com NaN rejeitado", db.rag_context(&[f32::NAN], 3).is_err(), "".into());

    // ── 1.2 chaves maliciosas ───────────────────────────────────────────────
    // chave com `/` crua (mã do resolve_storage_key): `md/{key}` cria subpath
    let r = db.remember_semantic("path/inject", "conteudo", &emb(1));
    rep.check("chave com / aceita (md/path/inject)", r.is_ok(), format!("{r:?}"));
    // `#` separador reservado de L6 — associate deve rejeitar
    rep.check("associate rejeita #", db.associate("a#b", neural_sgdb::RelationKind::RelatedTo, "c").is_err(), "".into());
    // chave com prefixo de sistema `sys/` crua vira `md/L4/sys/inject`
    // (inofensivo — não colide com side-tables reais `sys/state/` etc.)
    let r = db.remember_semantic("sys/inject", "ok", &emb(2));
    rep.check("remember em sys/ é doc normal (md/L4/sys/inject)", r.is_ok(), format!("{r:?}"));
    // chave prefixo-colision (ART não suporta prefix keys) — engine::put guarda
    let r1 = db.remember_semantic("coll/abc", "um", &emb(3));
    let r2 = db.remember_semantic("coll/abcd", "dois", &emb(4));
    rep.check("prefix-key coll/abc vs coll/abcd rejeitada por has_prefix_conflict",
        r1.is_ok() && r2.is_err(), format!("{r1:?} {r2:?}"));

    // ── 1.3 estados inválidos / side-table overwrite ────────────────────────
    // set_state em chave que não existe — NÃO deve criar side-table órfã
    // (achado do hot-test: validate pegava órfãs). AUDIT: engine::set_state
    // agora recusa estado ≠ Active para chave sem doc.
    let r = db.set_state("md/L4/nao-existe", MemoryState::Archived);
    rep.check("set_state em chave inexistente rejeitado", r.is_err(), format!("{r:?}"));
    // set_importance com NaN → Invalid
    let r = db.remember_semantic("h/imp", "imp", &emb(5));
    rep.check("remember ok para h/imp", r.is_ok(), "".into());
    let r = db.set_importance("md/L4/h/imp", f32::NAN);
    rep.check("set_importance NaN → Invalid", r.is_err(), format!("{r:?}"));
    let r = db.set_importance("md/L4/h/imp", 1.5);
    rep.check("set_importance 1.5 clampada a [0,1]", r.is_ok(), "".into());
    let imp = db.meta("md/L4/h/imp").unwrap().unwrap().importance;
    rep.check("importance clampada == 1.0", (imp - 1.0).abs() < 1e-6, format!("imp={imp}"));
    // set_state em chave válida continua funcionando (regressão do fix)
    let r = db.set_state("md/L4/h/imp", MemoryState::Decayed);
    rep.check("set_state em chave existente ok", r.is_ok(), format!("{r:?}"));

    // ── 1.4 recall com k=0 / k gigante ─────────────────────────────────────
    let r = db.recall(&emb(10), 0);
    rep.check("recall k=0 não panics (ok/vazio)", r.is_ok(), format!("{r:?}"));
    let r = db.recall(&emb(10), usize::MAX);
    rep.check("recall k=MAX não panics (ok)", r.is_ok(), format!("{r:?}"));

    // ── 1.5 relations com chave inexistente ────────────────────────────────
    // DESIGN: relação é afirmada pela camada superior — o doc NÃO precisa
    // existir (teste relations_do_not_require_docs). Só `#` e prefix-conflict
    // são rejeitados.
    let r = db.associate("md/L4/ghost-a", neural_sgdb::RelationKind::Causes, "md/L4/ghost-b");
    rep.check("associate com chaves inexistentes é design (ok)", r.is_ok(), format!("{r:?}"));
    // ...mas supersede/merge dependem de docs reais → Err
    let r = db.supersede("md/L4/ghost-old", "md/L4/ghost-new");
    rep.check("supersede com chaves inexistentes rejeitado", r.is_err(), format!("{r:?}"));
    let r = db.merge_memories("md/L4/ghost-a", "md/L4/ghost-b", "");
    rep.check("merge_memories com inexistentes rejeitado", r.is_err(), format!("{r:?}"));

    // ── 1.6 integridade final pós-ataque ───────────────────────────────────
    let issues = db.validate();
    rep.check("validate: nenhuma side-table órfã pós-ataque", issues.is_empty(),
        issues.iter().map(|i| format!("[{}] {}", i.key, i.message)).collect::<Vec<_>>().join("; "));

    println!("battery 1 (attack) em {} ms", t.elapsed().as_millis());
    let _ = std::fs::remove_file(&path);
    std::process::exit(rep.finish());
}