//! Suíte de integração "múltiplos agentes" (v1.3.1).
//!
//! Testa o nsgdb COMO SE várias IAs (agentes) dividissem o mesmo banco:
//! isolamento de escopo, identidade por entidades, fatos compartilhados vs
//! privados, ciclo de vida (supersede/expire/ttl), fork/merge de run,
//! sessões intercaladas (dois "processos" no mesmo storage), ledger de
//! ausências, auditoria e as superfícies de DX (scope_dim_labels/display).
//!
//! Determinístico: InMemory / backend compartilhado (Arc<Mutex>), `demo_embed`
//! p/ semântico (256-dim). Zero deps; só a API pública.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use neural_sgdb::{
    demo_embed, GcConfig, InMemory, MemoryDoc, MemoryLayer, MemoryState, MergeStrategy,
    RememberOptions, ScopeDims, ScopeFilter, Sgdb, SnapshotStorage, Storage,
};

/// Backend compartilhado: simula UM arquivo de DB acessado por "processos"
/// diferentes (cada `Sgdb::open` reindexa do storage, como dois agentes na
/// mesma máquina).
#[derive(Clone, Default)]
struct SharedBackend(Arc<Mutex<BTreeMap<Vec<u8>, Vec<u8>>>>);

impl Storage for SharedBackend {
    fn name(&self) -> &'static str {
        "shared"
    }
    fn put(&mut self, k: &[u8], v: &[u8]) -> Result<(), neural_sgdb::SgdbError> {
        self.0.lock().unwrap().insert(k.to_vec(), v.to_vec());
        Ok(())
    }
    fn get(&mut self, k: &[u8]) -> Result<Option<Vec<u8>>, neural_sgdb::SgdbError> {
        Ok(self.0.lock().unwrap().get(k).cloned())
    }
    fn scan_prefix(&mut self, p: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, neural_sgdb::SgdbError> {
        let m = self.0.lock().unwrap();
        Ok(m.iter()
            .filter(|(k, _)| k.starts_with(p))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }
    fn delete(&mut self, k: &[u8]) -> Result<(), neural_sgdb::SgdbError> {
        self.0.lock().unwrap().remove(k);
        Ok(())
    }
}

fn agent_dims(user: &str, run: &str) -> ScopeDims {
    ScopeDims {
        user: user.to_string(),
        agent: String::new(),
        app: String::new(),
        run: run.to_string(),
    }
}

fn filter_user(user: &str) -> ScopeFilter {
    ScopeFilter {
        user: Some(user.to_string()),
        agent: None,
        app: None,
        run: None,
    }
}

fn q(text: &str) -> Vec<f32> {
    demo_embed(text)
}

fn keys(hits: &[neural_sgdb::Hit]) -> Vec<String> {
    hits.iter().map(|h| h.key.clone()).collect()
}

// ── 1. Isolamento por escopo legado ──────────────────────────────────────────
#[test]
fn agents_are_isolated_by_legacy_scope() {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    // Três agentes, cada um no seu escopo; um fato GLOBAL compartilhado.
    db.remember_text_with("proj/constraint", "nao usar threads", RememberOptions::default())
        .unwrap();
    for (id, fact) in [
        ("alice", "alice prefere dark mode"),
        ("bob", "bob prefere light mode"),
        ("carol", "carol usa vim"),
    ] {
        let key = format!("agent/{id}/pref");
        let scope = format!("agent/{id}");
        db.remember_text_with(
            &key,
            fact,
            RememberOptions {
                scope: Some(&scope),
                ..Default::default()
            },
        )
        .unwrap();
    }
    // Global: só o fato compartilhado.
    let g = keys(&db.recall_lexical("mode threads dark light vim", 10).unwrap());
    assert_eq!(g, vec!["md/L3/proj/constraint"], "global não vaza escopos: {g:?}");
    // Cada agente vê o próprio escopo pela palavra que o identifica.
    for (id, word) in [("alice", "dark"), ("bob", "light"), ("carol", "vim")] {
        let scope = format!("agent/{id}");
        let scoped = keys(&db.recall_lexical_scoped(word, 10, &scope).unwrap());
        assert_eq!(scoped.len(), 1, "{id}: {scoped:?}");
        assert!(scoped[0].contains(id), "{id}: {scoped:?}");
        assert!(!scoped[0].contains("proj/constraint"));
    }
}

// ── 2. Isolamento por dims (scope_user/agent/app/run) ───────────────────────
#[test]
fn agents_are_isolated_by_scope_dims() {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    for (u, a, app) in [("alice", "planner", "shop"), ("bob", "coder", "shop"), ("carol", "planner", "docs")] {
        let emb = q(&format!("relatorio {u}"));
        db.remember_semantic_with(
            &format!("r/{u}"),
            &format!("relatorio de {u} para {app}"),
            &emb,
            RememberOptions {
                scope_dims: Some(ScopeDims {
                    user: u.to_string(),
                    agent: a.to_string(),
                    app: app.to_string(),
                    run: String::new(),
                }),
                ..Default::default()
            },
        )
        .unwrap();
    }
    // Alice (user=alice) só vê o dela.
    let a = keys(&db.recall_scoped_dims(&q("relatorio alice"), 10, &filter_user("alice")).unwrap());
    assert_eq!(a, vec!["md/L4/r/alice"]);
    // Wildcard app=shop → alice + bob.
    let shop = keys(&db.recall_scoped_dims(&q("relatorio"), 10, &ScopeFilter {
        user: None, agent: None, app: Some("shop".to_string()), run: None,
    }).unwrap());
    assert_eq!(shop.len(), 2, "{shop:?}");
    assert!(shop.iter().any(|k| k.contains("alice")) && shop.iter().any(|k| k.contains("bob")));
    // Global não vê nenhuma (null-scoping).
    assert!(db.recall(&q("relatorio"), 10).unwrap().is_empty());
}

// ── 3. Fatos compartilhados + preferências privadas ─────────────────────────
#[test]
fn shared_global_plus_private_preferences() {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    db.remember_semantic_with("proj/rule", "proibido comprar dependencia", &q("proibido dependencia"), RememberOptions::default()).unwrap();
    for (u, pref) in [("alice", "gosta de rust"), ("bob", "gosta de zig")] {
        db.remember_semantic_with(
            &format!("pref/{u}"),
            pref,
            &q(pref),
            RememberOptions {
                scope: Some(u),
                ..Default::default()
            },
        )
        .unwrap();
    }
    // Alice: global + a preferência dela; NUNCA a do bob.
    let a = keys(&db.recall(&q("gosta dependencia"), 10).unwrap());
    assert_eq!(a, vec!["md/L4/proj/rule"], "global so compartilhado: {a:?}");
    let a_pref = keys(&db.recall_scoped(&q("gosta rust"), 10, "alice").unwrap());
    assert_eq!(a_pref, vec!["md/L4/pref/alice"]);
    let b_pref = keys(&db.recall_scoped(&q("gosta zig"), 10, "bob").unwrap());
    assert_eq!(b_pref, vec!["md/L4/pref/bob"]);
}

// ── 4. Entidades 1-hop por agente ───────────────────────────────────────────
#[test]
fn entity_hop_only_reaches_own_agent_facts() {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    for (u, fact) in [("alice", "alice fechou o ticket 42"), ("bob", "bob revisou o pr 99")] {
        let agent_ent = format!("agent/{u}");
        db.remember_text_with(
            &format!("mom/{u}"),
            fact,
            RememberOptions {
                entities: &[agent_ent.as_str(), "mom/decision"],
                ..Default::default()
            },
        )
        .unwrap();
    }
    let a = keys(&db.recall_entities(&["agent/alice"], 10).unwrap());
    assert_eq!(a.len(), 1);
    assert!(a[0].contains("alice"));
    let b = keys(&db.recall_entities(&["agent/bob"], 10).unwrap());
    assert!(b[0].contains("bob"));
    // overlap de entidade compartilhada acha os dois.
    let both = keys(&db.recall_entities(&["mom/decision"], 10).unwrap());
    assert_eq!(both.len(), 2);
}

// ── 5. Supersede respeitado entre agentes ───────────────────────────────────
#[test]
fn supersede_respected_across_agents() {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    // keys SEM relação de prefixo (regra ART).
    let o1 = db
        .remember_semantic_with("fact/old-theme", "versao antiga do tema", &q("tema"), RememberOptions::default())
        .unwrap();
    let o2 = db
        .remember_semantic_with("fact/new-theme", "versao nova do tema", &q("tema"), RememberOptions::default())
        .unwrap();
    // Agente "curador" marca a antiga como superseded pela nova.
    db.supersede(&o1.storage_key, &o2.storage_key).unwrap();
    // Qualquer agente com recall active-only não vê a superseded.
    let active = keys(&db.recall(&q("tema"), 10).unwrap());
    assert_eq!(active, vec!["md/L4/fact/new-theme"]);
    // Histórico explícito mostra as duas, com estado.
    let hist = db.recall_historical(&q("tema"), 10).unwrap();
    assert_eq!(hist.len(), 2);
    let old = hist.iter().find(|h| h.key == "md/L4/fact/old-theme").unwrap();
    assert_eq!(old.provenance.as_ref().unwrap().state, MemoryState::Superseded);
}

// ── 6. Fork/merge: promote_run Ours + dedup ─────────────────────────────────
#[test]
fn fork_merge_promote_run_ours_and_dedup() {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    let run = "rel-42";
    // Base (alice, sem run) já tem um fato.
    db.remember_text_with("note1", "base note1", RememberOptions {
        scope_dims: Some(agent_dims("alice", "")),
        ..Default::default()
    }).unwrap();
    // Sandbox: alice + run escreve note1 (idêntico ao base → dedup) e note2 novo.
    db.remember_text_with(&format!("{run}/note1"), "base note1", RememberOptions {
        scope_dims: Some(agent_dims("alice", run)),
        ..Default::default()
    }).unwrap();
    db.remember_text_with(&format!("{run}/note2"), "novo do run", RememberOptions {
        scope_dims: Some(agent_dims("alice", run)),
        ..Default::default()
    }).unwrap();
    let rep = db
        .promote_run(&ScopeFilter { user: None, agent: None, app: None, run: Some(run.to_string()) }, &agent_dims("alice", run), MergeStrategy::Ours)
        .unwrap();
    assert_eq!(rep.deduped, 1, "note1 idêntico → dedup");
    assert_eq!(rep.promoted, vec!["md/L3/note2"], "{rep:?}");
    assert!(rep.conflicts.is_empty());
    // No base (alice, sem run): note1 e note2 alcançáveis SEM o prefixo run.
    // O run e o base COMPARTILHAM o legacy scope "alice" (write-through v1.1.24),
    // então isolamos pela dim run="": só as re-escopadas. (L3 → lexical dims.)
    let base = keys(&db.recall_lexical_dims(
        "base note1 novo",
        10,
        &ScopeFilter { user: Some("alice".into()), agent: None, app: None, run: Some(String::new()) },
    ).unwrap());
    assert_eq!(base.len(), 2, "{base:?}");
    assert!(base.iter().all(|k| !k.contains(run)), "re-escopada sem run: {base:?}");
    assert!(base.iter().any(|k| k == "md/L3/note1"));
    assert!(base.iter().any(|k| k == "md/L3/note2"));
    // O run original continua alcançável pela dim run.
    let run_only = keys(&db.recall_lexical_dims(
        "base note1 novo",
        10,
        &ScopeFilter { user: Some("alice".into()), agent: None, app: None, run: Some(run.into()) },
    ).unwrap());
    assert_eq!(run_only.len(), 2, "{run_only:?}");
}

// ── 7. Fork/merge: Theirs e Fail ────────────────────────────────────────────
#[test]
fn fork_merge_promote_run_theirs_and_fail() {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    let run = "rel-99";
    db.remember_text_with("note1", "texto do base", RememberOptions {
        scope_dims: Some(agent_dims("bob", "")),
        ..Default::default()
    }).unwrap();
    db.remember_text_with(&format!("{run}/note1"), "texto DIFERENTE no run", RememberOptions {
        scope_dims: Some(agent_dims("bob", run)),
        ..Default::default()
    }).unwrap();
    let f = ScopeFilter { user: None, agent: None, app: None, run: Some(run.to_string()) };
    let bd = agent_dims("bob", run);
    // Fail: conflito → erro em voz alta, nada muda.
    assert!(db.promote_run(&f, &bd, MergeStrategy::Fail).is_err());
    // Theirs: base vence; conflito fica no run.
    let rep = db.promote_run(&f, &bd, MergeStrategy::Theirs).unwrap();
    assert_eq!(rep.conflicts_kept, vec!["md/L3/note1"]);
    // Base (bob, sem run) mantém o texto do base — dims run="" isola.
    let base = keys(&db.recall_lexical_dims(
        "texto do base",
        10,
        &ScopeFilter { user: Some("bob".into()), agent: None, app: None, run: Some(String::new()) },
    ).unwrap());
    assert_eq!(base, vec!["md/L3/note1"]);
    // O run continua no run (não promovido) — L3 → lexical dims.
    let run_mem = keys(&db.recall_lexical_dims("texto DIFERENTE", 10, &ScopeFilter { user: Some("bob".into()), agent: None, app: None, run: Some(run.into()) }).unwrap());
    assert_eq!(run_mem, vec![format!("md/L3/{run}/note1")]);
}

// ── 8. Sessões intercaladas (dois "processos" no mesmo storage) ─────────────
#[test]
fn interleaved_agent_sessions_share_storage() {
    let back = SharedBackend::default();
    // Sessão A: alice escreve.
    {
        let mut a = Sgdb::open(back.clone()).unwrap();
        a.remember_text_with("a/f1", "fato da alice", RememberOptions {
            scope: Some("alice"),
            ..Default::default()
        }).unwrap();
    }
    // Sessão B: bob abre o MESMO storage (reindexa), escreve, e vê só o dele.
    {
        let mut b = Sgdb::open(back.clone()).unwrap();
        assert_eq!(b.recall_lexical_scoped("fato", 10, "alice").unwrap().len(), 1, "bob reindexa o que alice escreveu");
        b.remember_text_with("b/f1", "fato do bob", RememberOptions {
            scope: Some("bob"),
            ..Default::default()
        }).unwrap();
        assert_eq!(b.recall_lexical_scoped("fato", 10, "bob").unwrap().len(), 1);
        assert!(b.recall_lexical("fato", 10).unwrap().is_empty(), "global não vê escopados");
    }
    // Nova sessão A: vê os dois fatos, cada um no seu escopo.
    let mut a = Sgdb::open(back.clone()).unwrap();
    assert_eq!(a.recall_lexical_scoped("fato", 10, "alice").unwrap().len(), 1);
    assert_eq!(a.recall_lexical_scoped("fato", 10, "bob").unwrap().len(), 1);
}

// ── 9. Modos de retrieval respeitam o escopo do agente ──────────────────────
#[test]
fn retrieval_modes_respect_agent_scope() {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    for (u, txt) in [("alice", "alice estuda rust ownership"), ("bob", "bob estuda zig allocator")] {
        let emb = q(txt);
        db.remember_semantic_with(&format!("{u}/study"), txt, &emb, RememberOptions {
            scope: Some(u),
            ..Default::default()
        }).unwrap();
    }
    // lexical escopado isola.
    assert_eq!(db.recall_lexical_scoped("ownership", 10, "alice").unwrap().len(), 1);
    assert!(db.recall_lexical_scoped("ownership", 10, "bob").unwrap().is_empty());
    // semântico escopado: só o primário L4 (o companion L2 não entra no BQ).
    assert_eq!(db.recall_scoped(&q("rust"), 10, "alice").unwrap().len(), 1);
    // hybrid RRF escopado: semântico (L4) ∪ lexical (L3/companion L2) — tudo
    // do MESMO agente; nada do bob.
    let h = keys(&db.recall_hybrid_rrf_scoped(&q("rust ownership"), "rust ownership", 10, "alice").unwrap());
    assert!(h.iter().any(|k| k == "md/L4/alice/study"), "{h:?}");
    assert!(h.iter().all(|k| k.contains("alice")), "nada do bob: {h:?}");
    // O escopo de bob NUNCA contém fato da alice (isolamento, não vazio).
    let b = keys(&db.recall_hybrid_rrf_scoped(&q("rust ownership"), "rust ownership", 10, "bob").unwrap());
    assert!(!b.iter().any(|k| k.contains("alice")), "não vaza fato da alice: {b:?}");
}

// ── 10. Validade temporal por agente ────────────────────────────────────────
#[test]
fn temporal_validity_per_agent() {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    let out = db.remember_semantic_with("plan/a", "plano de alice", &q("plano"), RememberOptions {
        scope: Some("alice"),
        ..Default::default()
    }).unwrap();
    db.set_validity(&out.storage_key, 1000, 2000).unwrap();
    // Válido dentro da janela; recall_temporal em 1500 acha.
    let t = keys(&db.recall_temporal_scoped(&q("plano"), 10, 1500, 1.0, 10.0, "alice").unwrap());
    assert!(!t.is_empty());
    // `recall_at` FILTRA por validade: fora da janela não acha.
    assert!(db.recall_at(&q("plano"), 10, 3000).unwrap().is_empty());
    assert_eq!(db.expire_old(2000).unwrap(), 1);
    assert!(db.recall_scoped(&q("plano"), 10, "alice").unwrap().is_empty(), "invalidada some do active-only");
    assert!(!db.recall_scoped_historical(&q("plano"), 10, "alice").unwrap().is_empty(), "histórico preserva");
}

// ── 11. Feedback re-moldura recall ponderado ────────────────────────────────
#[test]
fn feedback_reshapes_weighted_recall() {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    let s1 = db.remember_semantic_with("shared/a", "doc a", &q("doc"), RememberOptions::default()).unwrap();
    db.remember_semantic_with("shared/b", "doc b", &q("doc"), RememberOptions::default()).unwrap();
    // Agente B dá feedback POSITIVO na memória compartilhada a.
    db.feedback(&s1.storage_key, true, 0.6).unwrap();
    let w = db.recall_weighted(&q("doc"), 2, 1.0, 0.0, 1.0, 1).unwrap();
    assert_eq!(w[0].key, s1.storage_key, "feedback positivo sobe a importância → top-1");
    // reforço (reinforce) também importa.
    db.reinforce(&s1.storage_key, 0.3).unwrap();
    let m = db.meta(&s1.storage_key).unwrap().unwrap();
    assert!(m.importance > 0.9);
}

// ── 12. Ledger de ausências é escopado + self-healing ───────────────────────
#[test]
fn absence_ledger_is_scoped_and_self_heals() {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    // Alice procura "senha wifi" no escopo dela: não existe → registra ausência.
    let led_a = db.recall_with_ledger("senha wifi", 5, Some("alice"), 100).unwrap();
    assert!(led_a.recorded, "primeiro probe registra ausência");
    assert!(!led_a.absences.is_empty());
    // Bob, no escopo DELE, não é afetado pela ausência da alice.
    let led_b = db.recall_with_ledger("senha wifi", 5, Some("bob"), 100).unwrap();
    assert!(led_b.recorded, "bob registra a PRÓPRIA ausência (escopo isolado)");
    // Alice anota o fato; o próximo probe ACHA e remove a ausência (self-heal).
    db.remember_text_with("wifi/alice", "senha wifi alice: 1234", RememberOptions {
        scope: Some("alice"),
        ..Default::default()
    }).unwrap();
    let led_a2 = db.recall_with_ledger("senha wifi", 5, Some("alice"), 101).unwrap();
    assert!(!led_a2.recorded, "achou → self-healing remove a ausência");
    assert!(led_a2.absences.is_empty());
}

// ── 13. TTL/GC atinge só o próprio agente ───────────────────────────────────
#[test]
fn ttl_gc_only_touches_own_agent() {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    let a = db.remember_text_with("tmp/a", "sessao alice", RememberOptions {
        scope: Some("alice"),
        ..Default::default()
    }).unwrap();
    let b = db.remember_text_with("keep/b", "sessao bob", RememberOptions {
        scope: Some("bob"),
        ..Default::default()
    }).unwrap();
    db.set_ttl(&a.storage_key, 100).unwrap();
    assert!(db.ttl_of(&b.storage_key).unwrap().is_none(), "bob sem TTL");
    assert_eq!(db.expire_ttl(100).unwrap(), 1, "só o TTL da alice expira");
    assert!(db.get(MemoryLayer::L3EpisodicLong, "tmp/a").unwrap().is_none());
    assert!(db.get(MemoryLayer::L3EpisodicLong, "keep/b").unwrap().is_some());
    // GC de estado: alice arquiva, bob não; collect_garbage só pega arquivada.
    db.forget(&b.storage_key).unwrap();
    let rep = db.collect_garbage(999, &GcConfig { collect_archived: true, min_age_ticks: 0, ..GcConfig::default() }).unwrap();
    assert_eq!(rep.state_collected, 1);
}

// ── 14. Auditoria hash-chain rastreia agentes ───────────────────────────────
#[test]
fn audit_chain_tracks_agent_writes() {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    db.remember_text_with("a/1", "fato alice", RememberOptions { scope: Some("alice"), ..Default::default() }).unwrap();
    db.remember_text_with("b/1", "fato bob", RememberOptions { scope: Some("bob"), ..Default::default() }).unwrap();
    let s1 = db.audit_checkpoint(100).unwrap();
    db.remember_text_with("a/2", "mais um da alice", RememberOptions { scope: Some("alice"), ..Default::default() }).unwrap();
    let s2 = db.audit_checkpoint(200).unwrap();
    assert!(s2 > s1);
    let rep = db.audit_verify().unwrap();
    assert!(rep.chain_intact, "chain intacta");
    // rollback ao 1º checkpoint remove o metadado da memória criada DEPOIS.
    let n = db.rollback_to(s1).unwrap();
    assert!(n >= 1, "{n}");
    assert!(db.meta("md/L3/a/2").unwrap().is_none(), "meta pós-checkpoint revertida");
}

// ── 15. Descritores de escopo + issues (DX P0.3) ────────────────────────────
#[test]
fn scope_descriptors_and_issues_reflect_agents() {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    db.remember_semantic_with("run/note", "nota do run", &q("nota"), RememberOptions {
        scope_dims: Some(ScopeDims { user: "alice".into(), agent: String::new(), app: String::new(), run: "rel-7".into() }),
        ..Default::default()
    }).unwrap();
    let descs = db.scope_dim_descriptors().unwrap();
    let d = &descs[0];
    assert_eq!(d.user, "alice");
    assert_eq!(d.run, "rel-7");
    assert_eq!(d.display, "user=alice+run=rel-7", "display legível (nunca ///x)");
    assert_eq!(d.count, 1);
    assert!(db.scope_issues().unwrap().is_empty(), "dims-only legítimo não é issue");
    // Escopo legado com segmento vazio → issue.
    db.remember_text_with("x", "x", RememberOptions::default()).unwrap();
    db.set_scope("md/L3/x", "tenant//proj").unwrap();
    let issues = db.scope_issues().unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].0, "tenant//proj");
}

// ── 16. Timeline de fato evolutivo por agente ───────────────────────────────
#[test]
fn event_timeline_per_agent_fact() {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    let p1 = db.remember_text_with("cargo/a", "alice em SP", RememberOptions { scope: Some("alice"), ..Default::default() }).unwrap();
    let p2 = db.remember_text_with("cargo/b", "alice em RJ", RememberOptions { scope: Some("alice"), ..Default::default() }).unwrap();
    db.set_validity(&p1.storage_key, 1000, 2000).unwrap();
    db.set_validity(&p2.storage_key, 3000, 4000).unwrap();
    db.set_event(&p1.storage_key, "cargo/alice", 2000).unwrap();
    db.set_event(&p2.storage_key, "cargo/alice", 0).unwrap();
    let tl = db.recall_timeline("cargo/alice", 10).unwrap();
    assert_eq!(tl.len(), 2);
    assert_eq!(tl[0].0, p1.storage_key, "ordenado por validade.from");
    assert_eq!(tl[1].0, p2.storage_key);
    db.close_event(&p2.storage_key, 4000).unwrap();
    assert_eq!(db.event_of(&p2.storage_key).unwrap().unwrap().1, 4000);
}

// ── 17. RRF funde semântico+lexical do MESMO agente ─────────────────────────
#[test]
fn hybrid_rrf_fuses_agent_semantic_and_lexical() {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    let emb = q("ERR_2043 rotate credentials");
    db.remember_semantic_with("k1", "ERR_2043 rotate credentials", &emb, RememberOptions { scope: Some("alice"), ..Default::default() }).unwrap();
    db.remember_text_with("k2", "ERR_2043 rotate credentials report", RememberOptions { scope: Some("alice"), ..Default::default() }).unwrap();
    // RRF = semântico (L4/k1) ∪ lexical (L3/k2 E o companion L2 de k1) —
    // todos do mesmo agente; os dois PRIMÁRIOS presentes.
    let fused = keys(&db.recall_hybrid_rrf_scoped(&emb, "ERR_2043 rotate credentials", 5, "alice").unwrap());
    assert!(fused.iter().any(|k| k == "md/L4/k1"), "{fused:?}");
    assert!(fused.iter().any(|k| k == "md/L3/k2"), "{fused:?}");
    // Escopo de bob não vê nada.
    assert!(db.recall_hybrid_rrf_scoped(&emb, "ERR_2043", 5, "bob").unwrap().is_empty());
    // Sem RRF (conteúdo idêntico) o score é > 0 em ambos.
    for h in db.recall_hybrid_rrf(&emb, "ERR_2043 rotate credentials", 5).unwrap() {
        assert!(h.score > 0.0);
    }
}

// ── 18. Estado corrente vence legado próximo (state-first) ──────────────────
#[test]
fn current_fact_wins_near_tie_for_agent() {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    let v = [0.0f32, 1.0, 0.0, 0.0];
    db.remember_semantic_with("a_leg", "legado do fato", &v, RememberOptions { entities: &["fato/tema"], ..Default::default() }).unwrap();
    db.remember_semantic_with("z_cur", "corrente do fato", &v, RememberOptions { entities: &["fato/tema"], ..Default::default() }).unwrap();
    // Terceiro doc com vetor DISTINTO (outro grupo de score) domina.
    db.remember_semantic("near", "outro assunto", &[1.0, 0.01, 0.0, 0.0]).unwrap();
    let hits = db.recall(&[1.0f32, 0.0, 0.0, 0.0], 5).unwrap();
    let ks = keys(&hits);
    assert_eq!(ks.len(), 3, "{ks:?}");
    assert!(ks[0].ends_with("/near"), "conteúdo domina: {ks:?}");
    assert!(ks[1].ends_with("/z_cur"), "corrente antes do legado: {ks:?}");
    assert!(ks[2].ends_with("/a_leg"), "{ks:?}");
}

// ── 19. SnapshotStorage: fronteira de sessão persistente ────────────────────
#[test]
fn snapshot_session_boundary_preserves_agent_state() {
    // Um "host" persiste o backend como bytes (OPFS/IndexedDB) e restaura na
    // próxima sessão: SnapshotStorage::to_bytes → from_bytes → Sgdb::open.
    let mut s = SnapshotStorage::new();
    let doc = MemoryDoc::new(MemoryLayer::L3EpisodicLong, "mem/alice", b"memoria de alice".to_vec());
    s.put(b"md/L3/mem/alice", &doc.encode()).unwrap();
    let snap = s.to_bytes();
    assert!(!snap.is_empty());
    // Nova sessão: restaura e abre; o Sgdb reindexa do storage.
    let restored = SnapshotStorage::from_bytes(&snap).unwrap();
    let mut r = Sgdb::open(restored).unwrap();
    let hits = r.recall_lexical("memoria", 5).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].key, "md/L3/mem/alice");
    // Truncado → Err, nunca panic.
    assert!(SnapshotStorage::from_bytes(&snap[..snap.len() / 2]).is_err());
}

// ── 20. Múltiplos agentes + doutrina escopada ───────────────────────────────
#[test]
fn agents_share_doctrine_but_isolate_their_facts() {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    let emb = q("nsgdb is a MEMORY substrate");
    db.ensure_doctrine(&emb).unwrap();
    // Doutrina está em scope=nsgdb/doctrine: nenhum agente a vê no global.
    assert!(db.recall(&emb, 5).unwrap().is_empty());
    // Mas é achada por recall_entities no próprio escopo.
    let d = keys(&db.recall_entities_scoped(&["doc/protocol"], 3, "nsgdb/doctrine").unwrap());
    assert_eq!(d, vec!["md/L4/nsgdb/doctrine"]);
    // Um agente escreve e NÃO contamina a doutrina.
    db.remember_semantic_with("u/1", "fato do usuario", &q("fato do usuario"), RememberOptions { scope: Some("user/ana"), ..Default::default() }).unwrap();
    assert!(db.recall(&q("fato do usuario"), 5).unwrap().is_empty(), "global não vê escopado");
    assert_eq!(db.recall_scoped(&q("fato do usuario"), 5, "user/ana").unwrap().len(), 1);
}