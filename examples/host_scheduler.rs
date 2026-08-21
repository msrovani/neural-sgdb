//! Host scheduler — camada superior que governa a memória sem tocar o core.
//!
//! O core não decide quando esquecer/consolidar/expirar — ele expõe
//! `tick`/`decay_importance`/`consolidate_recurrences`/`expire_old`/
//! `audit_checkpoint`. Este daemon é o "cérebro que usa a memória" no lado host:
//! chama cada um no tempo certo, com política configurável, sem `static` global.
//!
//! Run: `cargo run --release --example host_scheduler --features file-storage`
//! (usa FileStorage em `./.nsgdb/scheduler.db` por padrão)

use neural_sgdb::{ConsolidateConfig, DecayConfig, FileStorage, Sgdb};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::var("NEURAL_SGDB_DB").unwrap_or_else(|_| ".nsgdb/scheduler.db".into());
    let backend = FileStorage::open(&path)?;
    let mut db = Sgdb::open(backend)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;

    // 1. Expira janelas de validade fechadas (Graphiti/Zep: invalidar-não-deletar)
    let expired = db.expire_old(now)?;
    if expired > 0 {
        println!("[scheduler] expire_old: {expired} memórias invalidadas");
    }

    // 2. Decay Ebbinghaus — importância decai com a idade (item 1 v1.1.10)
    let decay_cfg = DecayConfig {
        half_life_ms: 7 * 24 * 3600 * 1000, // 7 dias (host escolhe, não o core)
        floor: 0.05,
        decay_state_at: 0.05,
        decay_confidence: true,
    };
    let decayed = db.decay_importance(now, &decay_cfg)?;
    if decayed > 0 {
        println!("[scheduler] decay_importance: {decayed} memórias decaídas");
    }

    // 3. Consolidação por recorrência — L2 verbatim repetido → L3 fato (item 2)
    let cons_cfg = ConsolidateConfig {
        min_repeats: 3,
        min_len: 24,
        max_new: 16,
    };
    let consolidated = db.consolidate_recurrences(&cons_cfg)?;
    if consolidated > 0 {
        println!("[scheduler] consolidate_recurrences: {consolidated} fatos novos");
    }

    // 4. Auditoria hash-chain + checkpoint (item 5)
    let seq = db.audit_checkpoint(now)?;
    let report = db.audit_verify()?;
    println!(
        "[scheduler] audit_checkpoint seq={seq} chain_intact={} digest_matches={} entries={}",
        report.chain_intact, report.digest_matches_last, report.entries
    );

    // 5. Health check — o que o operador vê
    let health = db.health();
    println!(
        "[scheduler] health: docs={} bq_len={} conflicts={} validate_issues={}",
        health.doc_count,
        health.bq_len,
        health.open_conflicts,
        db.validate().len()
    );

    // Auto-checks (determinístico, sem LLM)
    assert!(report.chain_intact, "audit chain quebrou");
    assert!(report.digest_matches_last, "digest diverge logo após checkpoint");
    println!("[scheduler] 5/5 checks PASS — memória governada, IAs podem trabalhar");

    Ok(())
}
