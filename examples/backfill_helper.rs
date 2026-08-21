//! Backfill helper — migra L3 lexical (sem vetor) → L4 semântico via re-embed do texto preservado em /L2/.
//!
//! O core nunca gera embedding (contrato). Este helper é a camada host que re-embeda
//! o texto verbatim guardado e reescreve o doc no mesmo id (overwrite preserva
//! `memory_id`) + `rebuild_indices()` para resetar a largura do BQ.
//! É o caminho "guarded NÃO serve" da era migration (ADR-0007, BENCHMARKS.md §Era).
//!
//! Run: `cargo run --release --example backfill_helper --features file-storage`

use neural_sgdb::embedder::{DemoEmbedder, Embedder};
use neural_sgdb::{FileStorage, MemoryDoc, MemoryLayer, Sgdb};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::var("NEURAL_SGDB_DB").unwrap_or_else(|_| ".nsgdb/backfill.db".into());
    let backend = FileStorage::open(&path)?;
    let mut db = Sgdb::open(backend)?;
    let embedder = DemoEmbedder;

    // 1. Inventário: L3 docs
    let l3_keys = db.scan_prefix("md/L3/")?;
    println!("[backfill] L3 docs encontrados: {}", l3_keys.len());
    if l3_keys.is_empty() {
        println!("[backfill] nada para migrar — grave alguns L3 via remember_text_with primeiro");
        return Ok(());
    }

    // 2. Para cada L3, lê o doc e re-embeda o payload
    let mut migrated = 0usize;
    let mut skipped = 0usize;
    for (key, _) in l3_keys.iter().take(20) {
        let id_part = key.strip_prefix("md/L3/").unwrap_or(key);
        // Tenta ler como L3
        let doc_opt = db.get(MemoryLayer::L3EpisodicLong, id_part)?;
        let Some(doc) = doc_opt else {
            skipped += 1;
            continue;
        };
        let text = String::from_utf8_lossy(&doc.payload).to_string();
        if text.len() < 8 {
            skipped += 1;
            continue;
        }
        let emb = embedder.embed(&text)?;
        let mut new_doc = MemoryDoc::new(
            MemoryLayer::L4Semantic,
            &format!("md/L4/{id_part}"),
            doc.payload.clone(),
        );
        new_doc.clock = doc.clock.clone();
        // quantiza manualmente para o demo (mesma lógica do BqFlatIndex)
        let bits: Vec<u64> = {
            let emb_len = emb.len();
            let mut out = vec![0u64; emb_len.div_ceil(64)];
            for (i, &x) in emb.iter().enumerate() {
                if x > 0.0 {
                    out[i / 64] |= 1u64 << (i % 64);
                }
            }
            out
        };
        new_doc.bitvec = Some(bits);
        // put cru preserva memory_id por chave
        match db.put(new_doc) {
            Ok(_) => migrated += 1,
            Err(e) => {
                eprintln!("[backfill] skip {id_part}: {e}");
                skipped += 1;
            }
        }
    }

    // 3. Rebuild dos índices — reseta largura do BQ para nova dim
    let t0 = std::time::Instant::now();
    let n = db.rebuild_indices()?;
    let elapsed = t0.elapsed();
    println!("[backfill] migrados: {migrated} L3→L4, ignorados: {skipped}, rebuild: {n} docs em {elapsed:?}");

    let lines = db.era_report_lines()?;
    for l in lines.iter().take(6) {
        println!("[backfill] era: {l}");
    }
    println!("[backfill] validate: {} issues", db.validate().len());
    println!("[backfill] done");

    Ok(())
}
