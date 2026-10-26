//! Aprendizado por trajetória — a convenção do HOST (v1.5.0, ADR-0019).
//!
//! O Ruflo chama isto de *trajectory contrastive learning*: guardar o
//! histórico de tentativas (o que foi tentado, o padrão que funcionou, o erro
//! que repetiu) e ajustar o peso das memórias conforme o DESFECHO real. No
//! neural-sgdb isso **não é um primitivo novo** — é disciplina de uso de duas
//! coisas que já existem:
//!
//! - `remember_episodic` — a TRAJETÓRIA verbatim (tarefa + desfecho), nunca
//!   resumida (achado MemoryArena: verbatim > abstração);
//! - `feedback(key, positivo, amount)` — a re-ponderação CONTRASTIVA de
//!   importance **e** confidence da memória que foi USADA: o par
//!   "o que eu escolhi" + "como terminou".
//!
//! O core não decide nada disto. Ele guarda o desfecho e re-pondera o peso; o
//! ROTEIRO (qual braço tentar, quando explorar, quando parar de tentar) é do
//! host. Este exemplo é o roteiro mínimo + as medições, no estilo MemoryArena:
//! duas configurações sobre a MESMA bateria determinística.
//!
//! - **Config A (hoarder)** — escreve tudo (estratégias + episódios) e NUNCA
//!   re-pondera: o ranking do 1-hop fica no desempate pela key (alfabética).
//!   É o "agente que acumula memória" — memória sem desfecho não ensina.
//! - **Config B (trajetória)** — mesmo corpus e MESMO custo de exploração, com
//!   `feedback` por desfecho: o braço que funciona sobe, o que falha DESCE.
//!
//! Medido e pinado por auto-check:
//! 1. o custo de exploração é pago UMA vez (a política gulosa pura não
//!    descobre o segundo braço) e depois satura em 100% — em A, o mesmo custo
//!    não muda nada: SR 50% em toda época;
//! 2. a sessão 2 acerta desde a PRIMEIRA tarefa (o aprendizado sobrevive ao fim
//!    da sessão — a "persistência" que o Ruflo promete);
//! 3. contraste nos DOIS sentidos: importance/confidence do braço vencedor
//!    acima do perdedor, e o perdedor DESCE abaixo do valor inicial;
//! 4. a trilha: 1 par de episódios verbatim por tentativa, contável por braço
//!    (entity `traj/<classe>/<braço>`) — nada extraído de texto pelo core;
//! 5. exploração é POLÍTICA do host (o core não explora sozinho): o braço novo
//!    é tentado uma vez por decisão do host e, falhando, o roteador guloso
//!    nunca o escolhe de novo.
//!
//! Uso:
//! ```text
//! cargo run --release --example trajectory_learning
//! ```
//! Exit code 0 sse todas as asserções passaram.

use neural_sgdb::{Hit, InMemory, RememberOptions, Sgdb, SgdbError};

// ── reporter minimal (PASS/FAIL) ────────────────────────────────────────────
struct Rep {
    checks: Vec<(String, bool, String)>,
}
impl Rep {
    fn new() -> Rep {
        Rep { checks: Vec::new() }
    }
    fn check(&mut self, name: &str, ok: bool, detail: String) {
        println!("{} {}", if ok { "PASS" } else { "FAIL" }, name);
        if !ok {
            println!("      {detail}");
        }
        self.checks.push((name.to_string(), ok, detail));
    }
    fn done(&self) -> bool {
        let fails = self.checks.iter().filter(|c| !c.1).count();
        println!("\nasserções: {} total, {} falhas", self.checks.len(), fails);
        fails == 0
    }
}

// ── o AMBIENTE (determinístico): o "mundo" que julga cada tentativa ─────────
// O core não sabe nada disto — quem mede o desfecho é o host.
const CLASSES: [&str; 2] = ["fix", "refactor"];
const STRATEGIES: [&str; 2] = ["grep-first", "read-all"];

/// Gabarito do ambiente: qual braço funciona em qual classe de tarefa.
fn works(class: &str, strategy: &str) -> bool {
    matches!(
        (class, strategy),
        ("fix", "grep-first") | ("refactor", "read-all")
    )
}

// ── a ONTOLOGIA (strings canônicas: as MESMAS na escrita e na busca) ───────
fn strat_key(class: &str, strategy: &str) -> String {
    format!("strat/{class}/{strategy}")
}
fn strat_entity(class: &str) -> String {
    format!("strategy/{class}")
}
fn traj_entity(class: &str, strategy: &str) -> String {
    format!("traj/{class}/{strategy}")
}

/// Escreve a memória de um braço (uma vez por par classe/braço): é a memória
/// que o roteador consulta e que o desfecho vai re-ponderar.
fn seed_strategy(db: &mut Sgdb, class: &str, strategy: &str) -> Result<(), SgdbError> {
    let ent = strat_entity(class);
    let ents = [ent.as_str(), "strategy"];
    db.remember_text_with(
        &strat_key(class, strategy),
        &format!("estrategia {strategy} para a classe {class}"),
        RememberOptions {
            entities: &ents,
            ..Default::default()
        },
    )?;
    Ok(())
}

/// Pesos atuais do braço (importance, confidence) — o que o `feedback` move.
fn arm_state(db: &mut Sgdb, class: &str, strategy: &str) -> (f32, f32) {
    db.meta(&strat_key(class, strategy))
        .ok()
        .flatten()
        .map(|m| (m.importance, m.confidence))
        .unwrap_or((-1.0, -1.0))
}

/// O ROTEADOR (política do host): escolhe o braço pela MEMÓRIA — 1-hop por
/// entity, cujo ranking já é determinístico (overlap desc → importance desc →
/// key asc). Nenhum braço na memória ainda → `None` (o host decide).
fn pick(db: &mut Sgdb, class: &str) -> Option<String> {
    let ent = strat_entity(class);
    let hits = db.recall_entities(&[ent.as_str()], 16).ok()?;
    let sk = hits.first()?.key.clone();
    // `md/L3/strat/<classe>/<braço>` → braço
    Some(sk.rsplit('/').next().unwrap_or_default().to_string())
}

struct Attempt {
    class: String,
    strategy: String,
    ok: bool,
}

/// Uma TENTATIVA: o host escolhe o braço, o ambiente julga e o host registra a
/// trajetória (episódio verbatim + entity da trilha). Com `learn`, o desfecho
/// re-pondera a memória do braço USADO — o passo contrastivo.
fn attempt_with(
    db: &mut Sgdb,
    class: &str,
    task: &str,
    strategy: &str,
    learn: bool,
    now: u64,
) -> Attempt {
    let ok = works(class, strategy);
    let (u, a) = db
        .remember_episodic(
            &format!("tarefa {task} (classe {class})"),
            &format!(
                "estrategia {strategy} -> {}",
                if ok { "sucesso" } else { "falha" }
            ),
            now,
        )
        .expect("episodio verbatim");
    // a TRILHA: a tentativa fica contável por braço (nada é extraído do texto;
    // a entity é DECLARADA pelo host, mesmo contrato de sempre)
    let te = traj_entity(class, strategy);
    db.set_entities(&u, &[te.as_str()]).expect("trilha u");
    db.set_entities(&a, &[te.as_str()]).expect("trilha a");
    // a lição aponta para a evidência que a produziu (P3: reflection grounding)
    db.associate_checked(&u, neural_sgdb::RelationKind::DerivedFrom, &strat_key(class, strategy))
        .expect("derived_from da estrategia");
    if learn {
        // CONTRASTE: o desfecho move importance E confidence, para cima ou
        // para baixo (não é só reforço positivo do vencedor).
        db.feedback(&strat_key(class, strategy), ok, 0.1)
            .expect("feedback do desfecho");
    }
    Attempt {
        class: class.to_string(),
        strategy: strategy.to_string(),
        ok,
    }
}

/// Uma tentativa ROTEADA (o caminho normal do agente): se ainda não há memória
/// do braço, o host semeia os candidatos e começa pelo primeiro (cold start).
fn attempt_routed(db: &mut Sgdb, class: &str, task: &str, learn: bool, now: u64) -> Attempt {
    let strategy = match pick(db, class) {
        Some(s) => s,
        None => {
            for s in STRATEGIES {
                seed_strategy(db, class, s).expect("seed dos candidatos");
            }
            STRATEGIES[0].to_string()
        }
    };
    attempt_with(db, class, task, &strategy, learn, now)
}

/// A EXPLORAÇÃO é decisão do HOST (o core nunca explora sozinho): uma
/// tentativa por braço, para que o desfecho de cada um fique na memória.
fn run_exploration(
    db: &mut Sgdb,
    learn: bool,
    label: &str,
    clock: &mut u64,
) -> Vec<Attempt> {
    let mut log = Vec::new();
    for class in CLASSES {
        for s in STRATEGIES {
            *clock += 1_000;
            let task = format!("{label}-{class}-{s}");
            log.push(attempt_with(db, class, &task, s, learn, *clock));
        }
    }
    log
}

/// Uma ÉPOCA da bateria em regime de EXPLORAÇÃO ZERO (só o roteador decide):
/// `tasks_per_class` tarefas por classe, intercaladas (determinístico).
fn run_epoch(
    db: &mut Sgdb,
    learn: bool,
    tasks_per_class: usize,
    label: &str,
    clock: &mut u64,
) -> Vec<Attempt> {
    let mut log = Vec::new();
    for i in 0..tasks_per_class {
        for class in CLASSES {
            *clock += 1_000;
            let task = format!("{label}-{class}-{i}");
            log.push(attempt_routed(db, class, &task, learn, *clock));
        }
    }
    log
}

fn sr(log: &[Attempt]) -> (usize, usize) {
    (log.iter().filter(|a| a.ok).count(), log.len())
}

fn sr_of(log: &[Attempt], class: &str) -> (usize, usize) {
    let sub: Vec<&Attempt> = log.iter().filter(|a| a.class == class).collect();
    (sub.iter().filter(|a| a.ok).count(), sub.len())
}

fn pct(ok: usize, total: usize) -> String {
    format!("{ok}/{total} ({:.0}%)", 100.0 * ok as f64 / total.max(1) as f64)
}

fn main() {
    let mut rep = Rep::new();
    let mut clock: u64 = 1_000;

    // ── as duas configurações, com o MESMO corpus ──────────────────────────
    let mut naive = Sgdb::open(InMemory::new()).expect("open");
    let mut traj = Sgdb::open(InMemory::new()).expect("open");
    for db in [&mut naive, &mut traj] {
        for class in CLASSES {
            for s in STRATEGIES {
                seed_strategy(db, class, s).expect("seed");
            }
        }
    }
    // pesos INICIAIS (antes de qualquer desfecho): a linha de base do contraste
    let mut initial: Vec<(String, String, f32, f32)> = Vec::new();
    for class in CLASSES {
        for s in STRATEGIES {
            let (i, c) = arm_state(&mut traj, class, s);
            initial.push((class.to_string(), s.to_string(), i, c));
        }
    }
    println!(
        "pesos iniciais (sem nenhum desfecho): importance={} confidence={}",
        initial[0].2, initial[0].3
    );

    // ── Config A: o "hoarder" (memória sem desfecho) ───────────────────────
    println!("\n== Config A: hoarder (escreve, nunca re-pondera) ==");
    let mut clock_a = clock;
    let a_exp = run_exploration(&mut naive, false, "aexp", &mut clock_a);
    let a1 = run_epoch(&mut naive, false, 4, "a1", &mut clock_a);
    let a2 = run_epoch(&mut naive, false, 4, "a2", &mut clock_a);
    let (ae_ok, ae_n) = sr(&a_exp);
    let (a1_ok, a1_n) = sr(&a1);
    let (a2_ok, a2_n) = sr(&a2);
    println!(
        "   exploracao: {} | epoca 1: {} | epoca 2: {}",
        pct(ae_ok, ae_n),
        pct(a1_ok, a1_n),
        pct(a2_ok, a2_n)
    );
    rep.check(
        "A: a exploracao paga o custo e NAO ensina nada (SR roteado == exploracao)",
        a1_ok * ae_n == ae_ok * a1_n,
        format!("exploracao={ae_ok}/{ae_n} roteado={a1_ok}/{a1_n}"),
    );
    rep.check(
        "A: acumular memória sem desfecho nao muda o SR entre epocas (50% travado)",
        (a1_ok, a1_n) == (a2_ok, a2_n) && a1_ok < a1_n,
        format!("ep1={a1_ok}/{a1_n} ep2={a2_ok}/{a2_n}"),
    );

    // ── Config B: aprendizado por trajetória ───────────────────────────────
    println!("\n== Config B: trajetoria (feedback por desfecho) ==");
    let b_exp = run_exploration(&mut traj, true, "bexp", &mut clock);
    let b1 = run_epoch(&mut traj, true, 4, "b1", &mut clock);
    let b2 = run_epoch(&mut traj, true, 4, "b2", &mut clock);
    let (be_ok, be_n) = sr(&b_exp);
    let (b1_ok, b1_n) = sr(&b1);
    let (b2_ok, b2_n) = sr(&b2);
    println!(
        "   exploracao: {} | epoca 1: {} | epoca 2: {}",
        pct(be_ok, be_n),
        pct(b1_ok, b1_n),
        pct(b2_ok, b2_n)
    );
    rep.check(
        "B: o MESMO custo de exploracao vira aprendizado (SR roteado > exploracao)",
        b1_ok * be_n > be_ok * b1_n,
        format!("exploracao={be_ok}/{be_n} roteado={b1_ok}/{b1_n}"),
    );
    rep.check(
        "B: o roteador guloso satura nas epocas roteadas (100%)",
        b1_ok == b1_n && b2_ok == b2_n,
        format!("ep1={b1_ok}/{b1_n} ep2={b2_ok}/{b2_n}"),
    );
    rep.check(
        "B > A nas epocas roteadas (memoria que aprendeu o desfecho)",
        b2_ok * a2_n > a2_ok * b2_n,
        format!("A={a2_ok}/{a2_n} B={b2_ok}/{b2_n}"),
    );
    // satura nas DUAS classes (nao e um braco sortudo por classe)
    let mut classes_ok = true;
    let mut detail = String::new();
    for class in CLASSES {
        let (ok, n) = sr_of(&b2, class);
        classes_ok &= ok == n && n > 0;
        detail.push_str(&format!(" {class}={ok}/{n}"));
    }
    rep.check(
        "B: as DUAS classes saturam na epoca 2",
        classes_ok,
        detail.trim().to_string(),
    );

    // ── sessão 2: o aprendizado sobrevive ao fim da sessão ─────────────────
    // (mesma DB; o host "fecha" a sessão e o roteador recomeça do zero. Em
    // FileStorage o mecanismo é open+rebuild — aqui o que se prova é que a
    // DECISÃO não depende de estado de processo: ela mora na memória.)
    println!("\n== sessao 2: roteador novo, memoria antiga ==");
    let first = attempt_routed(&mut traj, "fix", "s2-fix-0", true, clock + 1_000);
    let second = attempt_routed(&mut traj, "refactor", "s2-ref-0", true, clock + 2_000);
    rep.check(
        "sessao 2: a PRIMEIRA tentativa de cada classe ja usa o braco certo",
        first.ok && second.ok,
        format!("fix={} refactor={}", first.strategy, second.strategy),
    );

    // ── contraste (nos DOIS sentidos) ──────────────────────────────────────
    println!("\n== contraste: o vencedor sobe, o perdedor DESCE ==");
    for class in CLASSES {
        let winner = STRATEGIES
            .iter()
            .find(|s| works(class, s))
            .copied()
            .unwrap_or(STRATEGIES[0]);
        let loser = STRATEGIES
            .iter()
            .find(|s| !works(class, s))
            .copied()
            .unwrap_or(STRATEGIES[1]);
        let (w_i, w_c) = arm_state(&mut traj, class, winner);
        let (l_i, l_c) = arm_state(&mut traj, class, loser);
        rep.check(
            &format!("contraste[{class}]: importance vencedor({winner}) > perdedor({loser})"),
            w_i > l_i,
            format!("winner={w_i} loser={l_i}"),
        );
        rep.check(
            &format!("contraste[{class}]: confidence vencedor > perdedor"),
            w_c > l_c,
            format!("winner={w_c} loser={l_c}"),
        );
        // o perdedor DESCEU de verdade em relação ao estado INICIAL (o sinal
        // negativo move os dois pesos — não é só reforço do vencedor).
        let init = initial
            .iter()
            .find(|(c, s, _, _)| c == class && s == loser)
            .map(|(_, _, i, c)| (*i, *c))
            .unwrap_or((0.0, 0.0));
        rep.check(
            &format!("contraste[{class}]: o braco que falha DESCE abaixo do inicial"),
            l_i < init.0 && l_c < init.1,
            format!(
                "inicial=({}, {}) final=({l_i}, {l_c})",
                init.0, init.1
            ),
        );
    }

    // ── a trilha: contável, verbatim, ligada à estratégia ──────────────────
    println!("\n== trilha: 1 par de episodios por tentativa ==");
    // uma tentativa de prova, VERBATIM, com a mesma trilha das outras: o core
    // guarda o par cru (u/a) e nada mais.
    let (u, a) = traj
        .remember_episodic(
            "tarefa probe-verbatim",
            "estrategia grep-first -> falha",
            clock + 9_000,
        )
        .expect("episodio de prova");
    traj.set_entities(&u, &["traj/fix/grep-first"])
        .expect("trilha da prova u");
    traj.set_entities(&a, &["traj/fix/grep-first"])
        .expect("trilha da prova a");
    let mut total_trail = 0usize;
    for class in CLASSES {
        for s in STRATEGIES {
            let te = traj_entity(class, s);
            let hits: Vec<Hit> = traj.recall_entities(&[te.as_str()], 500).expect("trilha");
            // cada tentativa grava o PAR cru (u + a) do episodio
            total_trail += hits.len();
        }
    }
    // tentativas de B: exploracao + 2 epocas + as 2 da sessao 2 + a de prova
    let attempts_b = be_n + b1_n + b2_n + 2 + 1;
    rep.check(
        "trilha: 2 docs por tentativa (o par u/a do episodio verbatim)",
        total_trail == attempts_b * 2,
        format!("trilha={total_trail} tentativas={attempts_b}"),
    );
    let pedido = traj.text_of(&u).expect("texto do episodio");
    let resposta = traj.text_of(&a).expect("texto do desfecho");
    rep.check(
        "trilha: o episodio e VERBATIM (tarefa e desfecho crus, nada extraido)",
        pedido.contains("probe-verbatim") && resposta.contains("grep-first") && resposta.contains("falha"),
        format!("u={pedido:?} a={resposta:?}"),
    );
    let tentativas_braco = traj
        .recall_entities(&["traj/fix/grep-first"], 500)
        .expect("hits");
    rep.check(
        "trilha: recall por entity do braco devolve as tentativas dele",
        !tentativas_braco.is_empty(),
        format!("{} docs de traj/fix/grep-first", tentativas_braco.len()),
    );

    // ── exploração é POLÍTICA do host (o core não explora sozinho) ─────────
    println!("\n== exploracao: decisao do host, desfecho na memoria ==");
    let before = pick(&mut traj, "fix").unwrap_or_default();
    seed_strategy(&mut traj, "fix", "monorepo-index").expect("braco novo");
    rep.check(
        "o roteador GULOSO nao arrisca o braco novo sozinho (o core nao explora)",
        pick(&mut traj, "fix").as_deref() == Some(before.as_str()),
        format!("pick={:?}", pick(&mut traj, "fix")),
    );
    // o host decide explorar: UMA tentativa forçada, com o desfecho gravado
    let explore = attempt_with(
        &mut traj,
        "fix",
        "explore-fix",
        "monorepo-index",
        true,
        clock + 10_000,
    );
    let (new_i, _) = arm_state(&mut traj, "fix", "monorepo-index");
    let (best_i, _) = arm_state(&mut traj, "fix", &before);
    rep.check(
        "exploracao: o braco novo falha e DESCE abaixo do incumbente",
        !explore.ok && new_i < best_i,
        format!("novo={new_i} incumbente={best_i}"),
    );
    let after = pick(&mut traj, "fix").unwrap_or_default();
    rep.check(
        "exploracao encerrada: o roteador volta ao incumbente (custo = 1 falha)",
        after == before && after != "monorepo-index",
        format!("pick={after}"),
    );

    // ── veredito ───────────────────────────────────────────────────────────
    println!(
        "\nveredito: SR A={} vs SR B={} (mesmo custo de exploracao {})",
        pct(a2_ok, a2_n),
        pct(b2_ok, b2_n),
        pct(be_ok, be_n)
    );
    if rep.done() {
        std::process::exit(0);
    }
    std::process::exit(1);
}
