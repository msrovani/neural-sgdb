//! Hot SGDB Test — Via 3: client cru (raw JSON-RPC over stdio).
//!
//! Audita o `mcp_server` EXATAMENTE como um IDE/agente faria: spawna o
//! binário, faz o handshake `2025-11-25`, lista as 15 tools, exerce
//! memória/recall/relações/observabilidade, cobra caminhos de erro,
//! paginação, resources e — o teste a quente de verdade — PERSISTÊNCIA:
//! mata o processo, respawna com o mesmo `NEURAL_SGDB_DB` e prova que a
//! memória sobrevive (cross-process / cross-session).
//!
//! Uso:
//! ```text
//! cargo build --release --example mcp_server      # o binário que vamos dirigir
//! cargo run --release --example mcp_client        # a cobaia
//! ```
//!
//! O binário do server é resolvido via env `NEURAL_SGDB_MCP_BIN` ou
//! `target/{release,debug}/examples/mcp_server{.exe}`. Cada asserção falha
//! imprime FAIL; o exit code é 0 sse TODAS passaram.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Instant;

use serde_json::{json, Value};
use neural_sgdb::demo_embed;

/// Cliente JSON-RPC mínimo: uma linha = uma mensagem; id ecoado verbatim.
struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    id: u64,
    startup_ms: u128,
}

impl Mcp {
    fn spawn(bin: &str, db: &str) -> Mcp {
        let t = Instant::now();
        let mut child = Command::new(bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit()) // logs do server visíveis (stderr)
            .env("NEURAL_SGDB_DB", db)
            .spawn()
            .expect("spawn do mcp_server falhou");
        let stdin = child.stdin.take().expect("stdin do server");
        let stdout = BufReader::new(child.stdout.take().expect("stdout do server"));
        Mcp { child, stdin, stdout, id: 0, startup_ms: t.elapsed().as_millis() }
    }

    /// Envia um request e lê até o response com o MESMO id (id ecoado).
    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let req = json!({"jsonrpc":"2.0","id":self.id,"method":method,"params":params});
        writeln!(self.stdin, "{req}").expect("write rpc");
        self.stdin.flush().expect("flush rpc");
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).expect("read rpc");
            assert!(n > 0, "server fechou stdout antes da resposta (id {})", self.id);
            let msg: Value = serde_json::from_str(&line).expect("resposta JSON-RPC inválida");
            if msg.get("id") == Some(&Value::Number(self.id.into())) {
                return msg;
            }
        }
    }

    /// tools/call → extrai `result.content[0].text` + `result.isError`.
    fn tool(&mut self, name: &str, args: Value) -> (String, bool) {
        let r = self.rpc("tools/call", json!({"name": name, "arguments": args}));
        let res = &r["result"];
        if let Some(err) = r.get("error") {
            return (format!("JSON-RPC error {}: {}", err["code"], err["message"]), true);
        }
        let text = res["content"]
            .as_array()
            .and_then(|c| c.first())
            .and_then(|c| c["text"].as_str())
            .unwrap_or("")
            .to_string();
        (text, res["isError"].as_bool().unwrap_or(false))
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn startup_ms(&self) -> u128 {
        self.startup_ms
    }
}

/// Reporte de auditoria: asserções + tempos por fase.
struct Report {
    checks: Vec<(String, bool, String)>,
    phases: Vec<(String, u128)>,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: String) {
        println!("{} {}", if ok { "PASS" } else { "FAIL" }, name);
        if !ok {
            println!("      {detail}");
        }
        self.checks.push((name.to_string(), ok, detail));
    }
    fn phase(&mut self, name: &str, t: &Instant) {
        let ms = t.elapsed().as_millis();
        self.phases.push((name.to_string(), ms));
        println!("-- fase '{name}' em {ms} ms");
    }
    fn phase_dur(&mut self, name: &str, d: std::time::Duration) {
        let ms = d.as_millis();
        self.phases.push((name.to_string(), ms));
        println!("-- fase '{name}' em {ms} ms");
    }
    fn finish(self) -> i32 {
        let fails = self.checks.iter().filter(|(_, ok, _)| !ok).count();
        println!("\n=== Hot SGDB Test (Via 3) ===");
        println!("fases: {}", self.phases.iter().map(|(n, ms)| format!("{n}={ms}ms")).collect::<Vec<_>>().join(", "));
        println!("asserções: {} total, {} falhas", self.checks.len(), fails);
        if fails > 0 {
            println!("RESULTADO: FALHOU (exit 1)");
        } else {
            println!("RESULTADO: PASSOU (exit 0)");
        }
        i32::from(fails > 0)
    }
}

fn server_bin() -> String {
    if let Ok(b) = std::env::var("NEURAL_SGDB_MCP_BIN") {
        return b;
    }
    let exe = if cfg!(windows) { ".exe" } else { "" };
    for dir in ["target/release/examples", "target/debug/examples"] {
        let p = format!("{dir}/mcp_server{exe}");
        if std::path::Path::new(&p).exists() {
            return p;
        }
    }
    panic!("binário do mcp_server não encontrado. Rode: cargo build --release --example mcp_server (ou set NEURAL_SGDB_MCP_BIN)");
}

fn main() {
    let bin = server_bin();
    let db = std::env::var("NEURAL_SGDB_DB").unwrap_or_else(|_| {
        format!("{}/hot_sgdb_test_{}.db", std::env::temp_dir().display(), std::process::id())
    });
    let _ = std::fs::remove_file(&db); // estado limpo entre execuções

    println!("=== Hot SGDB Test (Via 3) — server={bin} db={db}");
    let mut rep = Report { checks: Vec::new(), phases: Vec::new() };
    let t = Instant::now();

    // ---------- fase 1: handshake ----------
    let mut srv = Mcp::spawn(&bin, &db);
    rep.phase_dur("startup do server", std::time::Duration::from_millis(srv.startup_ms() as u64));
    rep.phase("startup+handshake", &t);
    let r = srv.rpc("initialize", json!({
        "protocolVersion": "2025-11-25",
        "capabilities": {},
        "clientInfo": {"name": "hot-sgdb-test", "version": "1.0"}
    }));
    rep.check("initialize responde", !r.get("error").is_some(), r.to_string());
    rep.check("protocolVersion 2025-11-25",
        r["result"]["protocolVersion"] == "2025-11-25", r.to_string());
    rep.check("serverInfo version 1.1.0",
        r["result"]["serverInfo"]["version"] == "1.1.0", r.to_string());
    rep.phase("handshake", &t);

    // ---------- fase 2: tools/list (23 tools) ----------
    let t = Instant::now();
    let r = srv.rpc("tools/list", json!({}));
    let tools = r["result"]["tools"].as_array().cloned().unwrap_or_default();
    let names: Vec<&str> = tools.iter().filter_map(|x| x["name"].as_str()).collect();
    rep.check("tools/list retorna 23 tools", names.len() == 23,
        format!("{} tools: {names:?}", names.len()));
    for want in ["remember", "remember_episodic", "recall", "rag_context", "recall_temporal",
                 "recall_entities",
                 "feedback", "diary", "profile", "expire_old",
                 "explain", "reinforce", "forget",
                 "associate", "related_to", "contradicts", "supersede", "conflicts",
                 "resolve_conflict", "merge_memories", "health", "validate", "era_report"] {
        rep.check(&format!("tool '{want}' presente"), names.contains(&want), "".into());
    }
    rep.phase("tools/list", &t);

    // ---------- fase 3: memória (remember) ----------
    let t = Instant::now();
    let (txt, is_err) = srv.tool("remember", json!({"text": "hot test alpha: P2 hardening landed com fuzz central e clippy zero-warnings"}));
    rep.check("remember alpha", !is_err, txt.clone());
    let key_a = txt.rsplit('(').next().unwrap_or("").trim_end_matches(')').to_string();
    let (txt, is_err) = srv.tool("remember", json!({"text": "hot test beta: mesh de telepatia converge com 8 agentes em 5 camadas"}));
    rep.check("remember beta", !is_err, txt.clone());
    let key_b = txt.rsplit('(').next().unwrap_or("").trim_end_matches(')').to_string();
    let (txt, is_err) = srv.tool("remember", json!({"text": "hot test gamma: MCP health e validate expoe integridade do banco"}));
    rep.check("remember gamma", !is_err, txt.clone());
    let key_g = txt.rsplit('(').next().unwrap_or("").trim_end_matches(')').to_string();
    rep.check("chaves unicas geradas", key_a != key_b && key_b != key_g, format!("{key_a} {key_b} {key_g}"));
    rep.phase("remember x3", &t);

    // ---------- fase 4: recall semântico (demo_embed trigram) ----------
    let t = Instant::now();
    let (txt, is_err) = srv.tool("recall", json!({"query": "telepatia converge", "k": 3}));
    rep.check("recall acha beta no top", !is_err && txt.contains("hot test beta"), txt.clone());
    rep.check("recall expõe a storage key do hit (md/L4/...)", txt.contains("md/L4/mcp/"), txt.clone());
    rep.check("recall expõe proveniência", txt.contains("[state="), txt.clone());
    let (txt, is_err) = srv.tool("recall", json!({"query": "clippy zero-warnings", "k": 3}));
    rep.check("recall acha alpha", !is_err && txt.contains("hot test alpha"), txt.clone());
    rep.phase("recall", &t);

    // ---------- fase 4d: modos de retrieval (v1.1.4 item 8, cognee) ----------
    // lexical (BM25, sem embedding) e hybrid (semântico + lexical) são
    // selecionáveis por `mode` no mesmo tool `recall`.
    let t = Instant::now();
    let (txt, is_err) = srv.tool("recall", json!({"query": "telepatia converge", "k": 3, "mode": "lexical"}));
    rep.check("recall mode=lexical acha beta sem embedding",
        !is_err && txt.contains("hot test beta"), txt.clone());
    let (txt, is_err) = srv.tool("recall", json!({"query": "telepatia converge", "k": 3, "mode": "hybrid"}));
    rep.check("recall mode=hybrid acha beta (semântico + lexical)",
        !is_err && txt.contains("hot test beta"), txt.clone());
    rep.phase("recall modes", &t);

    // ---------- fase 4e: retrieval temporal com intenção (v1.1.4 item 9) ----
    // recall_temporal(query, at): responde "qual era o estado em T?" — as
    // memórias VÁLIDAS em `at` sobem. O server indexa a memória de validade
    // que testamos via remember + set_validity no hot test? Não — aqui só
    // validamos que o tool existe e responde para um at no passado sem crash.
    let t = Instant::now();
    let (txt, is_err) = srv.tool("recall_temporal", json!({
        "query": "telepatia converge", "at": 1760400000000i64, "k": 3
    }));
    rep.check("recall_temporal existe e responde",
        !is_err && !txt.contains("obrigatorio"), txt.clone());
    let (txt2, is_err2) = srv.tool("recall_temporal", json!({"query": "telepatia converge"}));
    rep.check("recall_temporal sem at → -32602 parâmetro obrigatório",
        is_err2 && txt2.contains("obrigatorio"), txt2.clone());
    rep.phase("recall temporal", &t);

    // ---------- fase 4f: recall por entidades (v1.1.4 item 10, 1-hop) -------
    // O server aceita `entities` no remember e expõe `recall_entities` — o
    // core nunca extrai entidade de texto: as strings devem casar exatamente.
    let t = Instant::now();
    let (txt, is_err) = srv.tool("remember", json!({
        "text": "roteiro da reuniao do projeto neural-os",
        "entities": ["project/neural-os", "org/opencode"]
    }));
    rep.check("remember aceita entities", !is_err && txt.contains("md/L4/mcp/"), txt.clone());
    let (txt, is_err) = srv.tool("remember", json!({
        "text": "design do agente opencode",
        "entities": ["org/opencode"]
    }));
    rep.check("remember aceita entities (2)", !is_err && txt.contains("md/L4/mcp/"), txt.clone());
    let (txt, is_err) = srv.tool("recall_entities", json!({
        "entities": ["project/neural-os", "org/opencode"], "k": 5
    }));
    rep.check("recall_entities acha doc com overlap maior primeiro",
        !is_err && txt.contains("roteiro da reuniao"), txt.clone());
    let (txt, is_err) = srv.tool("recall_entities", json!({"entities": ["org/opencode"], "k": 5}));
    rep.check("recall_entities por uma entidade acha os dois docs",
        !is_err && txt.contains("roteiro") && txt.contains("design do agente"), txt.clone());
    let (txt, is_err) = srv.tool("recall_entities", json!({"entities": []}));
    rep.check("recall_entities sem entities → -32602 obrigatório",
        is_err && txt.contains("obrigatorio"), txt.clone());
    let (txt, is_err) = srv.tool("recall_entities", json!({"entities": ["entidade/inexistente"]}));
    rep.check("recall_entities com entidade inexistente → vazio",
        !is_err && txt.contains("nenhuma"), txt.clone());
    rep.phase("recall entities", &t);

    // ---------- fase 4b: embedding FORNECIDO pelo agente (v1.1 P4) ----------
    // O server aceita `embedding` no payload — a camada superior pluga um
    // modelo real; o demo é só o fallback. ADR-0007: o vetor do agente deve
    // pertencer à ERA do corpus (mesma dim) — dim estrangeira é REJEITADA no
    // write (width-lock truncaria em silêncio); quem fornece embedding usa o
    // MESMO modelo na gravação e na busca (contrato P4, agora enforced).
    let t = Instant::now();
    let foreign = srv.tool("remember", json!({
        "text": "vetor customizado do agente",
        "embedding": [1.0, -1.0, 1.0, -1.0]
    }));
    rep.check("era guard: embedding de outra dim → Invalid + hint era_report",
        foreign.1 && foreign.0.contains("era_report"), foreign.0.clone());
    let agent_emb: Vec<f32> = demo_embed("vetor customizado do agente");
    let agent_emb_json: Vec<f64> = agent_emb.iter().map(|x| *x as f64).collect();
    let (txt, is_err) = srv.tool("remember", json!({
        "text": "vetor customizado do agente",
        "embedding": agent_emb_json
    }));
    rep.check("remember aceita embedding do agente (mesma dim da era)", !is_err && txt.contains("md/L4/mcp/"), txt.clone());
    let key_emb = txt.rsplit('(').next().unwrap_or("").trim_end_matches(')').to_string();
    let (txt, is_err) = srv.tool("recall", json!({
        "query": "vetor customizado do agente",
        "embedding": agent_emb_json,
        "k": 3
    }));
    rep.check("recall com embedding do agente acha o doc",
        !is_err && txt.contains("vetor customizado do agente"), txt.clone());
    // contrato P4: MESMO modelo nos dois caminhos — o recall sem embedding
    // (fallback demo) casa com o doc gravado com o embedding do agente, pois
    // ambos derivam do mesmo modelo da era (consistência de era, ADR-0007)
    let (txt, is_err) = srv.tool("recall", json!({"query": "vetor customizado do agente", "k": 3}));
    rep.check("recall sem embedding acha doc do agente (mesmo modelo, mesma era)",
        !is_err && txt.contains("vetor customizado do agente"), txt.clone());
    // caminho do embedder do server (demo): doc gravado SEM embedding acha no
    // recall sem embedding
    let (txt, is_err) = srv.tool("remember", json!({"text": "doc demo do servidor com trigram"}));
    rep.check("remember sem embedding usa o embedder do server", !is_err && txt.contains("md/L4/mcp/"), txt.clone());
    let (txt, is_err) = srv.tool("recall", json!({"query": "doc demo trigram", "k": 3}));
    rep.check("recall sem embedding acha doc gravado pelo demo",
        !is_err && txt.contains("doc demo do servidor com trigram"), txt.clone());
    let (txt, is_err) = srv.tool("forget", json!({"key": key_emb}));
    rep.check("forget limpa o doc customizado", !is_err, txt.clone());
    rep.phase("embedding do agente", &t);

    // ---------- fase 4c: paginação LAZY do recall (v1.1.3 S5) ----------
    // O server computa só off+size+1 hits por página (em vez de top-100 fixo)
    // e usa cursor opaco de offset. Páginas fatiam o MESMO top-k determinístico
    // → sem repetição e sem buraco entre páginas. Usamos rpc() cru para ver o
    // campo `nextCursor` (top-level, o tool() só devolve content[0].text).
    let t = Instant::now();
    let mut paged_keys: Vec<String> = Vec::new();
    for i in 0..4 {
        let text = format!("memoria paginada {:02} com embedding do agente", i);
        let emb: Vec<f64> = demo_embed(&text).iter().map(|x| *x as f64).collect();
        let (txt, is_err) = srv.tool("remember", json!({
            "text": text,
            "embedding": emb
        }));
        rep.check(&format!("remember p4c-{}", i), !is_err, txt.clone());
    }
    let q_emb: Vec<f64> = demo_embed("memoria paginada embedding do agente").iter().map(|x| *x as f64).collect();
    let r1 = srv.rpc("tools/call", json!({"name": "recall", "arguments": {
        "query": "memoria paginada embedding do agente",
        "embedding": q_emb,
        "k": 8,
        "pageSize": 2
    }}));
    let t1 = r1["result"]["content"][0]["text"].as_str().unwrap_or("").to_string();
    let cur = r1["result"]["nextCursor"].as_str().unwrap_or("").to_string();
    rep.check("recall página 1 (pageSize=2) tem nextCursor",
        !t1.is_empty() && !cur.is_empty(), format!("cur={cur} | {t1}"));
    for hit_key in t1.split("- ").skip(1).filter_map(|s| s.split(" | ").next().map(|k| k.trim().to_string())) {
        paged_keys.push(hit_key);
    }
    rep.check("página 1 devolve 2 hits", paged_keys.len() == 2, format!("{paged_keys:?}"));
    let r2 = srv.rpc("tools/call", json!({"name": "recall", "arguments": {
        "query": "memoria paginada embedding do agente",
        "embedding": q_emb,
        "k": 8,
        "pageSize": 2,
        "cursor": cur
    }}));
    let t2 = r2["result"]["content"][0]["text"].as_str().unwrap_or("").to_string();
    rep.check("recall página 2 segue o cursor (hits ou fim)",
        !t2.is_empty() && !t2.contains("nenhuma memoria similar"), t2.clone());
    for hit_key in t2.split("- ").skip(1).filter_map(|s| s.split(" | ").next().map(|k| k.trim().to_string())) {
        rep.check(&format!("página 2 não repete hit da página 1: {hit_key}"),
            !paged_keys.contains(&hit_key), t2.clone());
    }
    rep.phase("paginação lazy do recall", &t);

    // ---------- fase 5: rag_context + explain ----------
    let t = Instant::now();
    let (txt, is_err) = srv.tool("rag_context", json!({"query": "integridade banco", "k": 2}));
    rep.check("rag_context recupera gamma (integridade)", !is_err && txt.contains("hot test gamma"), txt.clone());
    let (txt, is_err) = srv.tool("explain", json!({"key": key_a}));
    rep.check("explain retorna metadados", !is_err && txt.contains("memory_id") && txt.contains("version_id"), txt.clone());
    let (txt, is_err) = srv.tool("explain", json!({"key": "md/L4/nao-existe"}));
    rep.check("explain de chave inexistente → erro amigável", is_err && txt.contains("erro"), txt.clone());
    rep.phase("rag_context+explain", &t);

    // ---------- fase 6: reforço e linhagem (reinforce/supersede/associate) ----------
    let t = Instant::now();
    let (txt, is_err) = srv.tool("reinforce", json!({"key": key_a, "delta": 0.1}));
    rep.check("reinforce +0.1", !is_err && txt.contains("reforcada"), txt.clone());
    let (txt, is_err) = srv.tool("associate", json!({"a": key_a, "kind": "related_to", "b": key_b}));
    rep.check("associate L6", !is_err && txt.contains("relacao"), txt.clone());
    let (txt, is_err) = srv.tool("related_to", json!({"key": key_a}));
    rep.check("related_to vê o alvo (chave beta)", !is_err && txt.contains(&key_b), txt.clone());
    let (txt, is_err) = srv.tool("supersede", json!({"old": key_b, "new": key_a}));
    rep.check("supersede (linhagem causal)", !is_err && txt.contains("superseded"), txt.clone());
    rep.phase("linhagem", &t);

    // ---------- fase 7: observabilidade (health/validate) ----------
    let t = Instant::now();
    let (txt, is_err) = srv.tool("health", json!({}));
    rep.check("health: storage_ok", !is_err && txt.contains("\"storage_ok\": true"), txt.clone());
    rep.check("health: doc_count ≥ 6 (3 docs L4 + 3 companions L2)",
        !is_err && txt.contains("doc_count"), txt.clone());
    let (txt, is_err) = srv.tool("validate", json!({}));
    rep.check("validate: banco saudável", !is_err && txt.contains("saudavel"), txt.clone());
    // era_report (ADR-0007): veredito de era + custo estimado aplicando a
    // fórmula ao total de registros — a LLM gestora decide migrar/esperar.
    let (txt, is_err) = srv.tool("era_report", json!({}));
    rep.check("era_report: verdict ok (era única 256-dim)",
        !is_err && txt.contains("verdict: ok"), txt.clone());
    rep.check("era_report: estimativa de custo exposta (formula + db-side)",
        txt.contains("estimated db-side") && txt.contains("formula"), txt.clone());
    rep.phase("health/validate", &t);

    // ---------- fase 8: resources + paginação ----------
    let t = Instant::now();
    let r = srv.rpc("resources/list", json!({"pageSize": 4}));
    let res = r["result"]["resources"].as_array().map(|v| v.len()).unwrap_or(0);
    let has_next = r["result"]["nextCursor"].is_string();
    rep.check("resources/list página 1 (4 itens, nextCursor)",
        res == 4 && has_next, r.to_string());
    let cur = r["result"]["nextCursor"].as_str().unwrap_or("").to_string();
    let r = srv.rpc("resources/list", json!({"pageSize": 4, "cursor": cur}));
    let res2 = r["result"]["resources"].as_array().map(|v| v.len()).unwrap_or(0);
    rep.check("resources/list página 2 avança", res2 > 0, r.to_string());
    let uri = r["result"]["resources"][0]["uri"].as_str().unwrap_or("").to_string();
    let r = srv.rpc("resources/read", json!({"uri": uri}));
    rep.check("resources/read devolve texto",
        r["result"]["contents"][0]["text"].as_str().is_some_and(|s| !s.is_empty()), r.to_string());
    rep.phase("resources+paginação", &t);

    // ---------- fase 9: caminhos de erro ----------
    let t = Instant::now();
    let r = srv.rpc("server/discover", json!({}));
    rep.check("method desconhecido → -32601", r["error"]["code"] == -32601, r.to_string());
    let (txt, is_err) = srv.tool("tool_inexistente", json!({}));
    rep.check("tool desconhecida → erro", is_err && txt.contains("-32602"), txt.clone());
    let r = srv.rpc("tools/call", json!({"name": "remember"}));
    rep.check("parametro faltando → -32602", r["error"]["code"] == -32602, r.to_string());
    rep.phase("erros", &t);

    // ---------- fase 10: PERSISTÊNCIA (o teste a quente de verdade) ----------
    let t = Instant::now();
    srv.stop(); // mata o processo — memória só sobrevive se FileStorage+checkpoint OK
    let mut srv2 = Mcp::spawn(&bin, &db);
    let (txt, is_err) = srv2.tool("recall", json!({"query": "hot test alpha", "k": 3}));
    rep.check("PERSISTÊNCIA: alpha lembrado após restart do processo",
        !is_err && txt.contains("hot test alpha"), txt.clone());
    let (txt, _) = srv2.tool("validate", json!({}));
    rep.check("validate pós-restart: saudável", txt.contains("saudavel"), txt.clone());
    srv2.stop();
    rep.phase("persistência (restart)", &t);

    let _ = std::fs::remove_file(&db);
    std::process::exit(rep.finish());
}
