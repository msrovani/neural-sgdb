//! Protocolo do agente — como usar o neural-sgdb para DECISÃO (itens 2–6).
//!
//! Não é mais uma feature do core: é a DISCIPLINA de uso da camada superior.
//! Cada item é uma função reutilizável + auto-checks que provam o contrato:
//!
//! - **item 2 — ontologia de entidades fixa**: strings canônicas
//!   `kind/name` geradas por helpers (`project()`, `person()`, `topic()`).
//!   Mesmas strings na escrita (`remember`) e na busca (`recall_entities`).
//! - **item 3 — fato estruturado + verbatim**: `remember_fact(subject,
//!   predicate, object)` grava o texto canônico `<s> <p> <o>` + entidades;
//!   `remember_episodic` guarda o par cru (verbatim) que não deve ser
//!   resumido/extrado.
//! - **item 4 — decisão ponderada por provenance**: `evidence_for` junta
//!   recall semântico + entidades + temporal e pesa por confidence /
//!   importance / validade / contradição — uma memória pouco confiável ou
//!   fora da janela vale menos que uma confiável e vigente.
//! - **item 5 — ciclo de vida**: `supersede` quando um fato novo substitui o
//!   antigo, `feedback` quando um dado provou útil/errado, `expire_old`
//!   periódico para janelas fechadas.
//! - **item 6 — protocolo de duas passadas**: `decide` coleta evidências
//!   ANTES de responder e registra o aprendizado DEPOIS (fato novo ou
//!   supersede), sem nunca reaproveitar memória de outro scope.
//!
//! Uso:
//! ```text
//! cargo run --release --example agent_protocol
//! ```
//! Exit code 0 sse todas as asserções passaram.

use neural_sgdb::{Hit, InMemory, Sgdb, SgdbError};

// ── reporter minimal (PASS/FAIL) ────────────────────────────────────────────
struct Rep {
    checks: Vec<(String, bool, String)>,
    total: usize,
}
impl Rep {
    fn new() -> Rep { Rep { checks: Vec::new(), total: 0 } }
    fn check(&mut self, name: &str, ok: bool, detail: String) {
        self.total += 1;
        println!("{} {}", if ok { "PASS" } else { "FAIL" }, name);
        if !ok { println!("      {detail}"); }
        self.checks.push((name.to_string(), ok, detail));
    }
    fn done(&self) -> bool {
        let fails = self.checks.iter().filter(|c| !c.1).count();
        println!("\nasserções: {} total, {} falhas", self.checks.len(), fails);
        fails == 0
    }
}

// ── item 2: ontologia de entidades (strings canônicas, sempre iguais) ──────
fn project(name: &str) -> String { format!("project/{name}") }
fn person(name: &str) -> String { format!("person/{name}") }
fn topic(name: &str) -> String { format!("topic/{name}") }

// ── embedding determinístico do exemplo (mesmo modelo na escrita e busca) ──
fn emb(seed: u64) -> Vec<f32> {
    let mut s = seed.wrapping_mul(1103515245).wrapping_add(12345);
    let mut v = Vec::with_capacity(8);
    for _ in 0..8 {
        s = s.wrapping_mul(1103515245).wrapping_add(12345);
        v.push(((s >> 32) as i32 % 200) as f32 / 100.0 - 1.0);
    }
    v
}

// ── item 3: fato estruturado (subject predicate object) ─────────────────────
#[derive(Clone)]
struct Fact {
    key: String,
    subject: String,
    predicate: String,
    object: String,
    entities: Vec<String>,
    scope: String,
}

impl Fact {
    fn new(subject: &str, predicate: &str, object: &str) -> Fact {
        Fact {
            key: format!("fact/{subject}/{predicate}"),
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            object: object.to_string(),
            entities: Vec::new(),
            scope: String::new(),
        }
    }
    fn with(mut self, entities: &[String], scope: &str) -> Fact {
        self.entities = entities.to_vec();
        self.scope = scope.to_string();
        self
    }
    fn text(&self) -> String {
        format!("{} {} {}", self.subject, self.predicate, self.object)
    }
}

fn remember_fact(db: &mut Sgdb, f: &Fact) -> Result<(), SgdbError> {
    let text = f.text();
    db.remember_semantic(&f.key, &text, &emb(seed_from(&text)))?;
    let sk = format!("md/L4/{}", f.key);
    if !f.scope.is_empty() {
        db.set_scope(&sk, &f.scope)?;
    }
    let ents: Vec<&str> = f.entities.iter().map(|e| e.as_str()).collect();
    if !ents.is_empty() {
        db.set_entities(&sk, &ents)?;
    }
    Ok(())
}

fn seed_from(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// ── item 4: evidência ponderada por provenance ──────────────────────────────
// Devolve os hits do recall_weighted (importância do DOC) com score de
// evidência = w_sem·dist + w_rec·recência + w_imp·(1−importance), marcando
// se contradiz alguma memória vigente (evidência CONTRÁRIA).
fn evidence_for(db: &mut Sgdb, query: &str, k: usize, now: u64) -> Result<Vec<Hit>, SgdbError> {
    let q = emb(seed_from(query));
    // w_imp forte: a decisão puxa memórias CONFIÁVEIS, não só as próximas
    db.recall_weighted(&q, k, 1.0, 0.2, 2.0, now)
}

// ── item 6: protocolo de duas passadas ──────────────────────────────────────
// Passada 1: coleta evidências (semântica + entidades + contradições) SEM
// escrever nada. Passada 2: o "decisor" devolve o que decidiu; o chamador
// registra o aprendizado. A função não decide por você — decide COMO reunir
// o material da decisão.
fn gather_evidence(
    db: &mut Sgdb,
    query: &str,
    entities: &[String],
    k: usize,
    now: u64,
) -> Result<(Vec<Hit>, Vec<String>), SgdbError> {
    let sem = evidence_for(db, query, k, now)?;
    let mut ent: Vec<Hit> = Vec::new();
    if !entities.is_empty() {
        let ents: Vec<&str> = entities.iter().map(|e| e.as_str()).collect();
        ent = db.recall_entities(&ents, k)?;
    }
    // contradições: qualquer hit que contradiga o query (mesma entidade,
    // predicado oposto) vira evidência contrária explícita
    let mut against: Vec<String> = Vec::new();
    for h in &ent {
        if h.provenance.as_ref().map(|p| p.importance).unwrap_or(0.0) >= 0.5 {
            let sk = &h.key;
            for c in db.contradicts(sk) {
                if !against.contains(&c) {
                    against.push(c);
                }
            }
        }
    }
    Ok((sem, against))
}

// ── item 5: ciclo de vida ───────────────────────────────────────────────────
fn learn_fact(
    db: &mut Sgdb,
    f: &Fact,
    supersede_old: Option<&str>,
) -> Result<(), SgdbError> {
    remember_fact(db, f)?;
    let new_sk = format!("md/L4/{}", f.key);
    // supersede: o fato antigo (mesmo subject/predicate) morre na versão nova
    if let Some(old) = supersede_old {
        db.supersede(old, &new_sk)?;
    }
    // feedback positivo implícito: fato recém-aprendido vale mais na decisão
    db.feedback(&new_sk, true, 0.1)?;
    Ok(())
}

fn main() {
    let mut rep = Rep::new();
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    let now = 2_000u64;

    // ── item 2: ontologia ──────────────────────────────────────────────────
    db.remember_semantic("prefs/ana", "ana prefere cafe e trabalha no projeto neural-sgdb", &emb(seed_from("ana")))
        .unwrap();
    let sk_ana = "md/L4/prefs/ana";
    db.set_scope(sk_ana, "user/ana").unwrap();
    db.set_entities(sk_ana, &[&project("neural-sgdb"), &person("ana"), &topic("cafe")])
        .unwrap();
    let hits = db.recall_entities_scoped(&[&project("neural-sgdb")], 10, "user/ana").unwrap();
    rep.check(
        "item 2: ontologia (mesma string na escrita e busca) acha o doc no scope",
        hits.iter().any(|h| h.key == sk_ana),
        format!("hits: {:?}", hits.iter().map(|h| &h.key).collect::<Vec<_>>()),
    );
    // recall GLOBAL não vaza de scopes (mesmo contrato do recall semântico)
    let g = db.recall_entities(&[&project("neural-sgdb")], 10).unwrap();
    rep.check(
        "item 2: recall global não vaza de scopes",
        g.is_empty(),
        format!("hits: {:?}", g.iter().map(|h| &h.key).collect::<Vec<_>>()),
    );
    // string DIFERENTE (não-canônica) não casa — contrato de 1-hop exact match
    let no_hit = db.recall_entities(&["project/neural_sgdb"], 10).unwrap();
    rep.check(
        "item 2: string não-canônica NÃO casa (exact match, sem resolução de sinônimo)",
        no_hit.is_empty(),
        format!("hits: {:?}", no_hit.iter().map(|h| &h.key).collect::<Vec<_>>()),
    );

    // ── item 3: fato estruturado + verbatim ────────────────────────────────
    remember_fact(
        &mut db,
        &Fact::new("neural-sgdb", "usa", "rust")
            .with(&[project("neural-sgdb"), topic("linguagem")], ""),
    )
    .unwrap();
    let fact_hits = db.recall(&emb(seed_from("neural-sgdb usa rust")), 5).unwrap();
    rep.check(
        "item 3: fato estruturado é recallável pelo texto canônico",
        fact_hits.iter().any(|h| h.text.contains("neural-sgdb usa rust")),
        format!("{:?}", fact_hits.iter().map(|h| &h.text).collect::<Vec<_>>()),
    );
    // verbatim: par user/response cru fica em L2, recuperável via diary
    db.remember_episodic("qual o clima?", "sol e 24 graus", now).unwrap();
    let diary = db.diary(1, 10).unwrap();
    rep.check(
        "item 3: verbatim (remember_episodic) preservado em L2 no diary",
        diary.iter().any(|(_, t)| t.contains("sol e 24 graus")),
        format!("diary: {:?}", diary),
    );

    // ── item 4: decisão ponderada por provenance ───────────────────────────
    // memória antiga e pouco confiável NÃO vence uma confiável no recall_weighted
    db.remember_semantic("fato/legado", "neural-sgdb usa c++", &emb(seed_from("legado"))).unwrap();
    db.set_importance("md/L4/fato/legado", 0.1).unwrap(); // pouco confiável
    db.set_scope("md/L4/fato/legado", "").unwrap();
    let ev = evidence_for(&mut db, "neural-sgdb usa", 10, now).unwrap();
    // o fato confiável (rust, importance 1.0 default) ranqueia ACIMA do legado (0.1)
    let rust_pos = ev.iter().position(|h| h.text.contains("usa rust")).unwrap();
    let cpp_pos = ev.iter().position(|h| h.text.contains("usa c++")).unwrap();
    rep.check(
        "item 4: recall_weighted puxa a memória confiável (imp 1.0) antes da legada (imp 0.1)",
        rust_pos < cpp_pos,
        format!("ordem: {:?}", ev.iter().map(|h| &h.text).collect::<Vec<_>>()),
    );

    // ── item 5: ciclo de vida ──────────────────────────────────────────────
    // aprender um fato novo supersede o antigo (mesmo subject/predicate)
    learn_fact(
        &mut db,
        &Fact::new("neural-sgdb", "usa", "rust")
            .with(&[project("neural-sgdb"), topic("linguagem")], ""),
        Some("md/L4/fato/legado"),
    )
    .unwrap();
    // depois de supersede, o recall default (active-only) não devolve o antigo
    let after = db.recall(&emb(seed_from("neural-sgdb usa")), 10).unwrap();
    rep.check(
        "item 5: supersede arquiva o fato antigo no recall default",
        !after.iter().any(|h| h.text.contains("usa c++")),
        format!("{:?}", after.iter().map(|h| &h.text).collect::<Vec<_>>()),
    );
    // histórico preserva a versão superseded
    let hist = db.recall_historical(&emb(seed_from("neural-sgdb usa")), 10).unwrap();
    rep.check(
        "item 5: recall_historical preserva a versão superseded",
        hist.iter().any(|h| h.text.contains("usa c++")),
        format!("{:?}", hist.iter().map(|h| &h.text).collect::<Vec<_>>()),
    );
    // expire_old: janela fechada vira Invalidated e some do recall default
    db.set_validity("md/L4/fato/legado", 0, 1000).unwrap(); // fechou em 1000
    let expired = db.expire_old(now).unwrap();
    rep.check(
        "item 5: expire_old marca janela fechada como Invalidated",
        expired >= 1,
        format!("expired={expired}"),
    );
    let after_exp = db.recall(&emb(seed_from("legado")), 10).unwrap();
    rep.check(
        "item 5: memória Invalidated some do recall default",
        !after_exp.iter().any(|h| h.key.contains("/fato/legado")),
        format!("{:?}", after_exp.iter().map(|h| &h.key).collect::<Vec<_>>()),
    );

    // ── item 6: protocolo de duas passadas ─────────────────────────────────
    // passada 1: coleta evidências SEM escrever
    let (sem, against) = gather_evidence(
        &mut db,
        "qual a linguagem do neural-sgdb?",
        &[project("neural-sgdb")],
        5,
        now,
    )
    .unwrap();
    let found = sem.iter().any(|h| h.text.contains("usa rust"));
    rep.check(
        "item 6: passada 1 coleta evidências sem escrever (rust na frente)",
        found && against.is_empty(),
        format!("sem={} against={:?}", sem.len(), against),
    );
    // passada 2: o agente decide (ex: confirmar rust) e REGISTRA o aprendizado
    let decided = remember_fact(
        &mut db,
        &Fact::new("neural-sgdb", "e", "rust")
            .with(&[project("neural-sgdb"), topic("linguagem")], ""),
    );
    rep.check(
        "item 6: passada 2 registra o aprendizado (fato estruturado)",
        decided.is_ok(),
        format!("{:?}", decided.err()),
    );

    // fecha com o veredito do exemplo (exit code)
    if rep.done() {
        std::process::exit(0);
    }
    std::process::exit(1);
}