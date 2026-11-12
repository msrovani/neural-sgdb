//! longmemeval — benchmark PÚBLICO de retrieval (fase 1, release 1.5.7).
//!
//! POR QUE ISTO EXISTE: até a 1.5.6 todo número de retrieval do nsgdb vinha de
//! corpus sintético interno (`examples/bench.rs`, `BENCHMARKS.md`), sem ponto de
//! comparação externo. O ai-memory publica `hit@5` 0.951 no LongMemEval-S com
//! harness no repo; sem o mesmo dataset e a mesma estatística, "nosso recall é
//! bom" era opinião. Aqui o número passa a ser medido em 500 perguntas
//! públicas, com proveniência e por fatia.
//!
//! NÃO roda no `gate-all`: precisa de rede (dataset de 278 MB, não versionado)
//! e de minutos. O wrapper é `scripts/bench-longmemeval.sh`.
//!
//! ## Metodologia (o que torna o número comparável)
//!
//! - **Métrica**: `hit@k` = alguma sessão de evidência aparece no top-k
//!   (é o `Recall@k` que a maioria publica), `recall@k` = fração das sessões de
//!   evidência encontradas, `recall-all@k` = TODAS no top-k. Média por pergunta.
//! - **Abstention fora**: perguntas sem resposta não são acertáveis por
//!   retrieval; ficam de fora da conta e são reportadas separadamente.
//! - **Dois modos**, como o ai-memory decompõe:
//!   - `component` — cada sessão crua inteira indexada como um doc L3. Mede o
//!     RANKING.
//!   - `pipeline` — a mesma sessão com o corte de captura (2 KB, a fronteira de
//!     privacidade do harness real). Mede o custo de capturar, não de ranquear.
//!     A diferença entre os dois é o que a captura custa em qualidade.
//! - **Sem LLM, sem embedder, sem rede no scoring**: retrieval lexical (o
//!   default do nsgdb, ADR-0008). Determinístico: mesma DB + mesma query ⇒
//!   mesma ordem.
//! - **Isolamento por pergunta**: uma `Sgdb` nova por pergunta (o harness real
//!   usa um project por pergunta), então nenhuma pergunta vê o corpus de outra.
//!
//! ## Proveniência
//!
//! O relatório JSON carrega commit, sha256 do dataset, versão do crate, máquina,
//! modo, k e data — sem isso um número publicado não é reproduzível.
//!
//! Uso:
//!   cargo run --release --example longmemeval -- --dataset evals/datasets/longmemeval_oracle
//!   cargo run --release --example longmemeval -- --limit 20 --mode pipeline
//!   ... --json evals/runs/1.5.7-oracle.json --sha256 <hex> --commit <sha>

use neural_sgdb::{InMemory, RememberOptions, Sgdb};
use std::collections::BTreeMap;

const SCOPE: &str = "bench";
/// Fronteira de captura do `pipeline` — o mesmo 2 KB do hook do harness real.
const CAPTURE_CAP: usize = 2048;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Component,
    Pipeline,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Mode::Component => "component",
            Mode::Pipeline => "pipeline",
        }
    }
}

struct Args {
    dataset: String,
    mode: Mode,
    limit: usize,
    stratify: usize,
    k: usize,
    json: Option<String>,
    sha256: String,
    commit: String,
    /// Imprime, para as N primeiras perguntas, a evidência e as keys dos hits.
    /// Existe porque um harness que devolve 0.000 em tudo pode ser um ranking
    /// ruim OU um mapeamento de key errado — e esses dois pedem conserto oposto.
    dump: usize,
}

fn usage(msg: &str) -> ! {
    eprintln!("longmemeval: {msg}");
    eprintln!("uso: --dataset <arquivo> [--mode component|pipeline] [--limit N] [--stratify N] [--k 5] [--json <saida>] [--sha256 <hex>] [--commit <sha>]");
    std::process::exit(2);
}

fn parse_args() -> Args {
    let mut a = Args {
        dataset: "evals/datasets/longmemeval_oracle".to_string(),
        mode: Mode::Component,
        limit: 0,
        stratify: 0,
        k: 5,
        json: None,
        sha256: "unspecified".to_string(),
        commit: "unspecified".to_string(),
        dump: 0,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let next = |i: usize| -> String {
            argv.get(i + 1).cloned().unwrap_or_else(|| usage(&format!("{} exige valor", argv[i])))
        };
        match argv[i].as_str() {
            "--dataset" => { a.dataset = next(i); i += 2; }
            "--mode" => {
                a.mode = match next(i).as_str() {
                    "component" => Mode::Component,
                    "pipeline" => Mode::Pipeline,
                    other => usage(&format!("mode desconhecido: {other}")),
                };
                i += 2;
            }
            "--limit" => { a.limit = next(i).parse().unwrap_or(0); i += 2; }
            "--stratify" => { a.stratify = next(i).parse().unwrap_or(0); i += 2; }
            "--k" => { a.k = next(i).parse().unwrap_or(5).max(1); i += 2; }
            "--json" => { a.json = Some(next(i)); i += 2; }
            "--sha256" => { a.sha256 = next(i); i += 2; }
            "--commit" => { a.commit = next(i); i += 2; }
            "--dump" => { a.dump = next(i).parse().unwrap_or(0); i += 2; }
            "--help" | "-h" => usage("ajuda"),
            other => usage(&format!("flag desconhecida: {other}")),
        }
    }
    a
}

/// Uma linha do relatório por pergunta.
struct Scored {
    qtype: String,
    hit: bool,
    recall: f64,
    recall_all: bool,
}

fn as_array<'a>(v: &'a serde_json::Value, what: &str) -> &'a Vec<serde_json::Value> {
    v.as_array().unwrap_or_else(|| usage(&format!("campo {what} não é array")))
}

fn as_str<'a>(v: &'a serde_json::Value, what: &str) -> &'a str {
    v.as_str().unwrap_or_else(|| usage(&format!("campo {what} não é string")))
}

/// Texto da sessão: cada turno com papel e a data da sessão no início.
///
/// A data entra porque um histórico replicado carrega os próprios timestamps —
/// o harness oficial expõe a mesma informação ao retriever. Sem ela, perguntas
/// temporais (`before`, `after`, `first`) não são respondíveis por retrieval.
fn session_text(turns: &[serde_json::Value], date: &str, cap_per_turn: Option<usize>) -> String {
    let mut out = String::with_capacity(512);
    for t in turns {
        let role = t.get("role").and_then(|r| r.as_str()).unwrap_or("?");
        let mut content = t.get("content").and_then(|c| c.as_str()).unwrap_or("");
        // O teto e POR TURNO, como na captura real: cada observacao e limitada
        // a 2 KB e TODAS sao indexadas. Cortar a sessao inteira nos primeiros
        // 2 KB descartava os turnos finais (onde a resposta costuma estar) e
        // media uma captura que ninguem escreveria — foi o que derrubou
        // single-session-user de 0.971 para 0.500.
        if let Some(cap) = cap_per_turn {
            if content.len() > cap {
                let mut cut = cap;
                while cut > 0 && !content.is_char_boundary(cut) {
                    cut -= 1;
                }
                content = &content[..cut];
            }
        }
        out.push('[');
        out.push_str(date);
        out.push_str("] ");
        out.push_str(role);
        out.push_str(": ");
        out.push_str(content);
        out.push('\n');
    }
    out
}

/// Sessão de origem de uma key de hit.
///
/// `Hit.key` é a key CANÔNICA do storage (`md/L3/sess/<sid>`), não a key crua
/// que o caller passou ao `remember` — comparar com a crua dava 0.000 em tudo
/// com o ranking PERFEITO (o bug que o `--dump` pegou: top-3 = toda a
/// evidência, score zero). Procura `sess/` em qualquer posição e devolve o
/// primeiro componente depois dele; sids do dataset nunca contêm `/`.
fn session_of(key: &str) -> &str {
    match key.find("sess/") {
        Some(p) => {
            let rest = &key[p + "sess/".len()..];
            rest.split('/').next().unwrap_or(rest)
        }
        None => key.split('/').next().unwrap_or(key),
    }
}

fn run_question(q: &serde_json::Value, mode: Mode, k: usize, dump: bool) -> Scored {
    let qid = as_str(&q["question_id"], "question_id");
    let qtype = as_str(&q["question_type"], "question_type");
    let question = as_str(&q["question"], "question");
    let sessions = as_array(&q["haystack_sessions"], "haystack_sessions");
    let dates = as_array(&q["haystack_dates"], "haystack_dates");
    let sids = as_array(&q["haystack_session_ids"], "haystack_session_ids");
    let evidence: Vec<String> = as_array(&q["answer_session_ids"], "answer_session_ids")
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();

    let mut db = Sgdb::open(InMemory::new()).expect("open InMemory");
    let cap = match mode {
        Mode::Component => None,
        Mode::Pipeline => Some(CAPTURE_CAP),
    };
    for (i, turns) in sessions.iter().enumerate() {
        let sid = sids
            .get(i)
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| usage(&format!("{qid}: sessão {i} sem id")));
        let date = dates.get(i).and_then(|v| v.as_str()).unwrap_or("unknown");
        let text = session_text(as_array(turns, "sessão"), date, cap);
        if text.is_empty() {
            continue;
        }
        // `/s` terminador: as session ids do dataset se sobrepõem por prefixo
        // (`..._1` é prefixo de `..._10`) e a ART recusa prefix-key. Sem o
        // sufixo, a 21ª pergunta já estoura com "storage key is a prefix".
        db.remember_text_with(
            &format!("sess/{sid}/s"),
            &text,
            RememberOptions {
                scope: Some(SCOPE),
                entities: &[],
                content_type: Some("text"),
                scope_dims: None,
                model_id: None,
                index_key: false,
            },
        )
        .expect("remember da sessão");
    }

    let hits = db
        .recall_lexical_scoped(question, k, SCOPE)
        .expect("recall lexical escopado");
    if dump {
        println!(
            "DUMP {qid} [{qtype}] evid={:?} hits={:?}",
            evidence,
            hits.iter().map(|h| h.key.clone()).collect::<Vec<_>>()
        );
    }
    let found: Vec<String> = hits.iter().map(|h| session_of(&h.key).to_string()).collect();
    let hit = evidence.iter().any(|e| found.iter().any(|f| f == e));
    let matched = evidence.iter().filter(|e| found.iter().any(|f| &f == e)).count();
    let recall = if evidence.is_empty() { 0.0 } else { matched as f64 / evidence.len() as f64 };
    Scored { qtype: qtype.to_string(), hit, recall, recall_all: matched == evidence.len() && !evidence.is_empty() }
}

#[derive(Default)]
struct Slice {
    n: usize,
    hit: usize,
    recall_sum: f64,
    recall_all: usize,
}

impl Slice {
    fn add(&mut self, s: &Scored) {
        self.n += 1;
        self.hit += usize::from(s.hit);
        self.recall_sum += s.recall;
        self.recall_all += usize::from(s.recall_all);
    }
    fn hit_rate(&self) -> f64 { if self.n == 0 { 0.0 } else { self.hit as f64 / self.n as f64 } }
    fn recall_mean(&self) -> f64 { if self.n == 0 { 0.0 } else { self.recall_sum / self.n as f64 } }
    fn recall_all_rate(&self) -> f64 { if self.n == 0 { 0.0 } else { self.recall_all as f64 / self.n as f64 } }
}

fn main() {
    let args = parse_args();
    let started = std::time::Instant::now();
    let raw = std::fs::read_to_string(&args.dataset).unwrap_or_else(|e| {
        usage(&format!("não consegui ler {}: {e} (rode scripts/fetch-longmemeval.sh)", args.dataset))
    });
    let data: serde_json::Value = serde_json::from_str(&raw)
        .unwrap_or_else(|e| usage(&format!("{} não é JSON válido: {e}", args.dataset)));
    let questions = as_array(&data, "raiz");

    // Seleção: prefixo (--limit) ou N por fatia (--stratify, cobre todos os
    // tipos barato). `--stratify` vence `--limit`.
    let mut picked: Vec<&serde_json::Value> = Vec::new();
    if args.stratify > 0 {
        let mut per_type: BTreeMap<String, usize> = BTreeMap::new();
        for q in questions {
            let t = q["question_type"].as_str().unwrap_or("?").to_string();
            let c = per_type.entry(t).or_insert(0);
            if *c < args.stratify {
                *c += 1;
                picked.push(q);
            }
        }
    } else if args.limit > 0 {
        picked.extend(questions.iter().take(args.limit));
    } else {
        picked.extend(questions.iter());
    }

    let mut by_type: BTreeMap<String, Slice> = BTreeMap::new();
    let mut overall = Slice::default();
    let mut abstention_total = 0usize;
    let mut per_question: Vec<serde_json::Value> = Vec::new();

    for q in &picked {
        let evidence = q["answer_session_ids"].as_array().map(|a| a.len()).unwrap_or(0);
        // Abstention: sem sessão de evidência não há o que recuperar; entra na
        // conta separada e NUNCA no hit@k (creditar acerto aqui inflaria tudo).
        if evidence == 0 {
            abstention_total += 1;
            continue;
        }
        let s = run_question(q, args.mode, args.k, args.dump > per_question.len());
        by_type.entry(s.qtype.clone()).or_default().add(&s);
        overall.add(&s);
        per_question.push(serde_json::json!({
            "question_id": q["question_id"],
            "question_type": s.qtype,
            "hit": s.hit,
            "recall": s.recall,
            "recall_all": s.recall_all,
        }));
    }

    println!("=== longmemeval ({}) — modo {} — k={} ===", args.dataset, args.mode.label(), args.k);
    println!("perguntas avaliadas: {} | abstention (fora da conta): {}", overall.n, abstention_total);
    println!();
    println!("{:<28} {:>5} {:>8} {:>9} {:>10}", "fatia", "n", "hit@k", "recall@k", "recall-all");
    for (t, s) in &by_type {
        println!("{:<28} {:>5} {:>8.3} {:>9.3} {:>10.3}", t, s.n, s.hit_rate(), s.recall_mean(), s.recall_all_rate());
    }
    println!(
        "{:<28} {:>5} {:>8.3} {:>9.3} {:>10.3}",
        "OVERALL", overall.n, overall.hit_rate(), overall.recall_mean(), overall.recall_all_rate()
    );
    println!();
    println!("tempo: {:.1}s ({} perguntas)", started.elapsed().as_secs_f64(), overall.n);

    if let Some(path) = &args.json {
        let report = serde_json::json!({
            "harness": "examples/longmemeval.rs",
            "metric": "hit@k = any evidence session in top-k; recall@k = fraction found; abstention excluded",
            "crate_version": env!("CARGO_PKG_VERSION"),
            "commit": args.commit,
            "dataset": args.dataset,
            "dataset_sha256": args.sha256,
            "mode": args.mode.label(),
            "capture_cap_bytes": if args.mode == Mode::Pipeline { Some(CAPTURE_CAP) } else { None },
            "k": args.k,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "evaluated": overall.n,
            "abstention_excluded": abstention_total,
            "overall": {
                "hit": overall.hit_rate(),
                "recall": overall.recall_mean(),
                "recall_all": overall.recall_all_rate(),
            },
            "slices": by_type.iter().map(|(t, s)| (t.clone(), serde_json::json!({
                "n": s.n, "hit": s.hit_rate(), "recall": s.recall_mean(), "recall_all": s.recall_all_rate(),
            }))).collect::<BTreeMap<_, _>>(),
            "questions": per_question,
        });
        if let Some(dir) = std::path::Path::new(path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(path, serde_json::to_string_pretty(&report).expect("json"))
            .unwrap_or_else(|e| usage(&format!("não consegui escrever {path}: {e}")));
        println!("relatório: {path}");
    }
}
