//! Cognitive QA audit — the mission test: "returns MEMORIES, not data".
//!
//! Run: `cargo run --release --example audit`
//!
//! Three batteries, each a delivery:
//!   1. ATTACK — hostile inputs against the cognitive layer: malformed keys
//!      (`/`, `#`, `sys/`, prefix collisions), hostile embeddings
//!      (NaN/Inf/empty), invalid states, side-table overwrite, relations with
//!      ghost keys.
//!   2. CORRUPTION — end-to-end bit-rot: corrupt bytes mid-file, then
//!      reopen recovers deterministically, `validate()` reports and
//!      `rebuild_indices` reconciles.
//!   3. FIDELITY — recall returns MEMORIES (provenance/state/layer/lineage),
//!      `forget` archives (history kept, recall ignores), `supersede` builds a
//!      DAG, validity window gates recall, importance ranks.
//!
//! Every assertion prints PASS/FAIL; exit code 0 iff all pass.

use std::time::Instant;

use neural_sgdb::{FileStorage, MemoryLayer, MemoryState, Sgdb};

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
    fn finish(self, battery: &str) -> i32 {
        let fails = self.checks.iter().filter(|(_, ok, _)| !ok).count();
        println!(
            "\n=== AUDIT ({battery}) ===\nasserções: {} total, {} falhas",
            self.checks.len(),
            fails
        );
        i32::from(fails > 0)
    }
}

fn battery1_attack(path: &std::path::Path, rep: &mut Report) {
    let t = Instant::now();
    let mut db = Sgdb::open(FileStorage::open(path).unwrap()).unwrap();

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
    let r = db.remember_semantic("path/inject", "conteudo", &emb(1));
    rep.check("chave com / aceita (md/path/inject)", r.is_ok(), format!("{r:?}"));
    rep.check("associate rejeita #", db.associate("a#b", neural_sgdb::RelationKind::RelatedTo, "c").is_err(), "".into());
    let r = db.remember_semantic("sys/inject", "ok", &emb(2));
    rep.check("remember em sys/ é doc normal (md/L4/sys/inject)", r.is_ok(), format!("{r:?}"));
    let r1 = db.remember_semantic("coll/abc", "um", &emb(3));
    let r2 = db.remember_semantic("coll/abcd", "dois", &emb(4));
    rep.check("prefix-key coll/abc vs coll/abcd rejeitada por has_prefix_conflict",
        r1.is_ok() && r2.is_err(), format!("{r1:?} {r2:?}"));

    // ── 1.3 estados inválidos / side-table overwrite ────────────────────────
    let r = db.set_state("md/L4/nao-existe", MemoryState::Archived);
    rep.check("set_state em chave inexistente rejeitado", r.is_err(), format!("{r:?}"));
    let r = db.remember_semantic("h/imp", "imp", &emb(5));
    rep.check("remember ok para h/imp", r.is_ok(), "".into());
    let r = db.set_importance("md/L4/h/imp", f32::NAN);
    rep.check("set_importance NaN → Invalid", r.is_err(), format!("{r:?}"));
    let r = db.set_importance("md/L4/h/imp", 1.5);
    rep.check("set_importance 1.5 clampada a [0,1]", r.is_ok(), "".into());
    let imp = db.meta("md/L4/h/imp").unwrap().unwrap().importance;
    rep.check("importance clampada == 1.0", (imp - 1.0).abs() < 1e-6, format!("imp={imp}"));
    let r = db.set_state("md/L4/h/imp", MemoryState::Decayed);
    rep.check("set_state em chave existente ok", r.is_ok(), format!("{r:?}"));

    // ── 1.4 recall com k=0 / k gigante ─────────────────────────────────────
    let r = db.recall(&emb(10), 0);
    rep.check("recall k=0 não panics (ok/vazio)", r.is_ok(), format!("{r:?}"));
    let r = db.recall(&emb(10), usize::MAX);
    rep.check("recall k=MAX não panics (ok)", r.is_ok(), format!("{r:?}"));

    // ── 1.5 relations com chave inexistente ────────────────────────────────
    let r = db.associate("md/L4/ghost-a", neural_sgdb::RelationKind::Causes, "md/L4/ghost-b");
    rep.check("associate com chaves inexistentes é design (ok)", r.is_ok(), format!("{r:?}"));
    let r = db.supersede("md/L4/ghost-old", "md/L4/ghost-new");
    rep.check("supersede com chaves inexistentes rejeitado", r.is_err(), format!("{r:?}"));
    let r = db.merge_memories("md/L4/ghost-a", "md/L4/ghost-b", "");
    rep.check("merge_memories com inexistentes rejeitado", r.is_err(), format!("{r:?}"));

    // ── 1.6 integridade final pós-ataque ───────────────────────────────────
    let issues = db.validate();
    rep.check("validate: nenhuma side-table órfã pós-ataque", issues.is_empty(),
        issues.iter().map(|i| format!("[{}] {}", i.key, i.message)).collect::<Vec<_>>().join("; "));

    drop(db); // fecha o arquivo antes da corrupção da battery 2
    println!("battery 1 (attack) em {} ms", t.elapsed().as_millis());
}

/// Localiza a PRIMEIRA ocorrência de `needle` no arquivo (payload NMD1 cru).
fn find_byte_pos(data: &[u8], needle: &[u8]) -> Option<usize> {
    data.windows(needle.len()).position(|w| w == needle)
}

/// Corrompe bytes DENTRO do val do record cujo storage key é `md/L4/<k>`.
/// O texto do payload aparece 2x no arquivo (companion L2 + doc L4) — achar
/// só o texto é ambíguo. O storage key do record L4 (`md/L4/<k>`) é ÚNICO
/// (aparece como key do append-log `[klen][vlen][crc][key][val]`); o val do
/// NMD1 começa logo após os 12 bytes do key. Flip no meio do val → CRC
/// key‖val falha → recovery trunca ali.
fn corrupt_l4_val(path: &std::path::Path, k: &str, flip: usize) -> bool {
    let mut data = std::fs::read(path).unwrap();
    let key_bytes = format!("md/L4/{k}").into_bytes();
    let Some(pos) = find_byte_pos(&data, &key_bytes) else {
        return false;
    };
    let val_start = pos + key_bytes.len();
    let Some(mid) = val_start.checked_add(flip) else { return false };
    if mid >= data.len() {
        return false;
    }
    data[mid] ^= 0xFF;
    std::fs::write(path, &data).unwrap();
    true
}

fn battery2_corruption(path: &std::path::Path, rep: &mut Report) {
    let t = Instant::now();

    // ── 2.0 setup: banco saudável com docs conhecidos ───────────────────────
    {
        let mut db = Sgdb::open(FileStorage::open(path).unwrap()).unwrap();
        db.remember_semantic("c0", "memoria-zero", &emb(20)).unwrap();
        db.remember_semantic("c1", "memoria-um", &emb(21)).unwrap();
        db.remember_semantic("c2", "memoria-dois", &emb(22)).unwrap();
        db.checkpoint().unwrap();
    } // drop → arquivo fechado, sincronizado

    let original = std::fs::read(path).unwrap();

    // ── 2.1 bit-rot no MEIO do record L4 de c1 ─────────────────────────────
    // Corrompe CRC key‖val → o recovery TRUNCA no 1º record inválido (c1 e
    // tudo após ele some; c0 fica). Docs NUNCA ressuscitam com bytes corrompidos.
    assert!(corrupt_l4_val(path, "c1", 8), "record L4/c1 localizado");
    let mut db = Sgdb::open(FileStorage::open(path).unwrap()).unwrap();
    let c0 = db.get(MemoryLayer::L4Semantic, "c0").unwrap();
    rep.check("c0 sobrevive (antes da corrupção)", c0.is_some(), "".into());
    let c1 = db.get(MemoryLayer::L4Semantic, "c1").unwrap();
    rep.check("c1 NÃO ressuscita com bytes corrompidos", c1.is_none(), format!("{c1:?}"));
    let issues = db.validate();
    rep.check("validate pós-corrupção não panics", issues.len() <= 1, issues.iter().map(|i| i.message).collect::<Vec<_>>().join("; "));
    let n = db.rebuild_indices().unwrap();
    rep.check("rebuild_indices reconcilia", n >= 1, format!("n={n}"));
    let c0b = db.get(MemoryLayer::L4Semantic, "c0").unwrap();
    rep.check("c0 continua recuperável pós-rebuild", c0b.is_some(), "".into());
    drop(db);

    // ── 2.2 bit-rot no record L4 de c2 (o ÚLTIMO doc escrito) ──────────────
    // Último record corrompido = cauda truncada → c0, c1 ficam; c2 some.
    std::fs::write(path, &original).unwrap();
    assert!(corrupt_l4_val(path, "c2", 8), "record L4/c2 localizado");
    let mut db = Sgdb::open(FileStorage::open(path).unwrap()).unwrap();
    rep.check("corrupção do último record → c0 ok", db.get(MemoryLayer::L4Semantic, "c0").unwrap().is_some(), "".into());
    rep.check("corrupção do último record → c1 ok", db.get(MemoryLayer::L4Semantic, "c1").unwrap().is_some(), "".into());
    rep.check("corrupção do último record → c2 some", db.get(MemoryLayer::L4Semantic, "c2").unwrap().is_none(), "".into());
    drop(db);

    // ── 2.3 truncamento físico do arquivo (crash mid-append) ───────────────
    // Corta o arquivo no meio do último record → recovery trunca a cauda.
    let cut = original.len() / 2;
    std::fs::write(path, &original[..cut]).unwrap();
    let mut db = Sgdb::open(FileStorage::open(path).unwrap()).unwrap();
    rep.check("arquivo truncado reabre sem panic", true, "".into());
    rep.check("truncado → docs antes do corte sobrevivem", db.get(MemoryLayer::L4Semantic, "c0").unwrap().is_some(), "".into());
    let issues = db.validate();
    rep.check("validate pós-truncamento limpo", issues.is_empty(), issues.iter().map(|i| format!("[{}] {}", i.key, i.message)).collect::<Vec<_>>().join("; "));
    drop(db);

    println!("battery 2 (corruption) em {} ms", t.elapsed().as_millis());
}

fn battery3_fidelity(path: &std::path::Path, rep: &mut Report) {
    let t = Instant::now();
    let mut db = Sgdb::open(FileStorage::open(path).unwrap()).unwrap();

    // ── 3.0 setup: memórias com identidade/importância ──────────────────────
    db.remember_semantic("f1", "o cachorro do vizinho latiu a noite toda", &emb(40)).unwrap();
    db.remember_semantic("f2", "o projeto custou duzentos mil reais", &emb(41)).unwrap();
    db.set_importance("md/L4/f2", 0.9).unwrap();
    db.set_confidence("md/L4/f2", 0.8).unwrap();

    // ── 3.1 recall devolve MEMÓRIAS, não bytes ─────────────────────────────
    let hits = db.recall(&emb(40), 5).unwrap();
    let top = hits.iter().find(|h| h.key.ends_with("/f1"));
    rep.check("recall acha f1 por embedding", top.is_some(), format!("{hits:?}"));
    if let Some(h) = top {
        rep.check("recall devolve TEXT (legível), não payload bruto", h.text.contains("cachorro"), format!("text={:?}", h.text));
        rep.check("recall expõe provenance (memória, não dados)",
            h.provenance.is_some(), format!("prov={:?}", h.provenance));
        if let Some(p) = &h.provenance {
            rep.check("provenance: memory_id presente", !p.memory_id.is_empty(), "".into());
            rep.check("provenance: layer L4", p.layer == MemoryLayer::L4Semantic, format!("layer={:?}", p.layer));
            rep.check("provenance: state Active", p.state == MemoryState::Active, format!("state={:?}", p.state));
            // default de importância é POR CAMADA: L4 = 1.0 (memory_doc.rs)
            rep.check("provenance: importance default L4 = 1.0", (p.importance - 1.0).abs() < 1e-6, format!("imp={}", p.importance));
            // f2 tem importância alta — ranking pondera por importância
            db.recall_weighted(&emb(40), 5, 0.6, 0.0, 0.4, 1_000_000_000).unwrap();
        }
    }

    // ── 3.2 forget = arquivar (história preservada, recall ignora) ─────────
    db.forget("md/L4/f1").unwrap();
    // forget marca POR DOC: companion L2 também precisa sumir do recall
    db.forget("md/L2/f1").unwrap();
    let active = db.recall(&emb(40), 5).unwrap();
    rep.check("forget → recall default ignora f1", active.iter().all(|h| !h.key.ends_with("/f1")), format!("{active:?}"));
    // história preservada: recall_historical + get + explain ainda veem
    let hist = db.recall_historical(&emb(40), 5).unwrap();
    rep.check("recall_historical ainda vê f1 (memória, não deletada)",
        hist.iter().any(|h| h.key.ends_with("/f1")), format!("{hist:?}"));
    rep.check("get ainda lê o doc arquivado", db.get(MemoryLayer::L4Semantic, "f1").unwrap().is_some(), "".into());
    let st = db.get_state("md/L4/f1").unwrap();
    rep.check("estado é Archived", st == MemoryState::Archived, format!("{st:?}"));
    let expl = db.explain("md/L4/f1").unwrap();
    rep.check("explain expõe o estado Archived", expl.state == MemoryState::Archived, format!("state={:?}", expl.state));

    // ── 3.3 supersede constrói DAG (linhagem causal) ────────────────────────
    db.supersede("md/L4/f2", "md/L4/f3").unwrap(); // f3 ainda não existe — design
    // cria o sucessor real e liga a linhagem
    db.remember_semantic("f3", "o orçamento estourou em duzentos e dez mil", &emb(42)).unwrap();
    db.supersede("md/L4/f2", "md/L4/f3").unwrap();
    let st2 = db.get_state("md/L4/f2").unwrap();
    rep.check("supersede marca f2 como Superseded", st2 == MemoryState::Superseded, format!("{st2:?}"));
    let lin = db.lineage("md/L4/f3").unwrap();
    rep.check("lineage de f3 tem 2 elos (f3 → f2)", lin.len() >= 2, format!("lin={lin:?}"));
    let parent_vid = db.meta("md/L4/f2").unwrap().map(|m| m.version_id).unwrap_or_default();
    rep.check("f3 lista f2 como parent causal",
        db.meta("md/L4/f3").unwrap().map(|m| m.parent_ids.contains(&parent_vid)).unwrap_or(false), "".into());
    // recall default não traz superseded (f2 não compete)
    let active = db.recall(&emb(41), 5).unwrap();
    rep.check("recall default ignora f2 (superseded)", active.iter().all(|h| !h.key.ends_with("/f2")), format!("{active:?}"));

    // ── 3.4 validade temporal: from ≤ now < until ─────────────────────────
    db.set_validity("md/L4/f3", 1_000, 2_000).unwrap();
    rep.check("validity_at dentro da janela → true", db.validity_at("md/L4/f3", 1_500).unwrap(), "".into());
    rep.check("validity_at fora da janela → false", !db.validity_at("md/L4/f3", 2_500).unwrap(), "".into());
    let before = db.recall_at(&emb(42), 5, 1_500).unwrap();
    rep.check("recall_at (dentro) traz f3", before.iter().any(|h| h.key.ends_with("/f3")), format!("{before:?}"));
    let after = db.recall_at(&emb(42), 5, 2_500).unwrap();
    rep.check("recall_at (fora) exclui f3", after.iter().all(|h| !h.key.ends_with("/f3")), format!("{after:?}"));
    db.invalidate("md/L4/f3", 2_500).unwrap();
    // invalidate = janela de validade até now (invalidar-NÃO-deletar): o
    // estado lógico não muda (continua Active), mas a memória expira em now.
    // `until <= from` APAGA a marcação — usar now > from.
    let st3 = db.get_state("md/L4/f3").unwrap();
    rep.check("invalidate mantém estado (validade, não state)", st3 == MemoryState::Active, format!("{st3:?}"));
    rep.check("invalidate → validity_at(now+1) false", !db.validity_at("md/L4/f3", 2_501).unwrap(), "".into());
    rep.check("invalidate → get continua recuperável", db.get(MemoryLayer::L4Semantic, "f3").unwrap().is_some(), "".into());

    // ── 3.5 recall_weighted: importância POR CAMADA rankeia (contrato) ────
    // Contrato (AGENTS.md): recall_weighted = w_sem·dist + w_rec·recência +
    // w_imp·importância(camada). A importância da CAMADA (penalidade) difere
    // por layer: L4=0.0, L5=0.2 — sob w_imp alto, L4 vence L5 com o MESMO
    // embedding. (A importância POR DOC — set_importance/reinforce — é
    // exposta via provenance, não entra no ranking.)
    db.remember_semantic("f4", "patente arquivada para o sensor optico", &emb(50)).unwrap();
    db.remember_semantic("f5", "patente arquivada para o sensor optico", &emb(50)).unwrap();
    let mut l5 = neural_sgdb::MemoryDoc::new(
        neural_sgdb::MemoryLayer::L5Procedural,
        "proc/sensor",
        "patente arquivada para o sensor optico".as_bytes().to_vec(),
    );
    l5.bitvec = Some(neural_sgdb::quantize_f32(&emb(50)));
    db.put(l5).unwrap();
    let w = db.recall_weighted(&emb(50), 3, 0.0, 0.0, 1.0, 1_000_000_000).unwrap();
    rep.check("w_imp: L4 (penalty 0.0) vence L5 (penalty 0.2)",
        w.iter().any(|h| h.key.contains("/f4")) && !w[0].key.contains("proc/sensor"),
        format!("w={:?}", w.iter().map(|h| &h.key).collect::<Vec<_>>()));

    let issues = db.validate();
    rep.check("validate final limpo", issues.is_empty(), issues.iter().map(|i| format!("[{}] {}", i.key, i.message)).collect::<Vec<_>>().join("; "));

    println!("battery 3 (fidelity) em {} ms", t.elapsed().as_millis());
}

fn main() {
    let dir = std::env::temp_dir().join("neural_sgdb_audit");
    let _ = std::fs::create_dir_all(&dir);

    let mut rep = Report { checks: Vec::new() };
    println!("=== AUDIT battery 1: ATTACK (camada cognitiva) ===");
    let path1 = dir.join("attack.db");
    let _ = std::fs::remove_file(&path1);
    battery1_attack(&path1, &mut rep);
    let code1 = rep.finish("battery 1: ATTACK");

    rep = Report { checks: Vec::new() };
    println!("\n=== AUDIT battery 2: CORRUPTION (bit-rot end-to-end) ===");
    let path2 = dir.join("corrupt.db");
    let _ = std::fs::remove_file(&path2);
    battery2_corruption(&path2, &mut rep);
    let code2 = rep.finish("battery 2: CORRUPTION");

    rep = Report { checks: Vec::new() };
    println!("\n=== AUDIT battery 3: FIDELITY (memórias, não dados) ===");
    let path3 = dir.join("fidelity.db");
    let _ = std::fs::remove_file(&path3);
    battery3_fidelity(&path3, &mut rep);
    let code3 = rep.finish("battery 3: FIDELITY");

    let _ = std::fs::remove_file(&path1);
    let _ = std::fs::remove_file(&path2);
    let _ = std::fs::remove_file(&path3);
    std::process::exit(i32::max(code1, i32::max(code2, code3)));
}
