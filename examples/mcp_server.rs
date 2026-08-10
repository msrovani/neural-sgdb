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
//! ⚠️ Embedding de demonstração: o crate standalone não tem modelo de
//! embedding (o kernel usa BGE); aqui usamos hash de trigramas → 256-dim para
//! `recall` funcionar de ponta a ponta. Troque por embeddings reais em
//! produção.

use std::io::{self, BufRead, Write};

use neural_sgdb::{FileStorage, Sgdb};
use serde_json::{json, Value};

/// Contador monotônico para chaves de `remember` (fix #10: mesma chave ms
/// colide — ms*1000 + seq garante unicidade no mesmo milissegundo).
static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Demo embedding: deterministic character-trigram hash → normalized 256-dim
/// vector. Good enough for short-text similarity recall; NOT a real semantic
/// model.
fn demo_embed(text: &str) -> Vec<f32> {
    const DIM: usize = 256;
    let mut v = vec![0f32; DIM];
    let bytes = text.as_bytes();
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    // text < 3 bytes: no trigrams → degenerate zero vector; fallback by
    // individual bytes (fix #10)
    let windows: Vec<&[u8]> = if bytes.len() < 3 {
        bytes.iter().map(|b| std::slice::from_ref(b)).collect()
    } else {
        bytes.windows(3).collect()
    };
    for w in windows {
        // FNV-1a sobre o n-grama
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for &b in w {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
        h ^= seed;
        let idx = (h % DIM as u64) as usize;
        v[idx] += if (h >> 8) & 1 == 1 { 1.0 } else { -1.0 };
        seed = seed.wrapping_mul(0x9E37_79B9).wrapping_add(1);
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-8);
    v.iter_mut().for_each(|x| *x /= norm);
    v
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
    let storage = match FileStorage::open(&db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[neural-sgdb] erro ao abrir {db_path}: {e}");
            std::process::exit(1);
        }
    };
    let mut db = match Sgdb::open(storage) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[neural-sgdb] erro ao iniciar Sgdb: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("[neural-sgdb] MCP server pronto — db={db_path} backend={}", db.backend());

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
                    "serverInfo":{"name":"neural-sgdb","version":"0.1.0"}
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
                     "description":"Armazena uma memoria de texto no banco neural-sgdb.",
                     "inputSchema":{"type":"object",
                       "properties":{"text":{"type":"string","description":"Conteudo a lembrar"}},
                       "required":["text"]}},
                    {"name":"recall",
                     "description":"Busca semantica sobre memorias armazenadas. Retorna as top-k mais similares.",
                     "inputSchema":{"type":"object",
                       "properties":{
                         "query":{"type":"string","description":"Texto de busca"},
                         "k":{"type":"integer","minimum":1,"maximum":20,"default":5}},
                       "required":["query"]}},
                    {"name":"rag_context",
                     "description":"Busca memorias e monta contexto formatado pronto para prompt RAG.",
                     "inputSchema":{"type":"object",
                       "properties":{
                         "query":{"type":"string","description":"Texto de busca"},
                         "k":{"type":"integer","minimum":1,"maximum":10,"default":3}},
                       "required":["query"]}}
                ]}}));
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
                        let emb = demo_embed(text);
                        match db.remember_semantic(&key, text, &emb) {
                            Ok(()) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("memoria armazenada ({key})")}],
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
                        let emb = demo_embed(query);
                        match db.recall(&emb, k) {
                            Ok(hits) => {
                                let text = if hits.is_empty() {
                                    "nenhuma memoria similar encontrada".into()
                                } else {
                                    hits.iter().map(|h| format!("- {} (d={:.3})", h.text, h.dist))
                                        .collect::<Vec<_>>().join("\n")
                                };
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":text}],"isError":false}}));
                            }
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("erro: {e}")}],"isError":true}})),
                        }
                    }
                    "rag_context" => {
                        let query = args["query"].as_str().unwrap_or("");
                        let k = args["k"].as_u64().unwrap_or(3) as usize;
                        if query.is_empty() {
                            send(&error_response(&id, -32602, "parametro 'query' obrigatorio"));
                            continue;
                        }
                        let emb = demo_embed(query);
                        match db.rag_context(&emb, k) {
                            Ok(ctx) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":if ctx.is_empty() {
                                    "nenhum contexto recuperado".into()} else {ctx}}],
                                "isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("erro: {e}")}],"isError":true}})),
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
