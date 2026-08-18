//! Era migration — ADR-0007 analytical benchmark.
//!
//! Run: `cargo run --release --example era_migration_bench`
//!
//! Mede o custo (tempo de processamento, por fase) da **MIGRAÇÃO POR RE-EMBED**
//! (ADR-0007): o texto está preservado nos companions `/L2/` ⇒ varre os docs da
//! era antiga, re-embutida com o modelo NOVO, reescreve payload+bitvec, reseta o
//! BQ (width novo) via `rebuild_indices()` e devolve semântica real ao passado.
//!
//! Era OLD = `DemoEmbedder` (256-dim, trigram — o "modelo antigo"); era NEW =
//! `EraEmbedder` (384-dim, trigram com seed diferente — o "novo modelo"
//! SIMULADO; um modelo real entraria pelo MESMO trait `Embedder`, o custo do
//! modelo em si é EXTERNO ao crate — ver `examples/embedder_http.rs`).
//!
//! Demonstra e mede:
//! 1. **Width-lock trap** (puro BQ, API pública): escrever um vetor 384-dim num
//!    BQ já travado em 4 words TRUNCA silenciosamente — dois vetores distintos
//!    viram indistinguíveis (v1 é devolvido como distância 0 para v2).
//! 2. **Custo por fase** da migração (FileStorage): scan `md/L4/` → leitura dos
//!    companions L2 → re-embed (crate-side) → reescrita payload+bitvec →
//!    rebuild do BQ.
//! 3. **Ressurreição**: após a migração, uma query re-embutida de um doc da era
//!    antiga retorna o PRÓPRIO doc (dist ≈ 0); a era antiga (256-dim) passa a
//!    **errar ALTO** no recall semântico (S1) e continua acessível por
//!    `recall_lexical` (rede de recuperação embedding-free); `validate()` limpo;
//!    identidade (`memory_id`) preservada no overwrite.

use std::time::{Duration, Instant};

use neural_sgdb::bq::{quantize_f32, BqFlatIndex};
use neural_sgdb::embedder::demo_embed;
use neural_sgdb::{Embedder, FileStorage, MemoryDoc, MemoryLayer, Sgdb, SgdbError};

/// "Novo modelo": trigram FNV com offset basis DIFERENTE do `demo_embed` e
/// 384 dims (6 words × 64 bits). Só simula o outro modelo — um modelo real
/// pluga pelo mesmo trait.
struct EraEmbedder;

impl Embedder for EraEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, SgdbError> {
        Ok(era_embed(text))
    }
}

fn era_embed(text: &str) -> Vec<f32> {
    const DIM: usize = 384;
    let mut v = vec![0f32; DIM];
    let bytes = text.as_bytes();
    let windows: Vec<&[u8]> = if bytes.len() < 3 {
        bytes.iter().map(core::slice::from_ref).collect()
    } else {
        bytes.windows(3).collect()
    };
    for w in windows {
        // FNV-1a com offset basis diferente do demo_embed → "outro modelo".
        let mut h = 0x9e37_79b9_7f4a_7c15u64;
        for &b in w {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
        let idx = (h % DIM as u64) as usize;
        v[idx] += if (h >> 8) & 1 == 1 { 1.0 } else { -1.0 };
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-8);
    v.iter_mut().for_each(|x| *x /= norm);
    v
}

fn percentiles(mut samples: Vec<Duration>) -> (Duration, Duration) {
    samples.sort();
    let p = |q: f64| {
        let idx = ((samples.len() as f64) * q) as usize;
        let idx = idx.min(samples.len().saturating_sub(1));
        samples[idx]
    };
    (p(0.50), p(0.99))
}

/// Width-lock trap (bughunt #11 / ADR-0007), demonstrado com API pública.
///
/// v0 (era antiga, 256-dim) trava `words_per_vec = 4`. v1 e v2 são vetores
/// 384-dim DIFERENTES (idênticos nos primeiros 256 dims, opostos nos 128
/// restantes) — ao entrar no BQ travado, ambos são TRUNCADOS para os primeiros
/// 4 words e viram indistinguíveis: uma query v2 devolve v1 com distância 0.
fn width_lock_trap_demo() {
    let mk = |pos: bool| {
        let mut v = vec![0f32; 384];
        for x in v.iter_mut().take(256) {
            *x = 0.5; // primeiros 4 words: idênticos
        }
        for x in v.iter_mut().skip(256) {
            *x = if pos { 1.0 } else { -1.0 }; // words 5–6: opostos
        }
        v
    };
    let v1 = mk(true);
    let v2 = mk(false);

    // ── sem lock (BQ fresco, 384 entra primeiro) → correto ──────────────
    let mut fresh = BqFlatIndex::new();
    fresh.insert_f32(1, &v1);
    fresh.insert_f32(2, &v2);
    let top = fresh.top_k_f32(&v2, 1);
    let (id, score) = top[0];
    assert_eq!(id, 2, "BQ fresco: query v2 → v2 (score {score})");
    assert_eq!(score, 0);

    // ── com lock (256-dim entra primeiro; v1/v2 truncados) → corrompido ─
    let mut locked = BqFlatIndex::new();
    locked.insert_f32(0, &vec![-0.5f32; 256]); // era antiga trava 4 words
    locked.insert_f32(1, &v1); // truncado para os 4 primeiros words
    locked.insert_f32(2, &v2); // idem — v1 e v2 agora têm o MESMO bitvec
    let top = locked.top_k_f32(&v2, 1);
    let (id, score) = top[0];
    assert_eq!(id, 1, "BQ travado: query v2 → v1 com score {score} (v2 sumiu)");
    assert_eq!(score, 0, "v2 é reportado como v1 com distância 0 — lixo silencioso");
    println!(
        "width-lock trap  : BQ fresco top-1=doc2 (certo) vs BQ travado top-1=doc1 score=0 (v2 indistinguível de v1 — vetor truncado)"
    );
}

fn main() {
    const N: usize = 2000;
    let db_path = std::env::temp_dir().join("neural_sgdb_era_migration.db");
    let _ = std::fs::remove_file(&db_path);

    println!("era migration bench — N={N} docs, FileStorage (ADR-0007)");
    println!("era OLD: DemoEmbedder 256-dim | era NEW: EraEmbedder 384-dim (simulado)");
    println!();

    width_lock_trap_demo();
    println!();

    // ── Fase 0 — escrever a era antiga ───────────────────────────────────
    let mut db = Sgdb::open(FileStorage::open(&db_path).unwrap()).unwrap();
    let t0 = Instant::now();
    for i in 0..N {
        let text = format!(
            "memoria antiga numero {i}: o valor do parametro alpha e {i} unidade beta gamma delta"
        );
        let emb = demo_embed(&text);
        db.remember_semantic(&format!("mem-{i:06}"), &text, &emb)
            .unwrap();
    }
    let t_write_old = t0.elapsed();
    assert_eq!(db.indexed_embedding_dims(), vec![256]);
    println!("fase 0 — write era OLD : {t_write_old:?} total ({:?}/doc) indexed_dims={:?}",
        t_write_old / N as u32, db.indexed_embedding_dims());

    // sanity: era antiga funciona semanticamente (query 256-dim → doc self)
    let k0 = 17usize;
    let text0 = db
        .get(MemoryLayer::L2EpisodicShort, &format!("mem-{k0:06}"))
        .unwrap()
        .unwrap();
    let old_emb0 = demo_embed(&String::from_utf8_lossy(&text0.payload));
    let hit = db.recall(&old_emb0, 1).unwrap().remove(0);
    assert_eq!(hit.key, format!("md/L4/mem-{k0:06}"), "era OLD recall funciona");
    assert!(hit.dist < 0.05, "dist self era OLD = {}", hit.dist);
    println!("  sanity era OLD: recall 256-dim → top-1 = self (dist {:.4})", hit.dist);

    // ── Fase 1 — scan da era antiga ──────────────────────────────────────
    let t0 = Instant::now();
    let l4 = db.scan_prefix("md/L4/").unwrap();
    let t_scan = t0.elapsed();
    assert_eq!(l4.len(), N);
    let ids: Vec<String> = l4
        .iter()
        .map(|(sk, _)| sk.trim_start_matches("md/L4/").to_string())
        .collect();
    println!("fase 1 — scan md/L4/   : {t_scan:?} ({N} keys)");

    // ── Fase 2 — leitura dos textos companions L2 (a fonte da migração) ──
    let t0 = Instant::now();
    let texts: Vec<String> = ids
        .iter()
        .map(|id| {
            let doc = db.get(MemoryLayer::L2EpisodicShort, id).unwrap().unwrap();
            String::from_utf8_lossy(&doc.payload).into_owned()
        })
        .collect();
    let t_read = t0.elapsed();
    println!(
        "fase 2 — read companions: {t_read:?} total ({:?}/doc)",
        t_read / N as u32
    );

    // ── Fase 3 — re-embed com o modelo novo (crate-side) ─────────────────
    // O custo do MODELO REAL é externo (rede/inferência); aqui mede-se só o
    // custo de invocar o trait + alocar o vetor — o piso do lado do crate.
    let embedder = EraEmbedder;
    let mut t_embed = Vec::with_capacity(N);
    let mut new_embs: Vec<Vec<f32>> = Vec::with_capacity(N);
    for text in &texts {
        let t0 = Instant::now();
        new_embs.push(embedder.embed(text).unwrap());
        t_embed.push(t0.elapsed());
    }
    let (p50, p99) = percentiles(t_embed);
    println!(
        "fase 3 — re-embed (sim) : P50={p50:?} P99={p99:?}/doc — modelo REAL é externo (embedder_http.rs)"
    );

    // identidade antes (prova da preservação no overwrite)
    let mid_before = db
        .get(MemoryLayer::L4Semantic, &format!("mem-{k0:06}"))
        .unwrap()
        .unwrap()
        .meta
        .unwrap()
        .memory_id;

    // ── Guard de escrita (ADR-0007): remember_semantic é LOUD ─────────────
    // O write-side era guard rejeita dim nova num corpus vivo (width-lock
    // truncaria em silêncio). A migração deliberada usa o put cru + rebuild.
    let guarded = db.remember_semantic(&format!("mem-{k0:06}"), &texts[0], &new_embs[0]);
    assert!(matches!(guarded, Err(SgdbError::Invalid(_))), "guard rejeita dim nova: {guarded:?}");
    assert_eq!(
        db.indexed_embedding_dims(),
        vec![256],
        "nada foi escrito pelo caminho guarded"
    );
    println!("guard de escrita: remember_semantic com dim nova → SgdbError::Invalid (nada escrito; migração = put cru + rebuild)");

    // ── Fase 4 — reescrita payload+bitvec (overwrite preserva identidade) ─
    // Caminho CRU (engine.put): o put em chave existente preserva memory_id/
    // source/created e bumpeia a versão (overwrite = nova versão da MESMA
    // memória). remember_semantic (guarded) NÃO é usado de propósito.
    let mut t_rewrite = Vec::with_capacity(N);
    for (i, id) in ids.iter().enumerate() {
        let t0 = Instant::now();
        let mut payload = Vec::with_capacity(new_embs[i].len() * 4);
        for x in &new_embs[i] {
            payload.extend_from_slice(&x.to_le_bytes());
        }
        let mut doc = MemoryDoc::new(MemoryLayer::L4Semantic, id, payload);
        doc.bitvec = Some(quantize_f32(&new_embs[i]));
        db.put(doc).unwrap();
        let tdoc = MemoryDoc::new(MemoryLayer::L2EpisodicShort, id, texts[i].as_bytes().to_vec());
        db.put(tdoc).unwrap();
        t_rewrite.push(t0.elapsed());
    }
    let (p50, p99) = percentiles(t_rewrite);
    println!(
        "fase 4 — rewrite payload: P50={p50:?} P99={p99:?}/doc (FileStorage append + meta + index)"
    );

    // mid-state: dims misturados (payloads já 384, BQ ainda travado em 4 words)
    let mid_dims = db.indexed_embedding_dims();
    assert_eq!(mid_dims, vec![256, 384], "estado misto antes do rebuild");
    println!(
        "  mid-state: indexed_dims={mid_dims:?} — BQ ainda na largura da era antiga (width-lock); queries 384 SÓ serão íntegras após o rebuild"
    );

    // era_report (mid-state): detecção da era mista + custo estimado
    let rep_mid = db.era_report().unwrap();
    assert_eq!(rep_mid.verdict, "mixed_dims", "era_report detecta a mistura de dims");
    println!("era_report (mid-state): verdict={} — docs={} text={} bytes est. db-side={:.1} ms",
        rep_mid.verdict, rep_mid.estimate.docs_to_reembed, rep_mid.text_bytes,
        rep_mid.estimate.db_side_ns as f64 / 1e6);

    let mid_after = db
        .get(MemoryLayer::L4Semantic, &format!("mem-{k0:06}"))
        .unwrap()
        .unwrap()
        .meta
        .unwrap()
        .memory_id;
    assert_eq!(mid_after, mid_before, "overwrite preserva memory_id (identidade estável)");
    println!("  identidade preservada no overwrite: memory_id estável (chave mem-{k0:06})");

    // ── Fase 5 — reset do BQ (width novo) via rebuild ────────────────────
    let t0 = Instant::now();
    let n = db.rebuild_indices().unwrap();
    let t_rebuild = t0.elapsed();
    assert_eq!(n, N + N, "rebuild reindexa L4 (embeddings) + L2 (textos)");
    assert_eq!(db.indexed_embedding_dims(), vec![384], "BQ agora na largura da era nova");
    println!(
        "fase 5 — rebuild BQ    : {t_rebuild:?} ({n} docs reindexados) indexed_dims={:?}",
        db.indexed_embedding_dims()
    );

    // era_report (pós-rebuild): corpus íntegro numa era só + custo estimado
    let rep = db.era_report().unwrap();
    assert_eq!(rep.verdict, "ok", "após o rebuild o corpus volta a ser uma era única");
    assert_eq!(rep.docs_per_dim, vec![(384, 2000)]);
    assert!((rep.companion_coverage - 1.0).abs() < 1e-9, "100%% de texto preservado");
    println!(
        "era_report (final): verdict={} docs_per_dim={:?} coverage={:.3} est. db-side={:.1} ms (N={} docs) — a LLM multiplica pelo custo do modelo externo",
        rep.verdict, rep.docs_per_dim, rep.companion_coverage,
        rep.estimate.db_side_ns as f64 / 1e6, rep.estimate.docs_to_reembed
    );
    for l in db.era_report_lines().unwrap() {
        println!("    {l}");
    }

    // ── Ressurreição: semântica real de volta ao passado ──────────────────
    let mut hits_ok = 0usize;
    let stride = N / 40;
    for (i, id) in ids.iter().step_by(stride).enumerate() {
        let q = &new_embs[i * stride];
        let hit = db.recall(q, 1).unwrap().remove(0);
        if hit.key == format!("md/L4/{id}") && hit.dist < 0.05 {
            hits_ok += 1;
        }
    }
    println!("ressurreição: recall 384-dim top-1 = self em {hits_ok}/40 docs (dist ≈ 0)");
    assert_eq!(hits_ok, 40, "todo doc da era antiga recupera semântica real");

    // ── Era antiga orfanada (contrato P4 / guard S1, agora LOUD) ─────────
    let err = db.recall(&old_emb0, 1).unwrap_err();
    assert!(
        matches!(err, SgdbError::Invalid(_)),
        "query 256-dim após migração → erro alto (S1), não lixo silencioso"
    );
    println!("era OLD orfanada: query 256-dim → SgdbError::Invalid (guard S1 — nunca lixo silencioso)");

    // ── Rede embedding-free: o passado continua acessível ────────────────
    let lex = db.recall_lexical(&format!("memoria antiga numero {k0}"), 1).unwrap();
    assert!(
        lex.iter().any(|h| h.key == format!("md/L4/mem-{k0:06}") || h.key == format!("md/L2/mem-{k0:06}")),
        "recall_lexical ainda encontra a era antiga"
    );
    println!("rede embedding-free: recall_lexical encontra a memória da era antiga (texto preservado)");

    assert!(db.validate().is_empty(), "validate limpo após a migração");
    println!("validate: limpo após a migração");

    let _ = std::fs::remove_file(&db_path);

    println!();
    println!("Total era OLD write: {t_write_old:?} | scan {t_scan:?} | read {t_read:?} | rewrite P50 {:.2}µs/doc | rebuild {t_rebuild:?}",
        p50.as_secs_f64() * 1e6);
    println!("Nota: compact() (FileStorage) reclama os blobs antigos pós-migração (append-only).");
    println!("Metodologia/hardware: ver BENCHMARKS.md (ADR-0007 section).");
}
