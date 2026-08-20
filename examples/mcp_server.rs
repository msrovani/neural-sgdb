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

use neural_sgdb::{
    ContentType, DemoEmbedder, Embedder, RecallPath, Sgdb, DOCTRINE, DOCTRINE_SCOPE,
};
#[cfg(feature = "file-storage")]
use neural_sgdb::FileStorage;
#[cfg(not(feature = "file-storage"))]
use neural_sgdb::InMemory;
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

/// Projeção PROSA de um hit (v1.1.6) — o consumidor é outra inteligência
/// (máquina), então o sufixo é parseável e TIPADO, não só prosa, no formato
/// `- {key} | {text} (d=..) [state=.. imp=.. conf=.. src=.. path=.. type=..
/// terms=.. rel=.. valid=..]`.
/// Invariantes preservadas do formato anterior (hot test): prefixo `- {key} | `
/// (a paginação fatia `split(" | ").next()`) e sufixo que abre em ` [state=`
/// (assert `txt.contains("[state=")`).
/// Datum não-prosa (Embedding/Binary): `text` vazio no core — o consumidor
/// vê `type=Embedding(256)` e sabe que o datum é o payload binário do doc,
/// nunca prosa lossy.
fn fmt_hit(h: &neural_sgdb::Hit) -> String {
    let mut tags = Vec::new();
    if let Some(p) = h.provenance.as_ref() {
        tags.push(format!(
            "state={:?} imp={:.2} conf={:.2} src={}",
            p.state, p.importance, p.confidence, p.source
        ));
        if !p.scope.is_empty() {
            tags.push(format!("scope={}", p.scope));
        }
        if !p.entities.is_empty() {
            tags.push(format!(
                "ents={}",
                p.entities.iter().take(6).cloned().collect::<Vec<_>>().join(",")
            ));
        }
    } else {
        tags.push("state=none".into());
    }
    tags.push(format!("path={:?}", h.path));
    tags.push(format!("type={:?}", h.content_type));
    if h.payload_type != h.content_type {
        // datum real do primário (Embedding(dim) p/ L4/L5) vs projeção
        tags.push(format!("payload={:?}", h.payload_type));
    }
    if !h.matched_terms.is_empty() {
        tags.push(format!(
            "terms={}",
            h.matched_terms.iter().take(8).cloned().collect::<Vec<_>>().join(",")
        ));
    }
    if let Some(rel) = h.rel.as_ref() {
        tags.push(format!("rel={rel}"));
    }
    if let Some((f, u)) = h.validity {
        tags.push(format!("valid=[{f},{u})"));
    }
    format!("- {} | {} (d={:.3}) [{}]", h.key, h.text, h.dist, tags.join(" "))
}

/// Strings ESTÁVEIS (machine-parseable) para o `format=json` — o consumidor
/// casa por valor, não por `Debug` (que pode mudar entre versões).
fn path_str(p: RecallPath) -> &'static str {
    match p {
        RecallPath::Semantic => "semantic",
        RecallPath::Lexical => "lexical",
        RecallPath::Entities => "entities",
    }
}

fn content_type_json(ct: ContentType) -> Value {
    match ct {
        ContentType::Text => json!({"type": "text"}),
        ContentType::Json => json!({"type": "json"}),
        ContentType::Code => json!({"type": "code"}),
        ContentType::Embedding(d) => json!({"type": "embedding", "dim": d}),
        ContentType::Binary => json!({"type": "binary"}),
    }
}

/// Hit estruturado (v1.1.6+) — o retorno primário para consumo máquina→
/// máquina: o consumidor parseia JSON e vê o datum (`type`), o caminho
/// (`path`), o grounding (`matched_terms`) e a proveniência, sem depender
/// da projeção prosa.
fn hit_json(h: &neural_sgdb::Hit) -> Value {
    let mut obj = json!({
        "key": h.key,
        "text": h.text,
        "dist": h.dist,
        "score": h.score,
        "path": path_str(h.path),
        "matched_terms": h.matched_terms,
        "validity": h.validity.map(|(f, u)| json!([f, u])),
        "rel": h.rel,
    });
    let ct = content_type_json(h.content_type);
    obj["type"] = ct["type"].clone();
    obj["dim"] = ct.get("dim").cloned().unwrap_or(Value::Null);
    // item 3 — datum real do primário (Embedding(dim)) vs projeção (type)
    let pct = content_type_json(h.payload_type);
    obj["payload_type"] = pct["type"].clone();
    obj["payload_dim"] = pct.get("dim").cloned().unwrap_or(Value::Null);
    obj["provenance"] = match h.provenance.as_ref() {
        Some(p) => json!({
            "memory_id": p.memory_id,
            "version_id": p.version_id,
            "layer": format!("{:?}", p.layer),
            "state": format!("{:?}", p.state),
            "source": p.source,
            "confidence": p.confidence,
            "importance": p.importance,
            "created_tick": p.created_tick,
            "parent_ids": p.parent_ids,
            "last_reinforced": p.last_reinforced,
            "scope": p.scope,
            "entities": p.entities,
        }),
        None => Value::Null,
    };
    obj
}

/// Serializa hits como array JSON (param `format=json`).
fn hits_json(hits: &[neural_sgdb::Hit]) -> String {
    let arr: Vec<Value> = hits.iter().map(hit_json).collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".into())
}

fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Número de tools em `tools/list` (aliases antigos ainda funcionam em tools/call).
const EXPECTED_MCP_TOOL_COUNT: usize = 4;
const MCP_CONTRACT_VERSION: &str = "1.1.8";
const BUILD_GIT: &str = env!("NEURAL_SGDB_BUILD_GIT");

/// Lista pública: 4 tools. Os 23 nomes antigos continuam válidos em `tools/call`.
fn expand_tool(name: &str, args: &Value) -> String {
    match name {
        "remember"
            if args["user"].as_str().is_some_and(|s| !s.is_empty())
                && args["response"].as_str().is_some_and(|s| !s.is_empty()) =>
        {
            "remember_episodic".into()
        }
        "recall" if args["entities"].as_array().is_some_and(|a| !a.is_empty()) => {
            "recall_entities".into()
        }
        "recall" if args["at"].as_u64().unwrap_or(0) > 0 => "recall_temporal".into(),
        "recall" if args["rag"].as_bool().unwrap_or(false) => "rag_context".into(),
        "health" => match args["view"].as_str().unwrap_or("status") {
            "validate" => "validate".into(),
            "era" | "era_report" => "era_report".into(),
            _ => "health".into(),
        },
        "curate" => args["op"].as_str().unwrap_or("curate").to_string(),
        other => other.to_string(),
    }
}

fn mcp_listed_tools() -> Value {
    json!([
        {"name":"remember",
         "description":"Write. text= fato semantico L4; user+response= episodico verbatim L2. INVARIANTE: scope nao vaza no recall global; devolve md/L4/... + recall_hint. Duas passadas: recall ANTES. Mesmo embedding/dim. entities= strings identicas na busca. type= text|json|code|embedding|binary. Demo NAO e semantico.",
         "inputSchema":{"type":"object","properties":{
           "text":{"type":"string"},
           "user":{"type":"string","description":"Com `response`: episodio L2 verbatim"},
           "response":{"type":"string"},
           "now":{"type":"integer"},
           "embedding":{"type":"array","items":{"type":"number"}},
           "scope":{"type":"string"},
           "entities":{"type":"array","items":{"type":"string"}},
           "type":{"type":"string","enum":["text","json","code","embedding","binary"]}
         }},
         "annotations":{"destructiveHint":true,"idempotentHint":true}},
        {"name":"recall",
         "description":"Read. Default: busca semantica/lexical/hybrid. entities[]= 1-hop; at= temporal; rag=true monta contexto. INVARIANTE: sem scope= so globais; vazio != inexistente. Doutrina: scope=nsgdb/doctrine entities=doc/protocol. format=json hits tipados.",
         "inputSchema":{"type":"object","properties":{
           "query":{"type":"string"},
           "mode":{"type":"string","enum":["semantic","lexical","hybrid"],"default":"semantic"},
           "format":{"type":"string","enum":["json"]},
           "embedding":{"type":"array","items":{"type":"number"}},
           "k":{"type":"integer","minimum":1,"maximum":20,"default":5},
           "scope":{"type":"string"},
           "cursor":{"type":"string"},
           "pageSize":{"type":"integer","minimum":1,"maximum":20,"default":5},
           "entities":{"type":"array","items":{"type":"string"},"description":"Se nao-vazio: recall_entities (query opcional)"},
           "at":{"type":"integer","description":"Se setado: recall_temporal"},
           "w_sem":{"type":"number","default":1.0},
           "w_time":{"type":"number","default":10.0},
           "rag":{"type":"boolean","default":false},
           "rerank":{"type":"boolean","default":false},
           "historical":{"type":"boolean","default":false}
         }},
         "annotations":{"readOnlyHint":true}},
        {"name":"health",
         "description":"Observabilidade. view=status (default): onboarding+doutrina+dims. view=validate: integridade. view=era: era_report ADR-0007 (dim mismatch). Chame cedo.",
         "inputSchema":{"type":"object","properties":{
           "view":{"type":"string","enum":["status","validate","era"],"default":"status"}
         }},
         "annotations":{"readOnlyHint":true}},
        {"name":"curate",
         "description":"Mutacao pontual / grafo L6. op= explain|reinforce|feedback|forget|expire_old|diary|profile|associate|related_to|contradicts|supersede|conflicts|resolve_conflict|merge_memories. Use a storage key completa md/L4/.... Nao hoarde: so depois de evidência.",
         "inputSchema":{"type":"object","properties":{
           "op":{"type":"string","enum":["explain","reinforce","feedback","forget","expire_old","diary","profile","associate","related_to","contradicts","supersede","conflicts","resolve_conflict","merge_memories"]},
           "key":{"type":"string"},
           "delta":{"type":"number"},
           "positive":{"type":"boolean"},
           "amount":{"type":"number"},
           "now":{"type":"integer"},
           "node_id":{"type":"integer"},
           "limit":{"type":"integer"},
           "a":{"type":"string"},
           "b":{"type":"string"},
           "kind":{"type":"string"},
           "old":{"type":"string"},
           "new":{"type":"string"},
           "conflict_id":{"type":"string"},
           "winner_version_id":{"type":"string"},
           "target":{"type":"string"}
         },"required":["op"]}}
    ])
}

fn binary_runtime_info() -> (String, Option<u64>) {
    std::env::current_exe()
        .ok()
        .map(|p| {
            let mtime = p.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs())
            });
            (p.display().to_string(), mtime)
        })
        .unwrap_or_else(|| ("unknown".into(), None))
}

fn mcp_tool_result(text: &str, structured: Value, is_error: bool) -> Value {
    json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": structured,
        "isError": is_error
    })
}

fn embedder_label() -> String {
    std::env::var("NEURAL_SGDB_EMBEDDER").unwrap_or_else(|_| "demo".into())
}

/// Erros acionáveis para o agente (S1/era guard, contrato de embedding).
fn mcp_actionable_error(e: impl std::fmt::Display) -> String {
    let msg = e.to_string();
    let lower = msg.to_ascii_lowercase();
    if lower.contains("indexed_embedding_dims")
        || lower.contains("era_report")
        || lower.contains("era do corpus")
        || (lower.contains("invalid") && lower.contains("dim"))
    {
        return format!(
            "{msg}\n\nacao: chame a tool `era_report` (read-only) para veredito \
             empty/ok/mixed_dims, dims indexadas e custo estimado de migracao."
        );
    }
    if lower.contains("embedding") || lower.contains("dimens") {
        return format!(
            "{msg}\n\ncontrato: use o MESMO modelo/dimensao em remember e recall, \
             ou forneca `embedding` explicito no payload de ambos."
        );
    }
    msg
}

fn health_payload(db: &mut Sgdb, db_path: &str, embedder: &str) -> Value {
    let h = db.health();
    let (binary_path, binary_mtime) = binary_runtime_info();
    json!({
        "backend": h.backend,
        "node_id": h.node_id,
        "storage_ok": h.storage_ok,
        "doc_count": h.doc_count,
        "bq_len": h.bq_len,
        "ram_len": h.ram_len,
        "open_conflicts": h.open_conflicts,
        "global_memory_count": h.global_memory_count,
        "scoped_memory_count": h.scoped_memory_count,
        "scope_labels": h.scope_labels,
        "db_path": db_path,
        "embedder": embedder,
        "default_scope": db.default_scope(),
        "mcp_contract_version": MCP_CONTRACT_VERSION,
        "mcp_tool_count": EXPECTED_MCP_TOOL_COUNT,
        "indexed_embedding_dims": h.indexed_embedding_dims,
        "demo_embed_dim": neural_sgdb::DEMO_EMBED_DIM,
        "demo_embed_note": neural_sgdb::DEMO_EMBED_NOTE,
        "build_git": BUILD_GIT,
        "binary_path": binary_path,
        "binary_mtime_unix": binary_mtime,
        "contract": "same embedding model/dimension on write and query; demo trigram is NOT semantic",
        "http_embedder": "cargo run --release --example embedder_http — see docs/MCP.md",
        "doctrine_scope": neural_sgdb::DOCTRINE_SCOPE,
        "doctrine_key": format!("md/L4/{}", neural_sgdb::DOCTRINE_KEY),
        "doctrine_entities": neural_sgdb::DOCTRINE_ENTITIES,
        "onboarding": [
            "0. doctrine: initialize.instructions + recall(scope=nsgdb/doctrine, mode=lexical) or recall_entities(['doc/protocol'], scope=nsgdb/doctrine) or resource nsgdb://doctrine",
            "1. remember(text=...) then recall(query=...) with the SAME words (demo embedder)",
            "2. remember(scope=..., entities=[...]) for multi-agent / recall_entities 1-hop",
            "3. recall(format=json) or rag_context(format=json) for typed machine hits",
            "4. remember(type=json|code|embedding|binary) to declare payload type (MDM1 v6)",
            "5. health(view=era) after any dimension/era Invalid error (ADR-0007)"
        ]
    })
}

fn recall_for_mcp(
    db: &mut Sgdb,
    mode: &str,
    scope: &str,
    emb: &[f32],
    query: &str,
    need: usize,
) -> Result<Vec<neural_sgdb::Hit>, String> {
    let r = match (mode, scope.is_empty()) {
        ("lexical", true) => db.recall_lexical(query, need),
        ("lexical", false) => db.recall_lexical_scoped(query, need, scope),
        ("hybrid", true) => db.recall_hybrid(emb, query, need),
        ("hybrid", false) => db.recall_hybrid_scoped(emb, query, need, scope),
        (_, true) => db.recall(emb, need),
        _ => db.recall_scoped(emb, need, scope),
    };
    r.map_err(mcp_actionable_error)
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
    if let Ok(scope) = std::env::var("NEURAL_SGDB_DEFAULT_SCOPE") {
        if !scope.is_empty() {
            db.set_default_scope(Some(scope));
        }
    }
    let embedder = load_embedder();
    let embedder_name = embedder_label();
    match embedder.embed(DOCTRINE) {
        Ok(emb) => match db.ensure_doctrine(&emb) {
            Ok(true) => eprintln!("[neural-sgdb] doctrine seeded (scope={DOCTRINE_SCOPE})"),
            Ok(false) => eprintln!("[neural-sgdb] doctrine already present"),
            Err(e) => eprintln!("[neural-sgdb] doctrine seed skipped: {e}"),
        },
        Err(e) => eprintln!("[neural-sgdb] doctrine embed failed: {e}"),
    }
    let (bin_path, bin_mtime) = binary_runtime_info();
    eprintln!(
        "[neural-sgdb] mcp={MCP_CONTRACT_VERSION} tools={EXPECTED_MCP_TOOL_COUNT} git={BUILD_GIT} \
         binary={bin_path} mtime={bin_mtime:?} db={db_path} backend={} default_scope={:?}",
        db.backend(),
        db.default_scope()
    );

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
                    "capabilities":{"tools":{},"resources":{}},
                    "instructions": DOCTRINE,
                    "serverInfo":{
                        "name":"neural-sgdb",
                        "version":MCP_CONTRACT_VERSION,
                        "title":"neural-sgdb cognitive memory MCP",
                        "mcp_contract_version":MCP_CONTRACT_VERSION,
                        "mcp_tool_count":EXPECTED_MCP_TOOL_COUNT
                    }
                }}));
            }
            "notifications/initialized" | "notifications/cancelled" | "notifications/progress" => {
                // fire-and-forget — sem resposta
            }
            "ping" => {
                send(&json!({"jsonrpc":"2.0","id":id,"result":{}}));
            }
            "tools/list" => {
                send(&json!({"jsonrpc":"2.0","id":id,"result":{"tools": mcp_listed_tools()}}));
            }
            "resources/list" => {
                // #8: expõe as memórias como resources `memory://{layer}/{key}`
                // com paginação por cursor opaco (offset).
                let cursor = msg["params"]["cursor"].as_str();
                let size = msg["params"]["pageSize"].as_u64().unwrap_or(20).max(1) as usize;
                let mut all: Vec<Value> = Vec::new();
                all.push(json!({
                    "uri": "nsgdb://doctrine",
                    "name": "agent-doctrine",
                    "mimeType": "text/plain",
                    "description": "How to use neural-sgdb (same as initialize.instructions)"
                }));
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
                if uri == "nsgdb://doctrine" {
                    send(&json!({"jsonrpc":"2.0","id":id,"result":{
                        "contents":[{"uri":uri,"mimeType":"text/plain","text":DOCTRINE}]}}));
                    continue;
                }
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
                let args = &msg["params"]["arguments"];
                let name = expand_tool(msg["params"]["name"].as_str().unwrap_or(""), args);
                match name.as_str() {
                    "remember" => {
                        let text = args["text"].as_str().unwrap_or("");
                        if text.is_empty() {
                            send(&error_response(&id, -32602, "parametro 'text' obrigatorio"));
                            continue;
                        }
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
                                    "content":[{"type":"text","text":format!("{}", mcp_actionable_error(&e))}],"isError":true}}));
                                continue;
                            }
                        };
                        let entities: Vec<&str> = args["entities"]
                            .as_array()
                            .map(|a| a.iter().filter_map(|e| e.as_str()).collect())
                            .unwrap_or_default();
                        let scope_explicit = args["scope"].as_str();
                        let scope_resolved = db.resolve_scope_param(scope_explicit);
                        let opts = neural_sgdb::RememberOptions {
                            scope: Some(scope_resolved.as_str()),
                            entities: &entities,
                            content_type: args["type"].as_str(),
                        };
                        match db.remember_semantic_with(&key, text, &emb, opts) {
                            Ok(out) => {
                                let structured = json!({
                                    "storage_key": out.storage_key,
                                    "companion_key": out.companion_key,
                                    "scope": out.scope,
                                    "entities": out.entities,
                                    "content_type": out.content_type,
                                    "recall_hint": out.recall_hint
                                });
                                let prose = format!(
                                    "memoria armazenada ({})\nscope={:?}\n{}",
                                    out.storage_key, out.scope, out.recall_hint
                                );
                                send(&json!({"jsonrpc":"2.0","id":id,"result":
                                    mcp_tool_result(&prose, structured, false)}));
                            }
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(&e)}],"isError":true}})),
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
                                "content":[{"type":"text","text":format!("{}", mcp_actionable_error(&e))}],"isError":true}})),
                        }
                    }
                    "recall" => {
                        let query = args["query"].as_str().unwrap_or("");
                        let k = args["k"].as_u64().unwrap_or(5) as usize;
                        if query.is_empty() {
                            send(&error_response(&id, -32602, "parametro 'query' obrigatorio"));
                            continue;
                        }
                        // v1.1.4 item 8 — modo de retrieval (cognee search_type):
                        // semantic (default, precisa embedding), lexical (BM25,
                        // sem embedding), hybrid (semântico + lexical).
                        let mode = args["mode"].as_str().unwrap_or("semantic");
                        let emb = if mode == "lexical" {
                            Vec::new()
                        } else {
                            match embed_for(embedder.as_ref(), query, args) {
                                Ok(e) => e,
                                Err(e) => {
                                    send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                        "content":[{"type":"text","text":format!("{}", mcp_actionable_error(&e))}],"isError":true}}));
                                    continue;
                                }
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
                        // v1.1.4 item 7 — scope: explícito ou default (env/core).
                        let scope = db.resolve_scope_param(args["scope"].as_str());
                        let all = match recall_for_mcp(&mut db, mode, &scope, &emb, query, need) {
                            Ok(h) => h,
                            Err(e) => {
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":e}],"isError":true}}));
                                continue;
                            }
                        };
                        let (page, next) = paginate(&all, args["cursor"].as_str(), size);
                        let json_fmt = args["format"].as_str().unwrap_or("") == "json";
                        let text = if json_fmt {
                            hits_json(&page)
                        } else if page.is_empty() {
                            db.recall_empty_hint(&scope, mode)
                                .unwrap_or_else(|| "nenhuma memoria similar encontrada".into())
                        } else {
                            page.iter().map(fmt_hit).collect::<Vec<_>>().join("\n")
                        };
                        let structured = if json_fmt {
                            json!({"hits": page.iter().map(hit_json).collect::<Vec<_>>(), "scope": scope, "mode": mode})
                        } else {
                            json!({"hit_count": page.len(), "scope": scope, "mode": mode})
                        };
                        let mut result = mcp_tool_result(&text, structured, false);
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
                        // v1.1.6 — mode = caminho de retrieval (mesmo contrato do
                        // recall): semantic (default, core rag_context), lexical
                        // (BM25, sem embedding) e hybrid (semântico + lexical).
                        let mode = args["mode"].as_str().unwrap_or("semantic");
                        let emb = if mode == "lexical" {
                            Vec::new()
                        } else {
                            match embed_for(embedder.as_ref(), query, args) {
                                Ok(e) => e,
                                Err(e) => {
                                    send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                        "content":[{"type":"text","text":format!("{}", mcp_actionable_error(&e))}],"isError":true}}));
                                    continue;
                                }
                            }
                        };
                        let json_fmt = args["format"].as_str().unwrap_or("") == "json";
                        let result_text = match mode {
                            "lexical" => {
                                let hits = db.recall_lexical(query, k).unwrap_or_default();
                                if json_fmt {
                                    hits_json(&hits)
                                } else if hits.is_empty() {
                                    "nenhum contexto recuperado".into()
                                } else {
                                    hits.iter().map(fmt_hit).collect::<Vec<_>>().join("\n")
                                }
                            }
                            "hybrid" => {
                                let hits = db.recall_hybrid(&emb, query, k).unwrap_or_default();
                                if json_fmt {
                                    hits_json(&hits)
                                } else if hits.is_empty() {
                                    "nenhum contexto recuperado".into()
                                } else {
                                    hits.iter().map(fmt_hit).collect::<Vec<_>>().join("\n")
                                }
                            }
                            _ => {
                                match recall_for_mcp(&mut db, "semantic", "", &emb, query, k) {
                                    Err(e) => {
                                        send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                            "content":[{"type":"text","text":e}],"isError":true}}));
                                        continue;
                                    }
                                    Ok(hits) => {
                                        if json_fmt {
                                            hits_json(&hits)
                                        } else if hits.is_empty() {
                                            "nenhum contexto recuperado".into()
                                        } else if args["rerank"].as_bool().unwrap_or(false) {
                                            db.rag_context_reranked(&emb, query, k)
                                                .map_err(mcp_actionable_error)
                                                .unwrap_or_else(|e| e)
                                        } else {
                                            db.rag_context(&emb, k)
                                                .map_err(mcp_actionable_error)
                                                .unwrap_or_else(|e| e)
                                        }
                                    }
                                }
                            }
                        };
                        let is_err = result_text.contains("acao: chame a tool `era_report`")
                            || result_text.starts_with("contrato:")
                            || result_text.starts_with("erro:");
                        send(&json!({"jsonrpc":"2.0","id":id,"result":{
                            "content":[{"type":"text","text":result_text}],
                            "isError":is_err}}));
                    }
                    "recall_temporal" => {
                        let query = args["query"].as_str().unwrap_or("");
                        let at = args["at"].as_u64().unwrap_or(0);
                        let k = args["k"].as_u64().unwrap_or(5) as usize;
                        if query.is_empty() || at == 0 {
                            send(&error_response(&id, -32602, "parametros 'query' e 'at' obrigatorios"));
                            continue;
                        }
                        let emb = match embed_for(embedder.as_ref(), query, args) {
                            Ok(e) => e,
                            Err(e) => {
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":format!("{}", mcp_actionable_error(&e))}],"isError":true}}));
                                continue;
                            }
                        };
                        let w_sem = args["w_sem"].as_f64().unwrap_or(1.0) as f32;
                        let w_time = args["w_time"].as_f64().unwrap_or(10.0) as f32;
                        let scope = args["scope"].as_str().unwrap_or("");
                        let hits = if scope.is_empty() {
                            db.recall_temporal(&emb, k, at, w_sem, w_time)
                        } else {
                            db.recall_temporal_scoped(&emb, k, at, w_sem, w_time, scope)
                        };
                        match hits {
                            Ok(hs) if hs.is_empty() => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":"nenhuma memoria valida em at"}],"isError":false}})),
                            Ok(hs) => {
                                let json_fmt = args["format"].as_str().unwrap_or("") == "json";
                                let text = if json_fmt {
                                    hits_json(&hs)
                                } else {
                                    hs.iter().map(fmt_hit).collect::<Vec<_>>().join("\n")
                                };
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":text}],"isError":false}}))
                            }
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("{}", mcp_actionable_error(&e))}],"isError":true}})),
                        }
                    }
                    "recall_entities" => {
                        let entities: Vec<&str> = args["entities"]
                            .as_array()
                            .map(|a| a.iter().filter_map(|e| e.as_str()).collect())
                            .unwrap_or_default();
                        if entities.is_empty() {
                            send(&error_response(&id, -32602, "parametro 'entities' obrigatorio (lista nao-vazia)"));
                            continue;
                        }
                        let k = args["k"].as_u64().unwrap_or(5) as usize;
                        let scope = args["scope"].as_str().unwrap_or("");
                        let historical = args["historical"].as_bool().unwrap_or(false);
                        let hits = if scope.is_empty() {
                            if historical {
                                db.recall_entities_historical(&entities, k)
                            } else {
                                db.recall_entities(&entities, k)
                            }
                        } else if historical {
                            db.recall_entities_scoped_historical(&entities, k, scope)
                        } else {
                            db.recall_entities_scoped(&entities, k, scope)
                        };
                        match hits {
                            Ok(hs) if hs.is_empty() => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":"nenhuma memoria com essas entidades"}],"isError":false}})),
                            Ok(hs) => {
                                let json_fmt = args["format"].as_str().unwrap_or("") == "json";
                                let text = if json_fmt {
                                    hits_json(&hs)
                                } else {
                                    hs.iter().map(fmt_hit).collect::<Vec<_>>().join("\n")
                                };
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":text}],"isError":false}}))
                            }
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("{}", mcp_actionable_error(&e))}],"isError":true}})),
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
                                "content":[{"type":"text","text":format!("{}", mcp_actionable_error(&e))}],"isError":true}})),
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
                                "content":[{"type":"text","text":format!("{}", mcp_actionable_error(&e))}],"isError":true}})),
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
                                "content":[{"type":"text","text":format!("{}", mcp_actionable_error(&e))}],"isError":true}})),
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
                                "content":[{"type":"text","text":format!("{}", mcp_actionable_error(&e))}],"isError":true}})),
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
                                "content":[{"type":"text","text":format!("{}", mcp_actionable_error(&e))}],"isError":true}})),
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
                                "content":[{"type":"text","text":format!("{}", mcp_actionable_error(&e))}],"isError":true}})),
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
                                "content":[{"type":"text","text":format!("{}", mcp_actionable_error(&e))}],"isError":true}})),
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
                                "content":[{"type":"text","text":format!("{}", mcp_actionable_error(&e))}],"isError":true}})),
                        }
                    }
                    "health" => {
                        let payload = health_payload(&mut db, &db_path, &embedder_name);
                        let text = serde_json::to_string_pretty(&payload).unwrap_or_default();
                        send(&json!({"jsonrpc":"2.0","id":id,"result":
                            mcp_tool_result(&text, payload, false)}));
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
                                "content":[{"type":"text","text":format!("{}", mcp_actionable_error(&e))}],"isError":true}})),
                        }
                    }
                    "profile" => {
                        let node = args["node_id"].as_u64().map(|n| n as u8).unwrap_or(db.node_id());
                        let limit = args["limit"].as_u64().unwrap_or(10) as usize;
                        match db.profile(node, limit) {
                            Ok(facts) => {
                                let text = if facts.is_empty() {
                                    format!("sem fatos estaveis do agente {node}")
                                } else {
                                    facts.iter().map(|(k, imp, conf, p)| {
                                        format!("{} [imp={:.2} conf={:.2}] | {}", k, imp, conf, p)
                                    }).collect::<Vec<_>>().join("\n")
                                };
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":text}],"isError":false}}));
                            }
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("{}", mcp_actionable_error(&e))}],"isError":true}})),
                        }
                    }
                    "expire_old" => {
                        let now = args["now"].as_u64().unwrap_or_else(|| {
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0)
                        });
                        match db.expire_old(now) {
                            Ok(n) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("{n} memorias expiradas em now={now}")}],"isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("{}", mcp_actionable_error(&e))}],"isError":true}})),
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
                    "era_report" => {
                        match db.era_report_lines() {
                            Ok(lines) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":lines.join("\n")}],"isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("{}", mcp_actionable_error(&e))}],"isError":true}})),
                        }
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

    #[test]
    fn mcp_contract_tool_count() {
        assert_eq!(EXPECTED_MCP_TOOL_COUNT, 4);
        assert_eq!(mcp_listed_tools().as_array().map(|a| a.len()), Some(4));
        assert_eq!(expand_tool("era_report", &serde_json::json!({})), "era_report");
        assert_eq!(
            expand_tool("health", &serde_json::json!({"view":"era"})),
            "era_report"
        );
        assert_eq!(
            expand_tool("curate", &serde_json::json!({"op":"reinforce"})),
            "reinforce"
        );
        assert_eq!(
            expand_tool(
                "recall",
                &serde_json::json!({"entities":["doc/protocol"]})
            ),
            "recall_entities"
        );
    }

    #[test]
    fn mcp_actionable_error_hints_era_report() {
        let msg = mcp_actionable_error("Invalid: query dims not in indexed_embedding_dims()");
        assert!(msg.contains("era_report"), "{msg}");
    }
}
