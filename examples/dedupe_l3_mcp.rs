//! One-off: remove a duplicata md/L3/mcp/1790975709980000 (item 13 do
//! relatório da sessão 1.4.5). Mantém o doc mais recente (…535000),
//! registra o forget_purge na audit chain.
//! Rodar: `cargo run --example dedupe_l3_mcp`.
use neural_sgdb::{FileStorage, Sgdb};

const DB: &str = ".nsgdb/memory.db";
const KEEP: &str = "1790975715535000";
const DROP: &str = "1790975709980000";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = Sgdb::open(FileStorage::open(DB)?)?;

    let keys: Vec<String> = db.scan_prefix("md/L3/mcp/")?.into_iter().map(|(k, _)| k).collect();
    println!("keys sob md/L3/mcp/: {:?}", keys);

    let keep_key = format!("md/L3/mcp/{}", KEEP);
    let drop_key = format!("md/L3/mcp/{}", DROP);
    if !keys.contains(&drop_key) {
        println!("duplicata {} não existe — nada a fazer.", drop_key);
        return Ok(());
    }

    let keep_text = db.text_of(&keep_key)?;
    let drop_text = db.text_of(&drop_key)?;
    println!("KEEP = {:?}\nDROP = {:?}", keep_text, drop_text);

    // Guarda: mesmo ASSUNTO (curadoria do DB) — os textos são paráfrases.
    // Segue o precedente do próprio banco ("forget arquivou ... história
    // preservada"): arquivar a mais ANTIGA, não purge.
    let drop_l = drop_text.to_lowercase();
    let keep_l = keep_text.to_lowercase();
    for termo in ["curadoria", "audit_checkpoint"] {
        if !drop_l.contains(termo) || !keep_l.contains(termo) {
            eprintln!("ABORT: termos \"{}\" ausentes — nada arquivado.", termo);
            return Ok(());
        }
    }

    db.forget(&drop_key)?;
    println!("forget (arquivado): {} — mantido {}", drop_key, keep_key);
    Ok(())
}
