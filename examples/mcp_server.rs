//! MCP server (roadmap item 5) — exposes neural-sgdb to AI agents
//! (Claude Code, Cursor, OpenCode) via the Model Context Protocol over stdio.
//!
//! Run with: `cargo run --release --example mcp_server` and point the MCP
//! client at the binary (e.g. `claude mcp add neural-sgdb -- cargo run
//! --release --example mcp_server`).
//!
//! Protocolo: JSON-RPC 2.0 sobre stdio, uma mensagem por linha (`\n`), stdout
//! SÓ com mensagens MCP (logs → stderr). Handshake legado `2025-11-25`
//! (initialize → initialized → tools/list → tools/call), ver spec em
//! https://modelcontextprotocol.io/specification/2025-11-25/
//!
//! ⚠️ Embedding de demonstração por default: o crate standalone não tem
//! modelo de embedding (o kernel usa BGE); aqui usamos hash de trigramas →
//! 256-dim para `recall` funcionar de ponta a ponta. Plugue um embedder REAL
//! via env `NEURAL_SGDB_EMBEDDER` (trait `neural_sgdb::Embedder`) ou forneça
//! `embedding` no payload de `remember`/`recall` (v1.1 P4).

use std::io::{self, BufRead, Write};

use neural_sgdb::Sgdb;
#[cfg(feature = "file-storage")]
use neural_sgdb::FileStorage;
#[cfg(not(feature = "file-storage"))]
use neural_sgdb::InMemory;
use neural_sgdb::{DemoEmbedder, Embedder};
use serde_json::{json, Value};

/// Contador monotônico para chaves de `remember` (fix #10: mesma chave ms
/// colide — ms*1000 + seq garante unicidade no mesmo milissegundo).
static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Embedder ativo no server: default `DemoEmbedder` (trigram). O trait
/// `neural_sgdb::Embedder` permite plugar um modelo real sem tocar no core.
/// `NEURAL_SGDB_EMBEDDER=demo` → demo; qualquer outro valor atual = demo
/// (registrado em stderr) — a porta de plug-in real é o trait + embeddings
/// no payload.
fn load_embedder() -> Box<dyn Embedder> {
    match std::env::var("NEURAL_SGDB_EMBEDDER").as_deref() {
        Ok("demo") | Ok("") => {
            eprintln!("[neural-sgdb] embedder: demo (trigram hash)");
            Box::new(DemoEmbedder)
        }
        Ok(other) => {
            eprintln!(
                "[neural-sgdb] embedder '{other}' desconhecido — usando demo; \
                 plugue um modelo real via trait Embedder"
            );
            Box::new(DemoEmbedder)
        }
        Err(_) => {
            eprintln!("[neural-sgdb] embedder: demo (trigram hash)");
            Box::new(DemoEmbedder)
        }
    }
}

/// Embedding para um texto: usa o fornecido pelo agente (payload) se
/// presente/validável, senão o embedder ativo do server.
fn embed_for(emb: &dyn Embedder, text: &str, payload: &Value) -> Result<Vec<f32>, String> {
    if let Some(arr) = payload["embedding"].as_array() {
        if !arr.is_empty() && arr.len() <= neural_sgdb::MAX_EMBEDDING_DIM {
            let v: Vec<f32> = arr
                .iter()
                .filter_map(|x| x.as_f64().map(|f| f as f32))
                .collect();
            if v.len() == arr.len() {
                return Ok(v);
            }
            return Err("parametro 'embedding' deve conter apenas numeros".into());
        }
        return Err(format!(
            "parametro 'embedding' deve ter 1..={} dimensoes",
            neural_sgdb::MAX_EMBEDDING_DIM
        ));
    }
    emb.embed(text).map_err(|e| format!("embedding falhou: {e}"))
}

/// #8 — parse do URI de resource `memory://{layer}/{key}` (ex: memory://L2/ts/0000).
fn parse_resource_uri(uri: &str) -> Option<(neural_sgdb::MemoryLayer, String)> {
    let rest = uri.strip_prefix("memory://")?;
    let (layer, key) = rest.split_once('/')?;
    let layer = match layer {
        "L0" => neural_sgdb::MemoryLayer::L0Sensory,
        "L1" => neural_sgdb::MemoryLayer::L1Working,
        "L2" => neural_sgdb::MemoryLayer::L2EpisodicShort,
        "L3" => neural_sgdb::MemoryLayer::L3EpisodicLong,
        "L4" => neural_sgdb::MemoryLayer::L4Semantic,
        "L5" => neural_sgdb::MemoryLayer::L5Procedural,
        "L6" => neural_sgdb::MemoryLayer::L6Reserved,
        "L7" => neural_sgdb::MemoryLayer::L7Identity,
        _ => return None,
    };
    Some((layer, String::from(key)))
}

/// #8 — paginação com cursor opaco (offset). Retorna (página, nextCursor).
/// `size` vem do JSON-RPC (entrada externa hostil): clampar impede DoS por
/// alocação gigante; `saturating_add` impede overflow de `off + size`.
fn paginate<T: Clone>(items: &[T], cursor: Option<&str>, size: usize) -> (Vec<T>, Option<String>) {
    const MAX_PAGE_SIZE: usize = 1000;
    let size = size.min(MAX_PAGE_SIZE);
    let off = cursor.and_then(|c| c.parse::<usize>().ok()).unwrap_or(0).min(items.len());
    let end = off.saturating_add(size).min(items.len());
    let page = items[off..end].to_vec();
    let next = if end < items.len() { Some((end as u32).to_string()) } else { None };
    (page, next)
}

fn send(msg: &Value) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{}", serde_json::to_string(msg).unwrap());
    let _ = out.flush();
}

fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn main() {
    let db_path = std::env::var("NEURAL_SGDB_DB").unwrap_or_else(|_| "sgdb_memory.db".into());

    // Backend concreto por feature: `FileStorage` (persistente) ou `InMemory`
    // (demo volátil) — `Sgdb::open(impl Storage)` aceita ambos sem boxing.
    #[cfg(feature = "file-storage")]
    let storage = match FileStorage::open(&db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[neural-sgdb] erro ao abrir {db_path}: {e}");
            std::process::exit(1);
        }
    };

    #[cfg(not(feature = "file-storage"))]
    let storage = {
        eprintln!("[neural-sgdb] file-storage desativada — usando InMemory (volátil)");
        InMemory::new()
    };

    let mut db = match Sgdb::open(storage) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[neural-sgdb] erro ao iniciar Sgdb: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("[neural-sgdb] MCP server pronto — db={db_path} backend={}", db.backend());
    let embedder = load_embedder();

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            continue; // linhas vazias toleradas
        }
        let msg: Value = match serde_json::from_str(line) {
            Ok(m) => m,
            Err(_) => {
                send(&json!({"jsonrpc":"2.0","id":null,
                    "error":{"code":-32700,"message":"Parse error"}}));
                continue;
            }
        };
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");

        match method {
            "initialize" => {
                // Version negotiation: respondemos o legado estável 2025-11-25
                // (clients 2026 modernos fazem fallback ao ver -32601 em
                // server/discover antes do initialize).
                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                    "protocolVersion":"2025-11-25",
                    "capabilities":{"tools":{}},
                    "serverInfo":{"name":"neural-sgdb","version":"1.1.0"}
                }}));
            }
            "notifications/initialized" | "notifications/cancelled" | "notifications/progress" => {
                // fire-and-forget — sem resposta
            }
            "ping" => {
                send(&json!({"jsonrpc":"2.0","id":id,"result":{}}));
            }
            "tools/list" => {
                send(&json!({"jsonrpc":"2.0","id":id,"result":{"tools":[
                    {"name":"remember",
                     "description":"Armazena uma memoria de texto no banco neural-sgdb. Opcional: forneca `embedding` (array de f32, 1..=256 dims) para usar um modelo real; sem ele, o server usa o embedder configurado (demo trigram).",
                     "inputSchema":{"type":"object",
                       "properties":{
                         "text":{"type":"string","description":"Conteudo a lembrar"},
                         "embedding":{"type":"array","items":{"type":"number"},"description":"Embedding fornecido pelo agente (opcional)"}},
                       "required":["text"]},
                     "annotations":{"destructiveHint":true,"idempotentHint":true}},
                    {"name":"remember_episodic",
                     "description":"Camada episodica VERBATIM (mempalace): guarda o par user/response cru em L2 timestamped, sem extracao nem resumo. Util quando a extracao perderia contexto. Devolve as storage keys (md/L2/<ts>/u e /a).",
                     "inputSchema":{"type":"object",
                       "properties":{
                         "user":{"type":"string","description":"Texto do usuario (verbatim)"},
                         "response":{"type":"string","description":"Texto do assistente (verbatim)"},
                         "now":{"type":"integer","description":"Timestamp (ms). Omitir = relogio local."}},
                       "required":["user","response"]},
                     "annotations":{"idempotentHint":true}},
                    {"name":"recall",
                     "description":"Busca semantica sobre memorias armazenadas. Retorna as top-k mais similares. Opcional: forneca `embedding` (consistente com o usado no remember) para busca com modelo real.",
                     "inputSchema":{"type":"object",
                       "properties":{
                         "query":{"type":"string","description":"Texto de busca"},
                         "embedding":{"type":"array","items":{"type":"number"},"description":"Embedding fornecido pelo agente (opcional)"},
                         "k":{"type":"integer","minimum":1,"maximum":20,"default":5},
                         "cursor":{"type":"string","description":"Cursor de paginacao (opaco, de um resultado anterior)"},
                         "pageSize":{"type":"integer","minimum":1,"maximum":20,"default":5}},
                       "required":["query"]},
                     "annotations":{"readOnlyHint":true}},
                    {"name":"rag_context",
                     "description":"Busca memorias e monta contexto formatado pronto para prompt RAG. Opcional: `embedding` fornecido pelo agente.",
                     "inputSchema":{"type":"object",
                       "properties":{
                         "query":{"type":"string","description":"Texto de busca"},
                         "embedding":{"type":"array","items":{"type":"number"},"description":"Embedding fornecido pelo agente (opcional)"},
                         "k":{"type":"integer","minimum":1,"maximum":10,"default":3}},
                       "required":["query"]},
                     "annotations":{"readOnlyHint":true}},
                    {"name":"explain",
                     "description":"Explica ESTRUTURADAMENTE por que uma memoria esta no estado atual (proveniencia, importância, linhagem, validade).",
                     "inputSchema":{"type":"object",
                       "properties":{"key":{"type":"string","description":"Storage key (md/L4/k ou L4/k)"}},
                       "required":["key"]},
                     "annotations":{"readOnlyHint":true}},
                    {"name":"reinforce",
                     "description":"Reforca uma memoria: importância += delta (clampada a [0,1]) e registra last_reinforced.",
                     "inputSchema":{"type":"object",
                       "properties":{
                         "key":{"type":"string"},
                         "delta":{"type":"number","description":"Aumento de importância (ex: 0.1)"}},
                       "required":["key","delta"]}},
                    {"name":"feedback",
                     "description":"Feedback de uso (cognee improve): re-pondera a memoria pelo resultado real — positive sobe importancia E confianca, negative desce ambas. amount (default 0.1) e a intensidade.",
                     "inputSchema":{"type":"object",
                       "properties":{
                         "key":{"type":"string"},
                         "positive":{"type":"boolean","description":"true = util (sobe), false = errado/inutil (desce)"},
                         "amount":{"type":"number","default":0.1,"description":"Intensidade (default 0.1)"}},
                       "required":["key","positive"]}},
                    {"name":"forget",
                     "description":"Esquece (ARCHIVA) uma memoria — historia preservada, recall default passa a ignora-la.",
                     "inputSchema":{"type":"object",
                       "properties":{"key":{"type":"string"}},
                       "required":["key"]},
                     "annotations":{"destructiveHint":true,"idempotentHint":true}},
                    {"name":"associate",
                     "description":"Afirma uma relacao L6: a --kind--> b (related_to|causes|supports|contradicts|derived_from|supersedes).",
                     "inputSchema":{"type":"object",
                       "properties":{
                         "a":{"type":"string"},
                         "kind":{"type":"string","enum":["related_to","causes","supports","contradicts","derived_from","supersedes"]},
                         "b":{"type":"string"}},
                       "required":["a","kind","b"]}},
                    {"name":"related_to",
                     "description":"Lista alvos de relacoes partindo de uma memoria.",
                     "inputSchema":{"type":"object",
                       "properties":{"key":{"type":"string"}},
                       "required":["key"]},
                     "annotations":{"readOnlyHint":true}},
                    {"name":"contradicts",
                     "description":"Lista memorias que contradizem a informada.",
                     "inputSchema":{"type":"object",
                       "properties":{"key":{"type":"string"}},
                       "required":["key"]},
                     "annotations":{"readOnlyHint":true}},
                    {"name":"supersede",
                     "description":"Marca old como superseded e liga new como sucessor (linhagem causal).",
                     "inputSchema":{"type":"object",
                       "properties":{"old":{"type":"string"},"new":{"type":"string"}},
                       "required":["old","new"]}},
                    {"name":"conflicts",
                     "description":"Lista conflitos persistidos (Open/Resolved) com evidencias preservadas.",
                     "inputSchema":{"type":"object","properties":{}},
                     "annotations":{"readOnlyHint":true}},
                    {"name":"resolve_conflict",
                     "description":"Resolve um conflito escolhendo o vencedor por version_id — o perdedor permanece na historia.",
                     "inputSchema":{"type":"object",
                       "properties":{
                         "conflict_id":{"type":"string"},
                         "winner_version_id":{"type":"string"}},
                       "required":["conflict_id","winner_version_id"]}},
                    {"name":"merge_memories",
                     "description":"Funde duas memorias em C: parent_ids=[A,B], payload concatenado, fontes intactas.",
                     "inputSchema":{"type":"object",
                       "properties":{
                         "a":{"type":"string"},
                         "b":{"type":"string"},
                         "target":{"type":"string","description":"Chave nova (vazia = gerada)"}},
                       "required":["a","b"]}},
                    {"name":"health",
                     "description":"Estado observavel do banco: backend, node_id, sonda de storage, contagens (docs/BQ/RAM) e conflitos abertos.",
                     "inputSchema":{"type":"object","properties":{}},
                     "annotations":{"readOnlyHint":true}},
                    {"name":"diary",
                     "description":"Diario por agente (mempalace): memorias L2 episodicas cujo source == node_id, mais recentes primeiro (keys ts sortable revertidas). Devolve (storage_key, payload).",
                     "inputSchema":{"type":"object",
                       "properties":{
                         "node_id":{"type":"integer","description":"ID do agente (source). Omitir = agente local (health.node_id)."},
                         "limit":{"type":"integer","minimum":1,"maximum":100,"default":10}},
                       "required":[]},
                     "annotations":{"readOnlyHint":true}},
                    {"name":"validate",
                     "description":"Integridade: varre storage md/, decodifica NMD1, cruza ART/BQ e detecta side-tables orfas. Vazio = saudavel; cada issue = key + descricao.",
                     "inputSchema":{"type":"object","properties":{}},
                     "annotations":{"readOnlyHint":true}}
                ]}}));
            }
            "resources/list" => {
                // #8: expõe as memórias como resources `memory://{layer}/{key}`
                // com paginação por cursor opaco (offset).
                let cursor = msg["params"]["cursor"].as_str();
                let size = msg["params"]["pageSize"].as_u64().unwrap_or(20).max(1) as usize;
                let mut all: Vec<Value> = Vec::new();
                for layer in ["L1", "L2", "L3", "L4", "L5", "L7"] {
                    if let Ok(items) = db.scan_prefix(&format!("md/{layer}/")) {
                        for (k, _) in items {
                            let key = k.trim_start_matches(&format!("md/{layer}/"));
                            all.push(json!({"uri": format!("memory://{layer}/{key}"),
                                            "name": key, "mimeType":"text/plain"}));
                        }
                    }
                }
                let (page, next) = paginate(&all, cursor, size);
                let mut result = json!({"resources": page});
                if let Some(n) = next {
                    result["nextCursor"] = json!(n);
                }
                send(&json!({"jsonrpc":"2.0","id":id,"result":result}));
            }
            "resources/read" => {
                let uri = msg["params"]["uri"].as_str().unwrap_or("");
                match parse_resource_uri(uri) {
                    Some((layer, key)) => match db.get(layer, &key) {
                        Ok(Some(doc)) => {
                            let text = String::from_utf8_lossy(&doc.payload).into_owned();
                            send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "contents":[{"uri":uri,"mimeType":"text/plain","text":text}]}}));
                        }
                        Ok(None) => send(&error_response(&id, -32002, "recurso nao encontrado")),
                        Err(e) => send(&error_response(&id, -32603, &format!("erro interno: {e}"))),
                    },
                    None => send(&error_response(&id, -32602, "URI de recurso invalido")),
                }
            }
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or("");
                let args = &msg["params"]["arguments"];
                match name {
                    "remember" => {
                        let text = args["text"].as_str().unwrap_or("");
                        if text.is_empty() {
                            send(&error_response(&id, -32602, "parametro 'text' obrigatorio"));
                            continue;
                        }
                        // unique key: ms + monotonic counter (2 remembers in
                        // the same ms do not collide — bughunt #10)
                        let key = format!("mcp/{:06}", {
                            let ms = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis())
                                .unwrap_or(0);
                            let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            ms * 1000 + seq as u128
                        });
                        let emb = match embed_for(embedder.as_ref(), text, args) {
                            Ok(e) => e,
                            Err(e) => {
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":format!("erro: {e}")}],"isError":true}}));
                                continue;
                            }
                        };
                        match db.remember_semantic(&key, text, &emb) {
                            Ok(()) => {
                                // devolve a STORAGE KEY completa (`md/L4/...`) —
                                // a chave crua `mcp/...` NÃO resolve em
                                // explain/reinforce (achado hot-test 2026-08-13)
                                let sk = format!("md/L4/{key}");
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":format!("memoria armazenada ({sk})")}],
                                    "isError":false}}))
                            }
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("erro: {e}")}],"isError":true}})),
                        }
                    }
                    "remember_episodic" => {
                        let user = args["user"].as_str().unwrap_or("");
                        let response = args["response"].as_str().unwrap_or("");
                        if user.is_empty() || response.is_empty() {
                            send(&error_response(&id, -32602, "parametros 'user' e 'response' obrigatorios"));
                            continue;
                        }
                        let now = args["now"].as_u64().unwrap_or_else(|| {
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0)
                        });
                        match db.remember_episodic(user, response, now) {
                            Ok((ku, ka)) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("episodio verbatim armazenado:\nuser: {ku}\nasst: {ka}")}],
                                "isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("erro: {e}")}],"isError":true}})),
                        }
                    }
                    "recall" => {
                        let query = args["query"].as_str().unwrap_or("");
                        let k = args["k"].as_u64().unwrap_or(5) as usize;
                        if query.is_empty() {
                            send(&error_response(&id, -32602, "parametro 'query' obrigatorio"));
                            continue;
                        }
                        let emb = match embed_for(embedder.as_ref(), query, args) {
                            Ok(e) => e,
                            Err(e) => {
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":format!("erro: {e}")}],"isError":true}}));
                                continue;
                            }
                        };
                        // v1.1.3 S5 — paginação LAZY: computa só o que a página
                        // pede. Antes buscava top-100 fixo e paginava sobre ele
                        // (custo fixo + teto artificial de 100 hits). Top-k é
                        // determinístico (score, key) — top-(off+size) da busca
                        // completa = os mesmos itens de top-100, então a página
                        // fatia o prefixo correto sem custo extra.
                        let size = args["pageSize"].as_u64().unwrap_or(k as u64).max(1) as usize;
                        let off = args["cursor"]
                            .as_str()
                            .and_then(|c| c.parse::<usize>().ok())
                            .unwrap_or(0);
                        // +1 = hit "sentinela" além da página: sem ele, uma
                        // página exatamente preenchida pareceria a última
                        // (paginate usa items.len() como "conjunto inteiro").
                        let need = off.saturating_add(size).saturating_add(1);
                        let all = db.recall(&emb, need).unwrap_or_default();
                        let (page, next) = paginate(&all, args["cursor"].as_str(), size);
                        let text = if page.is_empty() {
                            "nenhuma memoria similar encontrada".into()
                        } else {
                            // v0.9: hits expõem proveniência (roadmap §13) —
                            // estado/importância/confiança/fonte por hit
                            page.iter().map(|h| {
                                let p = h.provenance.as_ref().map(|p| format!(
                                    " [state={:?} imp={:.2} conf={:.2} src={}]",
                                    p.state, p.importance, p.confidence, p.source)).unwrap_or_default();
                                format!("- {} | {} (d={:.3}){}", h.key, h.text, h.dist, p)
                            }).collect::<Vec<_>>().join("\n")
                        };
                        let mut result = json!({
                            "content":[{"type":"text","text":text}],"isError":false
                        });
                        if let Some(n) = next {
                            result["nextCursor"] = json!(n);
                        }
                        send(&json!({"jsonrpc":"2.0","id":id,"result":result}));
                    }
                    "rag_context" => {
                        let query = args["query"].as_str().unwrap_or("");
                        let k = args["k"].as_u64().unwrap_or(3) as usize;
                        if query.is_empty() {
                            send(&error_response(&id, -32602, "parametro 'query' obrigatorio"));
                            continue;
                        }
                        let emb = match embed_for(embedder.as_ref(), query, args) {
                            Ok(e) => e,
                            Err(e) => {
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":format!("erro: {e}")}],"isError":true}}));
                                continue;
                            }
                        };
                        match db.rag_context(&emb, k) {
                            Ok(ctx) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":if ctx.is_empty() {
                                    "nenhum contexto recuperado".into()} else {ctx}}],
                                "isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("erro: {e}")}],"isError":true}})),
                        }
                    }
                    "explain" => {
                        let key = args["key"].as_str().unwrap_or("");
                        if key.is_empty() {
                            send(&error_response(&id, -32602, "parametro 'key' obrigatorio"));
                            continue;
                        }
                        match db.explain(key) {
                            Ok(ex) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":serde_json::to_string_pretty(&json!({
                                    "key": ex.key, "layer": format!("{:?}", ex.layer),
                                    "state": format!("{:?}", ex.state),
                                    "memory_id": ex.memory_id, "version_id": ex.version_id,
                                    "source": ex.source, "confidence": ex.confidence,
                                    "importance": ex.importance, "created_tick": ex.created_tick,
                                    "last_reinforced": ex.last_reinforced, "parents": ex.parents,
                                    "validity": ex.validity, "children": ex.children})).unwrap_or_default()}],
                                "isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("erro: {e}")}],"isError":true}})),
                        }
                    }
                    "reinforce" => {
                        let key = args["key"].as_str().unwrap_or("");
                        let delta = args["delta"].as_f64().unwrap_or(0.0) as f32;
                        if key.is_empty() {
                            send(&error_response(&id, -32602, "parametro 'key' obrigatorio"));
                            continue;
                        }
                        match db.reinforce(key, delta) {
                            Ok(()) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("reforcada: {key} (+{delta})")}],"isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("erro: {e}")}],"isError":true}})),
                        }
                    }
                    "forget" => {
                        let key = args["key"].as_str().unwrap_or("");
                        if key.is_empty() {
                            send(&error_response(&id, -32602, "parametro 'key' obrigatorio"));
                            continue;
                        }
                        match db.forget(key) {
                            Ok(()) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("arquivada: {key} (historia preservada)")}],"isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("erro: {e}")}],"isError":true}})),
                        }
                    }
                    "feedback" => {
                        let key = args["key"].as_str().unwrap_or("");
                        let positive = args["positive"].as_bool().unwrap_or(true);
                        let amount = args["amount"].as_f64().unwrap_or(0.1) as f32;
                        if key.is_empty() {
                            send(&error_response(&id, -32602, "parametro 'key' obrigatorio"));
                            continue;
                        }
                        match db.feedback(key, positive, amount) {
                            Ok(()) => {
                                let verb = if positive { "util (+)" } else { "errado (-)" };
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":format!("feedback aplicado ({verb} {amount}): {key}")}],"isError":false}}))
                            }
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("erro: {e}")}],"isError":true}})),
                        }
                    }
                    "associate" => {
                        let a = args["a"].as_str().unwrap_or("");
                        let b = args["b"].as_str().unwrap_or("");
                        let kind = match args["kind"].as_str().unwrap_or("") {
                            "related_to" => neural_sgdb::RelationKind::RelatedTo,
                            "causes" => neural_sgdb::RelationKind::Causes,
                            "supports" => neural_sgdb::RelationKind::Supports,
                            "contradicts" => neural_sgdb::RelationKind::Contradicts,
                            "derived_from" => neural_sgdb::RelationKind::DerivedFrom,
                            "supersedes" => neural_sgdb::RelationKind::Supersedes,
                            _ => { send(&error_response(&id, -32602, "kind invalido")); continue; }
                        };
                        match db.associate(a, kind, b) {
                            Ok(()) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("relacao: {a} --{kind:?}--> {b}")}],"isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("erro: {e}")}],"isError":true}})),
                        }
                    }
                    "related_to" => {
                        let key = args["key"].as_str().unwrap_or("");
                        if key.is_empty() {
                            send(&error_response(&id, -32602, "parametro 'key' obrigatorio"));
                            continue;
                        }
                        let rels = db.related_to(key);
                        let text = if rels.is_empty() { "sem relacoes".into() }
                            else { rels.iter().map(|(k, t)| format!("{k:?} -> {t}")).collect::<Vec<_>>().join("\n") };
                        send(&json!({"jsonrpc":"2.0","id":id,"result":{
                            "content":[{"type":"text","text":text}],"isError":false}}));
                    }
                    "contradicts" => {
                        let key = args["key"].as_str().unwrap_or("");
                        if key.is_empty() {
                            send(&error_response(&id, -32602, "parametro 'key' obrigatorio"));
                            continue;
                        }
                        let cs = db.contradicts(key);
                        let text = if cs.is_empty() { "sem contradicoes".into() }
                            else { cs.join("\n") };
                        send(&json!({"jsonrpc":"2.0","id":id,"result":{
                            "content":[{"type":"text","text":text}],"isError":false}}));
                    }
                    "supersede" => {
                        let old = args["old"].as_str().unwrap_or("");
                        let new = args["new"].as_str().unwrap_or("");
                        if old.is_empty() || new.is_empty() {
                            send(&error_response(&id, -32602, "parametros 'old' e 'new' obrigatorios"));
                            continue;
                        }
                        match db.supersede(old, new) {
                            Ok(()) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("{old} superseded por {new}")}],"isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("erro: {e}")}],"isError":true}})),
                        }
                    }
                    "conflicts" => {
                        let cs = db.conflicts();
                        let text = if cs.is_empty() { "nenhum conflito persistido".into() }
                            else { cs.iter().map(|c| format!(
                                "{} [{:?}] {} :: candidatos={} nodos={:?} records={}",
                                c.conflict_id, c.status, c.subject,
                                c.candidates.join(","), c.nodes, c.records.len()))
                                .collect::<Vec<_>>().join("\n") };
                        send(&json!({"jsonrpc":"2.0","id":id,"result":{
                            "content":[{"type":"text","text":text}],"isError":false}}));
                    }
                    "resolve_conflict" => {
                        let cid = args["conflict_id"].as_str().unwrap_or("");
                        let winner = args["winner_version_id"].as_str().unwrap_or("");
                        if cid.is_empty() || winner.is_empty() {
                            send(&error_response(&id, -32602, "parametros 'conflict_id' e 'winner_version_id' obrigatorios"));
                            continue;
                        }
                        match db.resolve_conflict(cid, winner) {
                            Ok(()) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("conflito {cid} resolvido -> {winner}")}],"isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("erro: {e}")}],"isError":true}})),
                        }
                    }
                    "merge_memories" => {
                        let a = args["a"].as_str().unwrap_or("");
                        let b = args["b"].as_str().unwrap_or("");
                        let target = args["target"].as_str().unwrap_or("");
                        if a.is_empty() || b.is_empty() {
                            send(&error_response(&id, -32602, "parametros 'a' e 'b' obrigatorios"));
                            continue;
                        }
                        match db.merge_memories(a, b, target) {
                            Ok(sk) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("fundidas em {sk}")}],"isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("erro: {e}")}],"isError":true}})),
                        }
                    }
                    "health" => {
                        let h = db.health();
                        send(&json!({"jsonrpc":"2.0","id":id,"result":{
                            "content":[{"type":"text","text":serde_json::to_string_pretty(&json!({
                                "backend": h.backend, "node_id": h.node_id,
                                "storage_ok": h.storage_ok, "doc_count": h.doc_count,
                                "bq_len": h.bq_len, "ram_len": h.ram_len,
                                "open_conflicts": h.open_conflicts})).unwrap_or_default()}],
                            "isError":false}}));
                    }
                    "diary" => {
                        let node = args["node_id"].as_u64().map(|n| n as u8).unwrap_or(db.node_id());
                        let limit = args["limit"].as_u64().unwrap_or(10) as usize;
                        match db.diary(node, limit) {
                            Ok(entries) => {
                                let text = if entries.is_empty() {
                                    format!("sem episodios L2 do agente {node}")
                                } else {
                                    entries.iter().map(|(k, p)| format!("{} | {}", k, p))
                                        .collect::<Vec<_>>().join("\n")
                                };
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":text}],"isError":false}}));
                            }
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("erro: {e}")}],"isError":true}})),
                        }
                    }
                    "validate" => {
                        let issues = db.validate();
                        let text = if issues.is_empty() {
                            "banco saudavel (nenhum issue de integridade)".into()
                        } else {
                            issues.iter().map(|i| format!("[{}] {}", i.key, i.message))
                                .collect::<Vec<_>>().join("\n")
                        };
                        send(&json!({"jsonrpc":"2.0","id":id,"result":{
                            "content":[{"type":"text","text":text}],"isError":false}}));
                    }
                    _ => send(&error_response(&id, -32602, "Unknown tool")),
                }
            }
            "" => {
                // notificação sem method válido / malformada
                send(&error_response(&id, -32600, "Invalid request"));
            }
            _ => {
                // -32601 em server/discover → client moderno faz fallback p/ initialize
                send(&error_response(&id, -32601, "Method not found"));
            }
        }
    }
    // EOF no stdin = shutdown
    eprintln!("[neural-sgdb] stdin fechado — encerrando");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_resource_uri_ok_and_bad() {
        let (l, k) = parse_resource_uri("memory://L4/doc%2Fum").unwrap();
        assert_eq!(l, neural_sgdb::MemoryLayer::L4Semantic);
        assert_eq!(k, "doc%2Fum");
        assert!(parse_resource_uri("memory://").is_none());
        assert!(parse_resource_uri("http://L4/x").is_none());
        assert!(parse_resource_uri("memory://L9/x").is_none());
    }

    #[test]
    fn paginate_pages_and_cursor() {
        let items: Vec<u32> = (0..10).collect();
        let (p1, c1) = paginate(&items, None, 4);
        assert_eq!(p1, vec![0, 1, 2, 3]);
        assert_eq!(c1.as_deref(), Some("4"));
        let (p2, c2) = paginate(&items, c1.as_deref(), 4);
        assert_eq!(p2, vec![4, 5, 6, 7]);
        assert_eq!(c2.as_deref(), Some("8"));
        let (p3, c3) = paginate(&items, c2.as_deref(), 4);
        assert_eq!(p3, vec![8, 9]);
        assert_eq!(c3, None, "última página não tem nextCursor");
        // cursor inválido → volta ao início
        let (p, _) = paginate(&items, Some("xyz"), 2);
        assert_eq!(p, vec![0, 1]);
    }

    #[test]
    fn paginate_hostile_size_does_not_panic() {
        let items: Vec<u32> = (0..10).collect();
        // pageSize hostil (u64::MAX → usize::MAX) com cursor ≥ 1 não pode
        // estourar `off + size` nem alocar além dos itens (regressão P0-8).
        let (p, next) = paginate(&items, Some("1"), usize::MAX);
        assert_eq!(p.len(), items.len() - 1, "clamp: página limitada aos itens");
        assert_eq!(next, None, "cursor 1 + tudo → não há next");
        // cursor no fim + size hostil
        let (p, next) = paginate(&items, Some("9"), usize::MAX);
        assert_eq!(p, vec![9]);
        assert_eq!(next, None);
        // tamanho acima do clamp não muda semântica legítima (page 0)
        let (p, next) = paginate(&items, None, 2000);
        assert_eq!(p, items);
        assert_eq!(next, None);
    }

    #[test]
    fn lazy_recall_pages_match_full_topk() {
        // v1.1.3 S5: a paginação lazy busca `off+size` hits em vez de top-100
        // fixo. O contrato é que a página do prefixo lazy == a página do
        // top-k completo (recall é determinístico por (score, key) — top-(n+1)
        // é um prefixo de top-N). Pina o invariante contra regressão futura.
        let mut db = neural_sgdb::Sgdb::open(neural_sgdb::InMemory::new()).unwrap();
        let mut texts = Vec::new();
        for i in 0..12 {
            let t = format!("memoria de teste numero {:02} {}", i, "overlap comum");
            texts.push(t.clone());
            db.remember_semantic(&format!("k{:02}", i), &t, &[1.0, -1.0, 1.0, -1.0]).unwrap();
        }
        let q = [1.0, -1.0, 1.0, -1.0];
        // "top-k completo" = o teto antigo (100); lazy = off+size+1 por página
        // (a sentinela +1 só sonda a próxima página — não muda o conteúdo).
        let full = db.recall(&q, 100).unwrap();
        assert_eq!(full.len(), 12);
        let mut cursor: Option<String> = None;
        let mut collected = Vec::new();
        for _ in 0..5 {
            let off = cursor.as_deref().and_then(|c| c.parse::<usize>().ok()).unwrap_or(0);
            let size = 3usize;
            let need = off.saturating_add(size).saturating_add(1);
            let all = db.recall(&q, need).unwrap();
            let (page, next) = paginate(&all, cursor.as_deref(), size);
            assert!(!page.is_empty(), "página vazia antes do fim do conjunto");
            // cada página lazy == fatia do top-k completo (determinismo)
            for (i, h) in page.iter().enumerate() {
                assert_eq!(h.key, full[off + i].key, "página lazy divergiu do top-k");
            }
            collected.extend(page.into_iter().map(|h| h.key));
            cursor = next;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(cursor, None, "deve ter iterado o conjunto inteiro");
        assert_eq!(collected.len(), 12);
        // sem duplicatas entre páginas
        let uniq: std::collections::HashSet<_> = collected.iter().collect();
        assert_eq!(uniq.len(), 12, "paginação repetiu hits");
    }
}
