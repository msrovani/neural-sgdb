//! AI-user simulation (audit): 500-turn agent loop over FileStorage.
//! Measures per-turn wall time (remember + recall + rag) and recall quality:
//! exact-words vs paraphrase, canonical L2+L4 via demo embedder (trigram).

use neural_sgdb::{Embedder, Sgdb, FileStorage};
use std::time::Instant;
use std::env::temp_dir;

fn main() {
    let n: usize = std::env::var("N").ok().and_then(|s| s.parse().ok()).unwrap_or(500);
    let dir = temp_dir().join("nsgdb_agent_sim");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("agent.db");
    let _ = std::fs::remove_file(&path);

    let emb = neural_sgdb::DemoEmbedder;
    let e = |t: &str| emb.embed(t).unwrap();
    let mut db = Sgdb::open(FileStorage::open(&path).unwrap()).unwrap();
    db.set_default_scope(Some("user/ana".into()));

    let mut turn_ms = Vec::with_capacity(n);
    let mut remember_ms = 0.0f64;
    let mut recall_ms = 0.0f64;
    let mut rag_ms = 0.0f64;

    for i in 0..n {
        let user = format!("How do I deploy service-{i} to the staging cluster?");
        let asst = format!("Run the pipeline deploy-{i} then verify the staging health endpoint.");

        let t0 = Instant::now();
        // grava como o MCP remember(text=) faz: L3 texto + entidades
        db.remember_text_with(
            &format!("fact/deploy/{i:05}"),
            &format!("service-{i} deploys via pipeline deploy-{i} (staging)"),
            neural_sgdb::RememberOptions {
                scope: Some("user/ana"),
                entities: &["service/fixed"],
                content_type: None,
                scope_dims: None,
                model_id: None,
            },
        ).unwrap();
        // episódico verbatim
        db.remember_episodic(&user, &asst, 1000 + i as u64).unwrap();
        // L4 semântico (era demo 256-dim)
        db.remember_semantic_with(
            &format!("sm/deploy/{i:05}"),
            &format!("service-{i} staging pipeline deploy-{i}"),
            &e(&format!("service-{i} staging pipeline deploy-{i}")),
            neural_sgdb::RememberOptions {
                scope: Some("user/ana"),
                entities: &["service/fixed"],
                content_type: Some("text"),
                scope_dims: None,
                model_id: None,
            },
        ).unwrap();
        let t1 = Instant::now();

        // recall lexical (default MCP)
        let q = format!("deploy service-{i}");
        let hits_lex = db.recall_lexical_scoped(&q, 5, "user/ana").unwrap();
        // recall semântico (com embedding)
        let hits_sem = db.recall_scoped(&e(&q), 5, "user/ana").unwrap();
        // RAG context compilado
        let _ctx = db.rag_context(&e(&q), 3).unwrap();
        let t2 = Instant::now();

        let _ = (hits_lex, hits_sem);
        turn_ms.push(t0.elapsed().as_secs_f64() * 1e3);
        remember_ms += (t1 - t0).as_secs_f64() * 1e3;
        recall_ms += (t2 - t1).as_secs_f64() * 1e3;
        rag_ms += t2.elapsed().as_secs_f64() * 1e3;
    }

    turn_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p = |f: f64| turn_ms[((f * (n - 1) as f64) as usize).min(n - 1)];
    println!("agent loop n={n} (FileStorage, scope=user/ana)");
    println!("  turn total   P50={:.2}ms P99={:.2}ms", p(0.50), p(0.99));
    println!("    remember     avg={:.2}ms (L3+L2episodic+L4 per turn)", remember_ms / n as f64);
    println!("    recall x2    avg={:.2}ms (lexical+semantic)", recall_ms / n as f64);
    println!("    rag_context  avg={:.2}ms", rag_ms / n as f64);

    // ── qualidade de recall: exato vs paráfrase ────────────────────────────
    // target: memória 42
    let target_words = "service-42 staging pipeline deploy-42";
    let exact = db.recall_lexical_scoped(target_words, 3, "user/ana").unwrap();
    let top_exact = exact.first().map(|h| h.key.clone()).unwrap_or_default();
    println!("recall exact-words: top1={} ({})", top_exact, if top_exact.contains("42") {"HIT"} else {"MISS"});

    // paráfrase com palavras DIFERENTES (o caso real do agente)
    let para = "what was the deployment procedure for that one service in the test environment";
    let hits_para = db.recall_lexical_scoped(para, 3, "user/ana").unwrap();
    let top_para = hits_para.first().map(|h| h.key.clone()).unwrap_or_default();
    println!("recall paraphrase : top1={} ({}), lexical não cobre paráfrase (esperado)", top_para, if top_para.contains("42") {"HIT"} else {"MISS"});
    let hits_sem_para = db.recall_scoped(&e(para), 3, "user/ana").unwrap();
    let top_sem = hits_sem_para.first().map(|h| h.key.clone()).unwrap_or_default();
    println!("semantic paraphrase: top1={} (demo trigram: sem modelo real, MISS esperado)", top_sem);

    // contaminação de scope: recall GLOBAL não deve trazer memória de user/ana
    db.set_default_scope(None);
    let leaked = db.recall_lexical(target_words, 5).unwrap();
    let any_leak = leaked.iter().any(|h| h.key.contains("deploy"));
    println!("scope isolation (global recall, sem scope): {} memórias user/ana no top5 — {}",
        leaked.iter().filter(|h| h.key.contains("deploy")).count(),
        if any_leak {"VAZAMENTO"} else {"isolado"});

    let bytes = std::fs::metadata(&path).unwrap().len();
    println!("  log final: {:.1} MiB ({} turnos)", bytes as f64/1048576.0, n);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(&dir);
}
