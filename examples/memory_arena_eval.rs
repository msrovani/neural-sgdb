//! memory_arena_eval — avaliação da UTILIDADE da memória (estilo MemoryArena,
//! arXiv 2602.16313): loop memória–agente–ambiente com subtarefas
//! INTERDEPENDENTES em múltiplas sessões. Mede SUCCESS RATE (SR, binário) e
//! SOFT PROGRESS (sPS, fração de restrições satisfeitas) — o mesmo desenho
//! que expõe o gap de agentes saturados em recall estático (LoCoMo) no uso
//! agêntico real.
//!
//! Dois agentes recebem os MESMOS episódios e respondem as MESMAS perguntas:
//! - **Config A (naive hoarder)**: escreve global, recall semântico puro,
//!   chaves com timestamp (append), sem scope/lexical/lifecycle.
//! - **Config B (protocolo v2)**: fato escopado + entidades, rerank gate
//!   (semântico∪lexical), write-path dedup, verbatim episódico, checkpoint de
//!   sessão escopado, ciclo de vida.
//!
//! Seção 1 — QUIZ de recall estático: ambos acertam 3/3 (saturação — recall
//! puro não distingue as configurações, como no LoCoMo).
//! Seção 2 — TAREFAS agênticas interdependentes: B 3/3, A 0/3. As subtarefas
//! dependem de informação adquirida em sessões anteriores que o ambiente NÃO
//! reestateia: (1) restrição escopada (checkpoint), (2) valor intermediário
//! exato verbatim (rerank gate), (3) estado corrente após update (supersede).
//!
//! Determinístico (sem LLM): InMemory, embeddings trigram do exemplo.
//! Exit code 0 sse quiz empata em 3/3 E SR(B) > SR(A) E sPS(B) > sPS(A).
//!
//! Uso:
//! ```text
//! cargo run --release --example memory_arena_eval
//! ```

use neural_sgdb::{Hit, InMemory, Sgdb, SgdbError};

// ── helpers compartilhados (mesmas convenções do protocolo do agente) ───────
fn project(name: &str) -> String { format!("project/{name}") }
fn person(name: &str) -> String { format!("person/{name}") }
fn topic(name: &str) -> String { format!("topic/{name}") }

fn emb(seed: u64) -> Vec<f32> {
    let mut s = seed.wrapping_mul(1103515245).wrapping_add(12345);
    let mut v = Vec::with_capacity(8);
    for _ in 0..8 {
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

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

fn grounding_score(hit_text: &str, query: &str) -> usize {
    let qt = tokens(query);
    let ht = tokens(hit_text);
    qt.iter().filter(|q| ht.contains(q)).count()
}

#[derive(Clone)]
struct Fact {
    subject: String,
    predicate: String,
    object: String,
    entities: Vec<String>,
    scope: String,
}

impl Fact {
    fn new(subject: &str, predicate: &str, object: &str) -> Fact {
        Fact {
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
    fn key(&self) -> String {
        format!("fact/{}/{}", self.subject, self.predicate)
    }
    fn text(&self) -> String {
        format!("{} {} {}", self.subject, self.predicate, self.object)
    }
}

fn remember_fact(db: &mut Sgdb, f: &Fact) -> Result<String, SgdbError> {
    let text = f.text();
    db.remember_semantic(&f.key(), &text, &emb(seed_from(&text)))?;
    let sk = format!("md/L4/{}", f.key());
    if !f.scope.is_empty() {
        db.set_scope(&sk, &f.scope)?;
    }
    let ents: Vec<&str> = f.entities.iter().map(|e| e.as_str()).collect();
    if !ents.is_empty() {
        db.set_entities(&sk, &ents)?;
    }
    Ok(sk)
}

// P1/P5 — rerank gate: pool híbrido + ancoragem lexical antes de compilar.
fn rerank_gate(db: &mut Sgdb, query: &str, k: usize) -> Result<Vec<Hit>, SgdbError> {
    let q = emb(seed_from(query));
    let mut pool = db.recall_oversampled(&q, k.max(1).saturating_mul(4), 4)?;
    let lex = db.recall_lexical(query, k.max(1).saturating_mul(4))?;
    for h in lex {
        if !pool.iter().any(|p| p.key == h.key) {
            pool.push(h);
        }
    }
    pool.sort_by(|a, b| {
        grounding_score(&b.text, query)
            .cmp(&grounding_score(&a.text, query))
            .then_with(|| a.dist.total_cmp(&b.dist))
            .then_with(|| {
                let ia = a.provenance.as_ref().map(|p| p.importance).unwrap_or(0.0);
                let ib = b.provenance.as_ref().map(|p| p.importance).unwrap_or(0.0);
                ib.partial_cmp(&ia).unwrap_or(core::cmp::Ordering::Equal)
            })
            .then_with(|| a.key.cmp(&b.key))
    });
    Ok(pool.into_iter().take(k).collect())
}

// P2 — write-path filter: dedup/supersede via 1-hop de entidades.
fn remember_fact_checked(db: &mut Sgdb, f: &Fact) -> Result<String, SgdbError> {
    let probe = format!("{} {}", f.subject, f.predicate);
    let ents: Vec<&str> = f.entities.iter().map(|e| e.as_str()).collect();
    if !ents.is_empty() {
        let hits = if f.scope.is_empty() {
            db.recall_entities(&ents, 16)?
        } else {
            db.recall_entities_scoped(&ents, 16, &f.scope)?
        };
        for h in hits {
            let t = h.text.trim_start();
            if t.starts_with(&probe) {
                if t[probe.len()..].trim() == f.object {
                    return Ok(h.key); // dedup: já existe idêntico
                }
                break; // objeto mudou → escrever (mesma chave, version bump)
            }
        }
    }
    remember_fact(db, f)
}

// P4/P6 — abrir sessão: expire_old + checkpoint escopado das restrições.
fn open_session(db: &mut Sgdb, scope: &str, now: u64) -> Result<Vec<Hit>, SgdbError> {
    db.expire_old(now)?;
    let q = emb(seed_from(scope));
    db.recall_scoped(&q, 8, scope)
}

// ── reasoners determinísticos ────────────────────────────────────────────────
fn pick_for_constraint(items: &[&str], constraint: bool) -> usize {
    if !constraint {
        return 0;
    }
    items.iter().position(|i| i.contains("sem lactose")).unwrap_or(0)
}

fn value_after_deu(text: &str) -> Option<&str> {
    let w: Vec<&str> = text.split_whitespace().collect();
    w.windows(2)
        .find(|w2| w2[0] == "deu")
        .map(|w2| w2[1])
}

fn oldest_lang(hits: &[Hit]) -> &str {
    // Config A (append com timestamp): escolhe a versão MAIS ANTIGA
    for h in hits {
        if h.text.contains("rust") {
            return "rust";
        }
        if h.text.contains("zig") {
            return "zig";
        }
    }
    "none"
}

// ── tarefas ──────────────────────────────────────────────────────────────────
// 1) shopping com restrição escopada: sessão 2 depende de preferência da
//    sessão 1 que o ambiente não reestateia (P6 checkpoint).
fn task_shopping(db_a: &mut Sgdb, db_b: &mut Sgdb, now: u64) -> (bool, bool, f32, f32) {
    let scope = "user/ana";
    // sessão 1: "ana nao come lactose"
    // A: hoarder — joga a conversa crua num episódico (L2) e nunca estrutura;
    //    recall semântico não indexa L2 → a restrição fica inacessível.
    let _ = db_a.remember_episodic("conversa sessao 1", "ana nao come lactose", now);
    // B: fato escopado + entidades (consolida em forma recuperável)
    let _ = remember_fact(db_b, &Fact::new("ana", "nao come", "lactose").with(&[person("ana"), topic("dieta")], scope));

    // sessão 2: subtarefa — escolher item compatível (restrição NÃO reestateada)
    let items = ["pizza quatro queijos", "salada sem lactose", "bolo de chocolate"];
    let a_constraint = db_a
        .recall(&emb(seed_from("o que ana pode comer?")), 4)
        .map(|hs| hs.iter().any(|h| h.text.contains("lactose")))
        .unwrap_or(false);
    let a_pick = pick_for_constraint(&items, a_constraint);
    let b_chk = open_session(db_b, scope, now)
        .map(|hs| hs.iter().any(|h| h.text.contains("lactose")))
        .unwrap_or(false);
    let b_pick = pick_for_constraint(&items, b_chk);
    let a_ok = a_pick == 1;
    let b_ok = b_pick == 1;
    let a_soft = if a_pick == 1 { 1.0 } else { 0.0 };
    let b_soft = if b_pick == 1 { 1.0 } else { 0.0 };
    (a_ok, b_ok, a_soft, b_soft)
}

// 2) formal reasoning com valor intermediário EXATO: sessão 2 precisa do
//    verbatim guardado na sessão 1 (P1 rerank gate + P5 verbatim > abstração).
fn task_formal(db_a: &mut Sgdb, db_b: &mut Sgdb, now: u64) -> (bool, bool, f32, f32) {
    // sessão 1: resultado intermediário — AMBOS guardam verbatim (L2)
    let _ = db_a.remember_episodic("passo 7", "o passo 7 deu 42", now);
    let _ = db_b.remember_episodic("passo 7", "o passo 7 deu 42", now);
    // sessão 2: "qual o valor do passo 7?"
    // A: recall semântico puro → L2 fora do BQ → nunca responde o verbatim
    let a_answered = db_a
        .recall(&emb(seed_from("qual o valor do passo 7?")), 4)
        .ok()
        .and_then(|hs| hs.first().cloned())
        .map(|h| value_after_deu(&h.text).map(|v| v == "42").unwrap_or(false))
        .unwrap_or(false);
    // B: rerank gate → lexical recupera o verbatim → "42"
    let b_answered = rerank_gate(db_b, "qual o valor do passo 7?", 3)
        .ok()
        .and_then(|hs| hs.first().cloned())
        .map(|h| value_after_deu(&h.text).map(|v| v == "42").unwrap_or(false))
        .unwrap_or(false);
    // A não responde (semântico puro não vê L2) → SR 0; B responde → SR 1.
    let a_ok = a_answered;
    let b_ok = b_answered;
    (a_ok, b_ok, if a_ok { 1.0 } else { 0.0 }, if b_ok { 1.0 } else { 0.0 })
}

// 3) lifecycle: sessão 3 pergunta o estado CORRENTE depois de um update na
//    sessão 2 (P2 dedup/supersede vs append cego).
fn task_lifecycle(db_a: &mut Sgdb, db_b: &mut Sgdb, _now: u64) -> (bool, bool, f32, f32) {
    // sessão 1: "linguagem do projeto = rust"
    let _ = db_a.remember_semantic("msg/1", "linguagem do projeto rust", &emb(seed_from("lang1")));
    let _ = remember_fact_checked(db_b, &Fact::new("projeto", "usa linguagem", "rust").with(&[project("projeto"), topic("linguagem")], ""));
    // sessão 2: "linguagem = zig" (mudou)
    let _ = db_a.remember_semantic("msg/2", "linguagem do projeto zig", &emb(seed_from("lang2")));
    let _ = remember_fact_checked(db_b, &Fact::new("projeto", "usa linguagem", "zig").with(&[project("projeto"), topic("linguagem")], ""));
    // sessão 3: "qual a linguagem ATUAL?"
    // A: append com timestamp → dois docs vivos → reasoner naive pega o antigo
    let a_hits = db_a.recall(&emb(seed_from("linguagem atual do projeto")), 8).unwrap_or_default();
    let a_lang = oldest_lang(&a_hits);
    // B: mesma chave + overwrite → recall default (active-only) só vê o corrente
    let b_hits = rerank_gate(db_b, "linguagem atual do projeto", 3).unwrap_or_default();
    let b_lang = if b_hits.iter().any(|h| h.text.contains("zig")) && !b_hits.iter().any(|h| h.text.contains("rust")) {
        "zig"
    } else {
        "rust"
    };
    let a_ok = a_lang == "zig";
    let b_ok = b_lang == "zig";
    (a_ok, b_ok, if a_ok { 1.0 } else { 0.0 }, if b_ok { 1.0 } else { 0.0 })
}

// ── quiz de recall estático: ambos saturam (3/3) ────────────────────────────
fn static_quiz(db_a: &mut Sgdb, db_b: &mut Sgdb) -> (usize, usize) {
    let facts = ["ana gosta de cafe", "rust e compilado", "sgdb e zero-dep"];
    for (i, txt) in facts.iter().enumerate() {
        let _ = db_a.remember_semantic(&format!("q/{i}"), txt, &emb(seed_from(txt)));
        let _ = db_b.remember_semantic(&format!("q/{i}"), txt, &emb(seed_from(txt)));
    }
    let mut ca = 0usize;
    let mut cb = 0usize;
    for q in ["ana gosta de", "rust e", "sgdb e"] {
        if db_a
            .recall(&emb(seed_from(q)), 4)
            .map(|hs| hs.iter().any(|h| h.text.contains(q.trim_end())))
            .unwrap_or(false)
        {
            ca += 1;
        }
        if db_b
            .recall(&emb(seed_from(q)), 4)
            .map(|hs| hs.iter().any(|h| h.text.contains(q.trim_end())))
            .unwrap_or(false)
        {
            cb += 1;
        }
    }
    (ca, cb)
}

fn main() {
    let now = 2_000u64;

    // quiz estático (instâncias frescas: só os 3 fatos)
    let mut qa = Sgdb::open(InMemory::new()).unwrap();
    let mut qb = Sgdb::open(InMemory::new()).unwrap();
    let (sc_a, sc_b) = static_quiz(&mut qa, &mut qb);

    // tarefas interdependentes (instâncias frescas por tarefa)
    let mut a1 = Sgdb::open(InMemory::new()).unwrap();
    let mut b1 = Sgdb::open(InMemory::new()).unwrap();
    let t1 = task_shopping(&mut a1, &mut b1, now);
    let mut a2 = Sgdb::open(InMemory::new()).unwrap();
    let mut b2 = Sgdb::open(InMemory::new()).unwrap();
    let t2 = task_formal(&mut a2, &mut b2, now);
    let mut a3 = Sgdb::open(InMemory::new()).unwrap();
    let mut b3 = Sgdb::open(InMemory::new()).unwrap();
    let t3 = task_lifecycle(&mut a3, &mut b3, now);

    let results = [t1, t2, t3];
    let names = [
        "shopping  (restrição escopada / P6 checkpoint)",
        "formal    (valor exato verbatim / P1 rerank gate)",
        "lifecycle (estado corrente / P2 supersede)",
    ];

    println!("=== memory-arena eval (determinístico, sem LLM) ===");
    println!("seção 1 — recall estático: A {sc_a}/3  B {sc_b}/3  (saturação: recall puro não distingue)");
    println!("seção 2 — tarefas agênticas interdependentes (SR = success rate, sPS = soft progress):");
    for (i, (a_ok, b_ok, a_soft, b_soft)) in results.iter().enumerate() {
        println!(
            "  {}: A SR={} sPS={:.1}  |  B SR={} sPS={:.1}",
            names[i],
            *a_ok as u8,
            a_soft,
            *b_ok as u8,
            b_soft
        );
    }
    let a_sr = results.iter().filter(|r| r.0).count();
    let b_sr = results.iter().filter(|r| r.1).count();
    let a_sps: f32 = results.iter().map(|r| r.2).sum();
    let b_sps: f32 = results.iter().map(|r| r.3).sum();
    println!(
        "  TOTAL: A SR={a_sr}/3 sPS={a_sps:.1}  |  B SR={b_sr}/3 sPS={b_sps:.1}"
    );

    let ok = sc_a == 3 && sc_b == 3 && b_sr > a_sr && b_sps > a_sps;
    println!(
        "veredito: {} (esperado: empate no quiz e B > A nas tarefas — memorização ≠ utilidade)",
        if ok { "PASS" } else { "FAIL" }
    );
    std::process::exit(if ok { 0 } else { 1 });
}