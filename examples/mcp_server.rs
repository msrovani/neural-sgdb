//! MCP server (roadmap item 5) â€” exposes neural-sgdb to AI agents
//! (Claude Code, Cursor, OpenCode) via the Model Context Protocol over stdio.
//!
//! Run with: `cargo run --release --example mcp_server` and point the MCP
//! client at the binary (e.g. `claude mcp add neural-sgdb -- cargo run
//! --release --example mcp_server`).
//!
//! ADR-0008: default retrieval is **lexical**. `DemoEmbedder` only if
//! `NEURAL_SGDB_EMBEDDER=demo`. Pass `embedding=` for cosine / L4.
//!
//! Protocolo: JSON-RPC 2.0 sobre stdio, uma mensagem por linha (`\n`), stdout
//! SÃ“ com mensagens MCP (logs â†’ stderr). Handshake legado `2025-11-25`
//! (initialize â†’ initialized â†’ tools/list â†’ tools/call), ver spec em
//! https://modelcontextprotocol.io/specification/2025-11-25/

#![recursion_limit = "256"]

use std::io::{self, BufRead, Write};

use neural_sgdb::{
    CommitFact, CommitRunPlan, CommitSupersede, ContentType, DemoEmbedder, Embedder, MemoryState,
    RecallPath, ScopeDims, ScopeFilter, Sgdb, DOCTRINE, DOCTRINE_SCOPE,
};
// v1.1.28 (ADR-0017): vocabulário ÚNICO prosa/JSON — os dois serializadores
// consomem as mesmas tabelas; o `{:?}` do Rust sai do wire de vez (D1/D8).
use neural_sgdb::{layer_label, path_label, state_label, stable_label};
#[cfg(feature = "file-storage")]
use neural_sgdb::FileStorage;
#[cfg(not(feature = "file-storage"))]
use neural_sgdb::InMemory;
use serde_json::{json, Value};

/// Contador monotÃ´nico para chaves de `remember` (fix #10: mesma chave ms
/// colide â€” ms*1000 + seq garante unicidade no mesmo milissegundo).
static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// `NEURAL_SGDB_EMBEDDER=demo` â†’ trigram explÃ­cito (ADR-0008). Unset = nenhum
/// embedder de host: remember sem `embedding=` vai para L3 lexical.
fn load_embedder() -> Option<Box<dyn Embedder>> {
    match std::env::var("NEURAL_SGDB_EMBEDDER") {
        Ok(s) if s == "demo" => {
            eprintln!("[neural-sgdb] embedder: demo (trigram hash, EXPLICITO)");
            Some(Box::new(DemoEmbedder))
        }
        Ok(other) if !other.is_empty() => {
            eprintln!(
                "[neural-sgdb] embedder '{other}' ainda nao plugado â€” sem host embedder \
                 (passe embedding= ou NEURAL_SGDB_EMBEDDER=demo)"
            );
            None
        }
        _ => {
            eprintln!("[neural-sgdb] embedder: none (ADR-0008 lexical-first)");
            None
        }
    }
}

/// Política do fast-mount do índice (v1.1.29, ADR-0009 §5) — seam de HOST
/// (o core fica livre de env-globals):
/// - `off` (default): `open()` legado, sempre full rebuild.
/// - `auto`: fast-mount quando existir snapshot persistido; o host persiste
///   no `audit_checkpoint` e no encerramento do processo.
/// - `always`: idem auto (o snapshot é opt-in por existência — sem snapshot
///   persistido o open cai para o rebuild de qualquer forma).
#[derive(Debug, Clone, Copy, PartialEq)]
enum IndexSnapshotPolicy {
    Off,
    Auto,
    Always,
}

fn load_index_snapshot_policy() -> IndexSnapshotPolicy {
    match std::env::var("NEURAL_SGDB_INDEX_SNAPSHOT").as_deref() {
        Ok("off") => IndexSnapshotPolicy::Off,
        Ok("always") => IndexSnapshotPolicy::Always,
        Ok("auto") => IndexSnapshotPolicy::Auto,
        _ => IndexSnapshotPolicy::Off,
    }
}

fn has_caller_embedding(payload: &Value) -> bool {
    payload["embedding"]
        .as_array()
        .is_some_and(|a| !a.is_empty())
}

/// ADR-0008: sem `mode` e sem `embedding=` â†’ lexical. semantic/hybrid exigem
/// vetor do caller ou host embedder explÃ­cito.
fn resolve_retrieval_mode(args: &Value, has_host_embedder: bool) -> Result<String, String> {
    let caller = has_caller_embedding(args);
    match args["mode"].as_str() {
        Some("lexical") => Ok("lexical".into()),
        Some(m @ ("semantic" | "hybrid")) => {
            if caller || has_host_embedder {
                Ok(m.into())
            } else {
                Err(
                    "ADR-0008: mode=semantic|hybrid exige `embedding` no payload ou \
                     NEURAL_SGDB_EMBEDDER=demo. Sem vetor, omita mode (default lexical)."
                        .into(),
                )
            }
        }
        Some(other) => Err(format!("mode desconhecido: {other}")),
        None => Ok(if caller {
            "semantic".into()
        } else {
            "lexical".into()
        }),
    }
}

/// Embedding para um texto: usa o fornecido pelo agente (payload) se
/// presente/validÃ¡vel, senÃ£o o embedder ativo do server.
fn embed_for(host: Option<&dyn Embedder>, text: &str, payload: &Value) -> Result<Vec<f32>, String> {
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
    match host {
        Some(e) => e.embed(text).map_err(|e| format!("embedding falhou: {e}")),
        None => Err(
            "ADR-0008: sem `embedding` e sem NEURAL_SGDB_EMBEDDER=demo â€” use mode=lexical \
             ou passe o vetor."
                .into(),
        ),
    }
}

/// #8 â€” parse do URI de resource `memory://{layer}/{key}` (ex: memory://L2/ts/0000).
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

/// #8 â€” paginaÃ§Ã£o com cursor opaco (offset). Retorna (pÃ¡gina, nextCursor).
/// `size` vem do JSON-RPC (entrada externa hostil): clampar impede DoS por
/// alocaÃ§Ã£o gigante; `saturating_add` impede overflow de `off + size`.
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

/// ProjeÃ§Ã£o PROSA de um hit (v1.1.6) â€” o consumidor Ã© outra inteligÃªncia
/// (mÃ¡quina), entÃ£o o sufixo Ã© parseÃ¡vel e TIPADO, nÃ£o sÃ³ prosa, no formato
/// `- {key} | {text} (d=..) [state=.. imp=.. conf=.. src=.. path=.. type=..
/// terms=.. rel=.. valid=..]`.
/// Invariantes preservadas do formato anterior (hot test): prefixo `- {key} | `
/// (a paginaÃ§Ã£o fatia `split(" | ").next()`) e sufixo que abre em ` [state=`
/// (assert `txt.contains("[state=")`).
/// Datum nÃ£o-prosa (Embedding/Binary): `text` vazio no core â€” o consumidor
/// vÃª `type=Embedding(256)` e sabe que o datum Ã© o payload binÃ¡rio do doc,
/// nunca prosa lossy.
fn fmt_hit(h: &neural_sgdb::Hit) -> String {
    let mut tags = Vec::new();
    if let Some(p) = h.provenance.as_ref() {
        tags.push(format!(
            "state={} imp={:.2} conf={:.2} src={}",
            state_label(p.state), p.importance, p.confidence, p.source
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
    tags.push(format!("path={}", path_label(h.path)));
    tags.push(format!("type={}", stable_label(h.content_type)));
    if h.payload_type != h.content_type {
        // datum real do primário (Embedding(dim) p/ L4/L5) vs projeção
        let dim_suf = match h.payload_type {
            ContentType::Embedding(d) => format!("({d})"),
            _ => String::new(),
        };
        tags.push(format!("payload={}{}", stable_label(h.payload_type), dim_suf));
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

/// Strings ESTÃVEIS (machine-parseable) para o `format=json` â€” o consumidor
/// casa por valor, nÃ£o por `Debug` (que pode mudar entre versÃµes).
fn content_type_json(ct: ContentType) -> Value {
    match ct {
        ContentType::Text => json!({"type": "text"}),
        ContentType::Json => json!({"type": "json"}),
        ContentType::Code => json!({"type": "code"}),
        ContentType::Embedding(d) => json!({"type": "embedding", "dim": d}),
        ContentType::Binary => json!({"type": "binary"}),
    }
}

/// Hit estruturado (v1.1.6+) â€” o retorno primÃ¡rio para consumo mÃ¡quinaâ†’
/// mÃ¡quina: o consumidor parseia JSON e vÃª o datum (`type`), o caminho
/// (`path`), o grounding (`matched_terms`) e a proveniÃªncia, sem depender
/// da projeÃ§Ã£o prosa.
fn hit_json(h: &neural_sgdb::Hit) -> Value {
    // v1.1.28 (D2): `dist` só carrega informação onde a distância existe —
    // no lexical é constante 0.0 (a armadilha do "match perfeito"). O
    // consumidor usa `score` (BM25) e `path` como discriminante.
    let dist_val = match h.path {
        RecallPath::Semantic | RecallPath::Entities => json!(h.dist),
        RecallPath::Lexical => Value::Null,
    };
    let mut obj = json!({
        "key": h.key,
        "text": h.text,
        "dist": dist_val,
        "score": h.score,
        "path": path_label(h.path),
        "matched_terms": h.matched_terms,
        "validity": h.validity.map(|(f, u)| json!([f, u])),
        "rel": h.rel,
    });
    let ct = content_type_json(h.content_type);
    obj["type"] = ct["type"].clone();
    obj["dim"] = ct.get("dim").cloned().unwrap_or(Value::Null);
    // item 3 â€” datum real do primÃ¡rio (Embedding(dim)) vs projeÃ§Ã£o (type)
    let pct = content_type_json(h.payload_type);
    obj["payload_type"] = pct["type"].clone();
    obj["payload_dim"] = pct.get("dim").cloned().unwrap_or(Value::Null);
    obj["provenance"] = match h.provenance.as_ref() {
        Some(p) => json!({
            "memory_id": p.memory_id,
            "version_id": p.version_id,
            "layer": layer_label(p.layer),
            "state": state_label(p.state),
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
    // v1.1.27 — scores de tipo SOBREPOSTOS (ADR-0016): derivados na leitura,
    // vocabulary estável (fields() é a tabela única — nunca Debug do Rust).
    obj["type_scores"] = match h.type_scores.as_ref() {
        Some(ts) => {
            let mut m = serde_json::Map::new();
            for (name, v) in ts.fields() {
                m.insert(name.into(), json!(v));
            }
            Value::Object(m)
        }
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

/// NÃºmero de tools em `tools/list` (aliases antigos ainda funcionam em tools/call).
const EXPECTED_MCP_TOOL_COUNT: usize = 5;
const MCP_CONTRACT_VERSION: &str = "1.1.28";
const BUILD_GIT: &str = env!("NEURAL_SGDB_BUILD_GIT");

/// Lista pÃºblica: 4 tools. Os 23 nomes antigos continuam vÃ¡lidos em `tools/call`.
/// As 4 tools LISTADAS no contrato `tools/list` (v1.1.21).
const LISTED_TOOLS: &[&str] = &["remember", "recall", "health", "curate", "decide"];

/// Superfície de ALIAS (v1.1.21): nomes aceitos em `tools/call` que NAO
/// aparecem em `tools/list`.
///
/// Fonte unica do alias surface — antes isto era prosa ("os 23 nomes antigos",
/// a contagem do rework v1.1.8). A superficie cresceu com as ops cognitivas
/// (v1.1.10) e o harness (v1.1.19), e prosa nao trava contra drift: o teste
/// `alias_surface_is_consistent` pina a lista, e `did_you_mean` procura aqui
/// para sugerir o nome certo a quem errou.
const ALIAS_SURFACE: &[&str] = &[
    "recall_candidates",
    "associate",
    "audit_checkpoint",
    "audit_verify",
    "close_event",
    "commit_run",
    "conflicts",
    "consolidate",
    "contradicts",
    "decay",
    "deprecate_run",
    "diary",
    "era_report",
    "expire_old",
    "expire_ttl",
    "explain",
    "feedback",
    "forget",
    "forget_absence",
    "gc",
    "merge_memories",
    "note_absence",
    "profile",
    "rag_context",
    "recall_absences",
    "recall_ann",
    "recall_entities",
    "recall_ledger",
    "recall_temporal",
    "reinforce",
    "related_to",
    "remember_episodic",
    "resolve_conflict",
    "rollback_to",
    "set_event",
    "set_ttl",
    "supersede",
    "timeline",
    "validate",
];

/// Distancia de edicao (Levenshtein, duas linhas). Determinística e sem deps;
/// so roda no caminho de ERRO (nome desconhecido), nunca num recall.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        core::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Sugestao determinística para um nome desconhecido (v1.1.21).
///
/// Ordem: prefixo/substring (typo de digitacao) e, se nada casar, os 3 nomes
/// mais proximos por distancia de edicao (desde que perto o bastante). O mesmo
/// nome devolve sempre a mesma sugestao — um retry do agente e previsivel.
fn did_you_mean(asked: &str) -> Vec<String> {
    let lower = asked.to_ascii_lowercase();
    let mut out: Vec<String> = Vec::new();
    fn push(t: &str, out: &mut Vec<String>) {
        if !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    if !lower.is_empty() {
        for t in LISTED_TOOLS.iter().chain(ALIAS_SURFACE.iter()) {
            if t.starts_with(&lower) || t.contains(&lower) {
                push(t, &mut out);
            }
        }
    }
    if out.is_empty() {
        let mut best: Vec<(usize, &str)> = LISTED_TOOLS
            .iter()
            .chain(ALIAS_SURFACE.iter())
            .map(|t| (edit_distance(&lower, t), *t))
            .collect();
        best.sort();
        for (d, t) in best.iter().take(3) {
            if *d <= 3 {
                push(t, &mut out);
            }
        }
    }
    out.truncate(3);
    out
}

/// Erro de tool desconhecida, ENRIQUECIDO (v1.1.21).
///
/// Continua sendo um erro com o mesmo `-32602` de antes — so ganha `data` com
/// a sugestao e a superficie, para o consumidor maquina consertar o schema numa
/// chamada em vez de queimar um turno adivinhando.
fn unknown_tool_error(id: &Value, asked: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32602,
            "message": "Unknown tool",
            "data": {
                "tool": asked,
                "did_you_mean": did_you_mean(asked),
                "listed_tools": LISTED_TOOLS,
                "alias_count": ALIAS_SURFACE.len(),
            }
        }
    })
}

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
         "description":"Write. Sem embedding= grava L3 lexical (ADR-0008, nao abre era BQ). embedding= ou NEURAL_SGDB_EMBEDDER=demo → L4. user+response= episodico L2. scope/scope_user/agent/app/run nao vaza no recall global. Devolve storage key + recall_hint.",
         "inputSchema":{"type":"object","properties":{
           "text":{"type":"string"},
           "user":{"type":"string","description":"Com `response`: episodio L2 verbatim"},
           "response":{"type":"string"},
           "now":{"type":"integer"},
           "embedding":{"type":"array","items":{"type":"number"}},
           "scope":{"type":"string","description":"Legado (alias user)"},
           "scope_user":{"type":"string"},
           "scope_agent":{"type":"string"},
           "scope_app":{"type":"string"},
           "scope_run":{"type":"string"},
           "model_id":{"type":"string","description":"Era do vetor (MDM1 v7)"},
           "entities":{"type":"array","items":{"type":"string"}},
           "type":{"type":"string","enum":["text","json","code","embedding","binary"]}
         }},
         "annotations":{"destructiveHint":true,"idempotentHint":true}},
        {"name":"recall",
         "description":"Read. Default mode=lexical (ADR-0008) se nao houver embedding=. semantic/hybrid exigem vetor (hybrid usa RRF). entities[]= 1-hop; at= temporal; rag=true. Sem scope= so globais. format=json hits tipados. Session: resource nsgdb://session.",
         "inputSchema":{"type":"object","properties":{
           "query":{"type":"string"},
           "mode":{"type":"string","enum":["semantic","lexical","hybrid"],"default":"lexical"},
           "format":{"type":"string","enum":["json"]},
           "embedding":{"type":"array","items":{"type":"number"}},
           "k":{"type":"integer","minimum":1,"maximum":20,"default":5},
           "scope":{"type":"string"},
           "scope_user":{"type":"string"},
           "scope_agent":{"type":"string"},
           "scope_app":{"type":"string"},
           "scope_run":{"type":"string"},
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
         "description":"Observabilidade. view=status (default): onboarding+doutrina+dims. view=validate: integridade. view=era: era_report ADR-0007. view=tensions: conflitos/superseded/unseen. view=staleness: TTL/Decay/contradicts/aging (read-only). view=index: index_fingerprint (orcaulo do estado derivado, ADR-0011) + custo de open (ADR-0009 §4) — opt-in, O(n log n). Chame cedo. Resource nsgdb://session.",
         "inputSchema":{"type":"object","properties":{
           "view":{"type":"string","enum":["status","validate","era","tensions","staleness","index"],"default":"status"},
           "now":{"type":"integer","description":"Relogio host p/ view=staleness (TTL/aging)"},
           "limit":{"type":"integer","description":"Max items em view=staleness"},
           "scope":{"type":"string","description":"Filtro de scope em view=staleness"}
         }},
         "annotations":{"readOnlyHint":true}},
        {"name":"curate",
         "description":"Mutacao pontual / grafo L6 + metadado cognitivo. op= explain|reinforce|feedback|forget|expire_old|decay|consolidate|diary|profile|associate|related_to|contradicts|supersede|conflicts|resolve_conflict|merge_memories|audit_checkpoint|audit_verify|rollback_to|set_ttl|expire_ttl|set_event|close_event|timeline|gc|recall_ann|commit_run|deprecate_run. ADR-0010: commit_run/deprecate_run usam scope_run (+ facts/anti_patterns). Use a storage key completa md/L4/.... Nao hoarde: so depois de evidencia.",
         "inputSchema":{"type":"object","properties":{
           "op":{"type":"string","enum":["explain","reinforce","feedback","forget","expire_old","decay","consolidate","diary","profile","associate","related_to","contradicts","supersede","conflicts","resolve_conflict","merge_memories","audit_checkpoint","audit_verify","rollback_to","set_ttl","expire_ttl","set_event","close_event","timeline","gc","recall_ann","commit_run","deprecate_run"]},
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
           "target":{"type":"string"},
           "half_life_ms":{"type":"integer"},
           "floor":{"type":"number"},
           "decay_state_at":{"type":"number"},
           "decay_confidence":{"type":"boolean"},
           "min_repeats":{"type":"integer"},
           "min_len":{"type":"integer"},
           "max_new":{"type":"integer"},
           "seq":{"type":"integer"},
           "scope_user":{"type":"string"},
           "scope_agent":{"type":"string"},
           "scope_app":{"type":"string"},
           "scope_run":{"type":"string"},
           "facts":{"type":"array","items":{"type":"object"}},
           "anti_patterns":{"type":"array","items":{"type":"object"}},
           "supersede_pairs":{"type":"array","items":{"type":"object"}},
           "archive_remaining_episodic":{"type":"boolean"},
           "ttl_episodic_ms":{"type":"integer"},
           "close_event_key":{"type":"string"},
           "audit":{"type":"boolean"},
           "archive_episodic":{"type":"boolean"}
         },"required":["op"]}},
        {"name":"decide",
         "description":"System-One control (ADR-0017): N perguntas com espacos de resposta FECHADOS -> respostas tipadas num round trip (Jev-Mem J(S,Q)). ask=evidence_sufficient (adaptive stop do ADR-0012 como resposta), temporal_relation (before/after/same_time por timestamp), valid_at (janela bi-temporal), candidate_relevance (sinais DECOMPOSTOS: sim_vec, lex_overlap, shared_entities, recency, rrf). Deterministico — o core responde, nunca gera texto.",
         "inputSchema":{"type":"object","properties":{
           "questions":{"type":"array","items":{"type":"object","properties":{
             "ask":{"type":"string","enum":["evidence_sufficient","temporal_relation","valid_at","candidate_relevance"]},
             "query":{"type":"string"},
             "key":{"type":"string"},
             "at":{"type":"integer"},
             "a_created":{"type":"integer"},
             "b_created":{"type":"integer"},
             "entities":{"type":"array","items":{"type":"string"}}
           },"required":["ask"]}}
         },"required":["questions"]},
         "annotations":{"readOnlyHint":true}}
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
    std::env::var("NEURAL_SGDB_EMBEDDER").unwrap_or_else(|_| "none".into())
}

/// Erros acionÃ¡veis para o agente (S1/era guard, contrato de embedding).
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

fn mcp_scope_filter(args: &Value) -> ScopeFilter {
    // Only multi-dim keys (v1.1.14). Legacy `scope=` stays on the
    // recall_entities_scoped / resolve_scope_param path — do not promote it
    // into ScopeFilter.user or doctrine/hot-test scoped recalls break.
    ScopeFilter {
        user: args["scope_user"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(String::from),
        agent: args["scope_agent"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(String::from),
        app: args["scope_app"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(String::from),
        run: args["scope_run"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(String::from),
    }
}

struct McpOwnedFact {
    key: String,
    text: String,
    entities: Vec<String>,
    content_type: Option<String>,
    embedding: Option<Vec<f32>>,
}

fn parse_mcp_facts(arr: Option<&Vec<Value>>) -> Vec<McpOwnedFact> {
    let Some(items) = arr else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, v) in items.iter().enumerate() {
        let key = v["key"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(|| format!("commit/{i}"));
        let text = v["text"].as_str().unwrap_or("").to_string();
        if text.is_empty() {
            continue;
        }
        let entities = v["entities"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|e| e.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let content_type = v["type"].as_str().map(String::from);
        let embedding = v["embedding"].as_array().map(|a| {
            a.iter()
                .filter_map(|x| x.as_f64().map(|f| f as f32))
                .collect()
        });
        out.push(McpOwnedFact {
            key,
            text,
            entities,
            content_type,
            embedding,
        });
    }
    out
}

fn mcp_commit_run(db: &mut Sgdb, args: &Value) -> Result<(String, Value), String> {
    let filter = mcp_scope_filter(args);
    let owned_facts = parse_mcp_facts(args["facts"].as_array());
    let owned_antis = parse_mcp_facts(args["anti_patterns"].as_array());
    let pairs_raw = args["supersede_pairs"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let pair_owned: Vec<(String, String)> = pairs_raw
        .iter()
        .filter_map(|p| {
            Some((
                p["old"].as_str()?.to_string(),
                p["new"].as_str()?.to_string(),
            ))
        })
        .collect();

    // Lifetime dance: build CommitFact slices from owned buffers.
    let fact_ents: Vec<Vec<&str>> = owned_facts
        .iter()
        .map(|f| f.entities.iter().map(|s| s.as_str()).collect())
        .collect();
    let anti_ents: Vec<Vec<&str>> = owned_antis
        .iter()
        .map(|f| f.entities.iter().map(|s| s.as_str()).collect())
        .collect();
    let facts: Vec<CommitFact<'_>> = owned_facts
        .iter()
        .enumerate()
        .map(|(i, f)| CommitFact {
            key: f.key.as_str(),
            text: f.text.as_str(),
            entities: fact_ents[i].as_slice(),
            content_type: f.content_type.as_deref(),
            embedding: f.embedding.as_deref(),
        })
        .collect();
    let antis: Vec<CommitFact<'_>> = owned_antis
        .iter()
        .enumerate()
        .map(|(i, f)| CommitFact {
            key: f.key.as_str(),
            text: f.text.as_str(),
            entities: anti_ents[i].as_slice(),
            content_type: f.content_type.as_deref(),
            embedding: f.embedding.as_deref(),
        })
        .collect();
    let supers: Vec<CommitSupersede<'_>> = pair_owned
        .iter()
        .map(|(o, n)| CommitSupersede {
            old: o.as_str(),
            new: n.as_str(),
        })
        .collect();

    let write_dims = ScopeDims::from_args(
        args["scope_user"].as_str().or(args["scope"].as_str()),
        args["scope_agent"].as_str(),
        args["scope_app"].as_str(),
        args["scope_run"].as_str(),
    );
    let now = args["now"].as_u64().unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    });
    let plan = CommitRunPlan {
        facts: &facts,
        anti_patterns: &antis,
        supersede: &supers,
        archive_remaining_episodic: args["archive_remaining_episodic"]
            .as_bool()
            .unwrap_or(false),
        ttl_episodic_ms: args["ttl_episodic_ms"].as_u64(),
        close_event_key: args["close_event_key"].as_str(),
        now,
        audit: args["audit"].as_bool().unwrap_or(false),
        write_dims,
    };
    let r = db.commit_run(&filter, &plan).map_err(mcp_actionable_error)?;
    let text = format!(
        "commit_run: written={} superseded={} archived={} ttl_set={} closed_event={} audit_seq={:?}",
        r.written.len(),
        r.superseded,
        r.archived,
        r.ttl_set,
        r.closed_event,
        r.audit_seq
    );
    let structured = json!({
        "written": r.written,
        "superseded": r.superseded,
        "archived": r.archived,
        "ttl_set": r.ttl_set,
        "closed_event": r.closed_event,
        "audit_seq": r.audit_seq
    });
    Ok((text, structured))
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
        "contract": "ADR-0008: default recall is lexical; demo trigram is NOT semantic and is not implied",
        "http_embedder": "cargo run --release --example embedder_http â€” see docs/MCP.md",
        "doctrine_scope": neural_sgdb::DOCTRINE_SCOPE,
        "doctrine_key": format!("md/L4/{}", neural_sgdb::DOCTRINE_KEY),
        "doctrine_entities": neural_sgdb::DOCTRINE_ENTITIES,
        // v1.1.28 (D4): onboarding TIPADO — {step, text}; o ordinal estava
        // dentro da string (duplicando o índice do array, podia divergir).
        "onboarding": [
            {"step": 0, "text": "cold-start: resource nsgdb://session (campo cold_start) + nsgdb://doctrine; recall CADA scope em health.scope_labels / cold_start.scopes_to_probe — default_scope NAO ve outros scopes"},
            {"step": 1, "text": "remember(text=...) is lexical L3; recall(query=...) default mode=lexical (same words). Cosine: pass embedding= on both, or NEURAL_SGDB_EMBEDDER=demo / embedder_http / nsgdb-embed (host, ADR-0008)"},
            {"step": 2, "text": "multi-agente: scope por agente/tarefa (agent/<id>, project/<repo>); 1 processo mcp_server writer por ficheiro DB — partilhar ficheiro != telepatia CRDT"},
            {"step": 3, "text": "recall(format=json) for typed machine hits"},
            {"step": 4, "text": "remember(type=json|code|embedding|binary) to declare payload type (MDM1 v6)"},
            {"step": 5, "text": "health(view=era) after dim/era Invalid; health(view=tensions) for conflicts/unseen scopes; health(view=staleness) for TTL/Decay/contradicts"},
            {"step": 6, "text": "2+ nos/DBs separados: feature p2p (examples/p2p_telepathy) — conflito preservado, arbitragem na leitura"},
            {"step": 7, "text": "MOM entities on write: mom/constraint|decision|fact|pattern|anti-pattern|learning|pref — identical strings on recall_entities; fim de tarefa: curate(op=commit_run) (ADR-0010)"}
        ]
    })
}

fn tensions_payload(db: &mut Sgdb) -> Value {
    let default = db.default_scope().unwrap_or("").to_string();
    let dist = db.scope_distribution().ok();
    let scope_labels: Vec<Value> = dist
        .as_ref()
        .map(|d| {
            d.scoped
                .iter()
                .map(|(s, c)| json!([s, c]))
                .collect()
        })
        .unwrap_or_default();
    // v1.1.28 (D5): tupla TIPADA (label, count) — era `"scope(count)"`,
    // uma string que o consumidor reparseia. Os dois campos já estavam
    // separados em `scope_labels`; aqui era o único lugar que achatava.
    let unseen_scopes: Vec<Value> = dist
        .as_ref()
        .map(|d| {
            d.scoped
                .iter()
                .filter(|(s, _)| s.as_str() != default)
                .map(|(s, c)| json!({"label": s, "count": c}))
                .collect()
        })
        .unwrap_or_default();
    let conflicts: Vec<Value> = db
        .conflicts()
        .into_iter()
        .map(|c| {
            json!({
                "id": c.conflict_id,
                "subject": c.subject,
                "status": match c.status {
                    neural_sgdb::ConflictStatus::Open => "open",
                    neural_sgdb::ConflictStatus::Resolved => "resolved",
                },
                "candidates": c.candidates,
            })
        })
        .collect();
    let open_n = conflicts
        .iter()
        .filter(|c| c["status"] == "open")
        .count();
    let mut superseded = Vec::new();
    if let Ok(items) = db.scan_prefix("md/") {
        for (k, _) in items {
            if superseded.len() >= 20 {
                break;
            }
            if matches!(db.get_state(&k), Ok(MemoryState::Superseded)) {
                superseded.push(k);
            }
        }
    }
    let empty_hint = db.recall_empty_hint(&default, "lexical");
    json!({
        "view": "tensions",
        "default_scope": default,
        "open_conflicts": open_n,
        "conflicts": conflicts,
        "superseded": superseded,
        "scope_labels": scope_labels,
        "unseen_scopes": unseen_scopes,
        "empty_hint": empty_hint
    })
}

fn session_payload(db: &mut Sgdb, db_path: &str, embedder: &str) -> Value {
    let health = health_payload(db, db_path, embedder);
    let tensions = tensions_payload(db);
    let default_scope = db.default_scope().unwrap_or("").to_string();
    // v1.1.26: a lista de probes vem de `scope_probes()` (procedência do core),
    // NÃO de `health.scope_labels` — que é top-8 por EXIBIÇÃO e omitia escopos
    // reais. Dois bugs num só: o truncamento escondia o 9º escopo, e a versão
    // antiga do `scope_distribution` contava dims não-`user` como globais, então
    // nem apareciam lá.
    let probes = db.scope_probes().unwrap_or_default();
    let mut scopes_to_probe: Vec<String> = Vec::new();
    for (label, _) in &probes.legacy {
        if !label.is_empty() && !scopes_to_probe.iter().any(|s| s == label) {
            scopes_to_probe.push(label.clone());
        }
    }
    if !default_scope.is_empty() && !scopes_to_probe.iter().any(|s| s == &default_scope) {
        scopes_to_probe.insert(0, default_scope.clone());
    }
    // Always probe doctrine — seed exists even if scope_labels omitted it briefly.
    if !scopes_to_probe.iter().any(|s| s == neural_sgdb::DOCTRINE_SCOPE) {
        scopes_to_probe.push(neural_sgdb::DOCTRINE_SCOPE.to_string());
    }
    // A rota que FALTAVA: `scope=` não nomeia estas. Cada entrada é um descritor
    // pronto para o `recall` — o consumidor não precisa parsear o rótulo (um
    // `scope` legado pode conter `/`, então por forma as duas listas seriam
    // indistinguíveis).
    let scopes_to_probe_dims: Vec<Value> = probes
        .dims_only
        .iter()
        .map(|(label, count)| {
            let mut seg = label.split('/');
            json!({
                "label": label,
                "user": seg.next().unwrap_or(""),
                "agent": seg.next().unwrap_or(""),
                "app": seg.next().unwrap_or(""),
                "run": seg.next().unwrap_or(""),
                "count": count,
            })
        })
        .collect();
    let cold_start = json!({
        "protocol": "gather-then-act",
        // v1.1.28 (D5): {step, text} tipado — o ordinal NÃO está mais dentro
        // da string; a ordem do array é a única fonte de sequência.
        "steps": [
            {"step": "1", "text": "Ler este resource (nsgdb://session) e nsgdb://doctrine"},
            {"step": "2", "text": "Para cada scope em scopes_to_probe: recall(mode=lexical, scope=..., k=5) OU recall(entities=[...], scope=...)"},
            {"step": "2b", "text": "Para cada entrada de scopes_to_probe_dims: recall(mode=lexical, k=5, scope_user/agent/app/run=os campos dela) — estas NAO sao alcancaveis por scope= (nem hybrid/temporal: eles recusam dims)"},
            {"step": "3", "text": "Preferencias IDE: entities pref/idioma, pref/memoria, nsgdb/usage, mom/pref no default_scope"},
            {"step": "4", "text": "Constraints de projeto: entities mom/constraint, adr/index, roadmap/non-goals, docs/telepathy no scope do repo (constraints primeiro)"},
            {"step": "5", "text": "health(view=staleness) — classifica aging/TTL/contradicts; curate manual (nao auto-forget)"},
            {"step": "6", "text": "So entao remember — MOM roles mom/*; fato identico → reinforce; nao hoarde; 1 writer por DB file"}
        ],
        "default_scope": default_scope,
        "scopes_to_probe": scopes_to_probe,
        "scopes_to_probe_dims": scopes_to_probe_dims,
        "unseen_scopes": tensions.get("unseen_scopes").cloned().unwrap_or(json!([])),
        "single_writer": "Um processo mcp_server por ficheiro NEURAL_SGDB_DB; dois writers no mesmo FileStorage e risco. Partilha de ficheiro = memorias comuns, nao sync CRDT.",
        "telepathy_when": "Dois ou mais Sgdb com DBs/nos distintos → cargo run --release --example p2p_telepathy --features p2p",
        "embedder_host": "Semantic/hybrid: embedding= nas tools, ou NEURAL_SGDB_EMBEDDER=demo (trigrama), ou examples/embedder_http / crates/nsgdb-embed (ADR-0008 — nunca no core)"
    });
    json!({
        "resource": "nsgdb://session",
        "recall_default": "lexical",
        "cold_start": cold_start,
        "health": health,
        "tensions": tensions
    })
}

/// Dims anunciadas sem rota no core → RECUSA explícita, nunca silêncio.
/// (Devolver o pool global quando o chamador pediu `scope_run` seria vazar
/// escopo, que é o que o null-scoping existe para impedir.)
const ERR_DIMS_HYBRID: &str = "scope_user/agent/app/run nao suportados em mode=hybrid \
(o RRF funde dois pools e nao tem variante de dims): use mode=lexical ou mode=semantic, \
ou filtre por scope= (legado). Nada foi retornado para nao vazar escopo.";
const ERR_DIMS_TEMPORAL: &str = "scope_user/agent/app/run nao suportados em recall_temporal \
(o re-rank temporal obra sobre o pool semantico, sem variante de dims): use scope= (legado) \
ou um dos campos na resposta. Nada foi retornado para nao vazar escopo.";

/// Rota de recall do MCP.
///
/// v1.1.26 — o schema ANUNCIA `scope_user/agent/app/run` no `recall` desde o
/// v1.1.14, e este caminho as IGNORAVA: `recall(scope_run="x")` respondia igual
/// a `recall()` — 0 hits com `isError:false` e um hint em prosa. Mesma familia
/// do `view=index` do v1.1.23 (anunciado != servido), invertida. Pior: o
/// sub-modo `entities` JA honrava dims (roteado para `recall_entities`), entao
/// dois usos do MESMO tool se comportavam de forma diferente.
///
/// Dims presentes vencem o `scope` legado: o filtro multi-dim e estritamente
/// mais especifico.
fn recall_for_mcp(
    db: &mut Sgdb,
    mode: &str,
    scope: &str,
    filter: &neural_sgdb::ScopeFilter,
    emb: &[f32],
    query: &str,
    need: usize,
) -> Result<Vec<neural_sgdb::Hit>, String> {
    let r = if !filter.is_global_only() {
        match mode {
            "lexical" => db.recall_lexical_dims(query, need, filter),
            "hybrid" => return Err(ERR_DIMS_HYBRID.into()),
            _ => db.recall_scoped_dims(emb, need, filter),
        }
    } else {
        match (mode, scope.is_empty()) {
            ("lexical", true) => db.recall_lexical(query, need),
            ("lexical", false) => db.recall_lexical_scoped(query, need, scope),
            ("hybrid", true) => db.recall_hybrid_rrf(emb, query, need),
            ("hybrid", false) => db.recall_hybrid_rrf_scoped(emb, query, need, scope),
            (_, true) => db.recall(emb, need),
            _ => db.recall_scoped(emb, need, scope),
        }
    };
    r.map_err(mcp_actionable_error)
}

fn main() {
    let db_path = std::env::var("NEURAL_SGDB_DB").unwrap_or_else(|_| "sgdb_memory.db".into());

    // Backend concreto por feature: `FileStorage` (persistente) ou `InMemory`
    // (demo volÃ¡til) â€” `Sgdb::open(impl Storage)` aceita ambos sem boxing.
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
        eprintln!("[neural-sgdb] file-storage desativada â€” usando InMemory (volÃ¡til)");
        InMemory::new()
    };

    let snapshot_policy = load_index_snapshot_policy();
    #[cfg(feature = "file-storage")]
    let mut db = match snapshot_policy {
        // Fast-mount: monta do snapshot IDX1 se existir e validar; senão
        // rebuild completo (mesma garantia, ADR-0009 §1).
        IndexSnapshotPolicy::Auto | IndexSnapshotPolicy::Always => {
            match Sgdb::open_with_snapshot(1, FileStorage::open(&db_path).expect("db")) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("[neural-sgdb] erro ao iniciar Sgdb: {e}");
                    std::process::exit(1);
                }
            }
        }
        IndexSnapshotPolicy::Off => match Sgdb::open(storage) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("[neural-sgdb] erro ao iniciar Sgdb: {e}");
                std::process::exit(1);
            }
        },
    };
    #[cfg(not(feature = "file-storage"))]
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
    // Doutrina: seed L4 com DemoEmbedder interno (texto canÃ´nico, nÃ£o o default do produto).
    match DemoEmbedder.embed(DOCTRINE) {
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
         binary={bin_path} mtime={bin_mtime:?} db={db_path} backend={} default_scope={:?} idx_snapshot={snapshot_policy:?}",
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
                // Version negotiation: respondemos o legado estÃ¡vel 2025-11-25
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
                // fire-and-forget â€” sem resposta
            }
            "ping" => {
                send(&json!({"jsonrpc":"2.0","id":id,"result":{}}));
            }
            "tools/list" => {
                send(&json!({"jsonrpc":"2.0","id":id,"result":{"tools": mcp_listed_tools()}}));
            }
            "resources/list" => {
                // #8: expÃµe as memÃ³rias como resources `memory://{layer}/{key}`
                // com paginaÃ§Ã£o por cursor opaco (offset).
                let cursor = msg["params"]["cursor"].as_str();
                let size = msg["params"]["pageSize"].as_u64().unwrap_or(20).max(1) as usize;
                let mut all: Vec<Value> = Vec::new();
                all.push(json!({
                    "uri": "nsgdb://doctrine",
                    "name": "agent-doctrine",
                    "mimeType": "text/plain",
                    "description": "How to use neural-sgdb (same as initialize.instructions)"
                }));
                all.push(json!({
                    "uri": "nsgdb://session",
                    "name": "cold-start",
                    "mimeType": "application/json",
                    "description": "health + tensions + doctrine pointers (ADR-0008 lexical default)"
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
                if uri == "nsgdb://session" {
                    let payload = session_payload(&mut db, &db_path, &embedder_name);
                    let text = serde_json::to_string_pretty(&payload).unwrap_or_default();
                    send(&json!({"jsonrpc":"2.0","id":id,"result":{
                        "contents":[{"uri":uri,"mimeType":"application/json","text":text}]}}));
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
                            scope_dims: neural_sgdb::ScopeDims::from_args(
                                args["scope_user"].as_str(),
                                args["scope_agent"].as_str(),
                                args["scope_app"].as_str(),
                                args["scope_run"].as_str(),
                            ),
                            model_id: args["model_id"].as_str(),
                        };
                        let semantic = has_caller_embedding(args) || embedder.is_some();
                        let written = if semantic {
                            match embed_for(embedder.as_deref(), text, args) {
                                Ok(emb) => db.remember_semantic_with(&key, text, &emb, opts),
                                Err(e) => {
                                    send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                        "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}}));
                                    continue;
                                }
                            }
                        } else {
                            db.remember_text_with(&key, text, opts)
                        };
                        match written {
                            Ok(out) => {
                                let indexed = if semantic { "semantic" } else { "lexical" };
                                let structured = json!({
                                    "storage_key": out.storage_key,
                                    "companion_key": out.companion_key,
                                    "scope": out.scope,
                                    "entities": out.entities,
                                    "content_type": out.content_type,
                                    "recall_hint": out.recall_hint,
                                    "indexed": indexed
                                });
                                let prose = format!(
                                    "memoria armazenada ({})\nindexed={indexed}\nscope={:?}\n{}",
                                    out.storage_key, out.scope, out.recall_hint
                                );
                                send(&json!({"jsonrpc":"2.0","id":id,"result":
                                    mcp_tool_result(&prose, structured, false)}));
                            }
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
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
                        let dims = neural_sgdb::ScopeDims::from_args(
                            args["scope_user"].as_str().or(args["scope"].as_str()),
                            args["scope_agent"].as_str(),
                            args["scope_app"].as_str(),
                            args["scope_run"].as_str(),
                        );
                        let written = match &dims {
                            Some(d) => db.remember_episodic_scoped(user, response, now, d),
                            None => db.remember_episodic(user, response, now),
                        };
                        match written {
                            Ok((ku, ka)) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("episodio verbatim armazenado:\nuser: {ku}\nasst: {ka}")}],
                                "isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "recall" => {
                        let query = args["query"].as_str().unwrap_or("");
                        let k = args["k"].as_u64().unwrap_or(5) as usize;
                        if query.is_empty() {
                            send(&error_response(&id, -32602, "parametro 'query' obrigatorio"));
                            continue;
                        }
                        // v1.1.28 (D6): k=0 é erro tipado, não "sucesso vazio" —
                        // o consumidor precisa distinguir "não havia nada" de
                        // "pediu nada".
                        if k == 0 {
                            send(&error_response(&id, -32602,
                                "k=0 é inválido: para sondar existência use k=1 e leia hits[] (vazio = não havia nada)"));
                            continue;
                        }
                        let mode = match resolve_retrieval_mode(args, embedder.is_some()) {
                            Ok(m) => m,
                            Err(e) => {
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":e}],"isError":true}}));
                                continue;
                            }
                        };
                        let emb = if mode == "lexical" {
                            Vec::new()
                        } else {
                            match embed_for(embedder.as_deref(), query, args) {
                                Ok(e) => e,
                                Err(e) => {
                                    send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                        "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}}));
                                    continue;
                                }
                            }
                        };
                        // v1.1.3 S5 â€” paginaÃ§Ã£o LAZY: computa sÃ³ o que a pÃ¡gina
                        // pede. Antes buscava top-100 fixo e paginava sobre ele
                        // (custo fixo + teto artificial de 100 hits). Top-k Ã©
                        // determinÃ­stico (score, key) â€” top-(off+size) da busca
                        // completa = os mesmos itens de top-100, entÃ£o a pÃ¡gina
                        // fatia o prefixo correto sem custo extra.
                        let size = args["pageSize"].as_u64().unwrap_or(k as u64).max(1) as usize;
                        let off = args["cursor"]
                            .as_str()
                            .and_then(|c| c.parse::<usize>().ok())
                            .unwrap_or(0);
                        // +1 = hit "sentinela" alÃ©m da pÃ¡gina: sem ele, uma
                        // pÃ¡gina exatamente preenchida pareceria a Ãºltima
                        // (paginate usa items.len() como "conjunto inteiro").
                        let need = off.saturating_add(size).saturating_add(1);
                        // v1.1.4 item 7 â€” scope: explÃ­cito ou default (env/core).
                        let scope = db.resolve_scope_param(args["scope"].as_str());
                        // v1.1.26: as dims anunciadas no schema chegam aqui
                        // (o no-op silencioso era o bug).
                        let dims_filter = mcp_scope_filter(args);
                        let all = match recall_for_mcp(
                            &mut db,
                            &mode,
                            &scope,
                            &dims_filter,
                            &emb,
                            query,
                            need,
                        ) {
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
                            db.recall_empty_hint(&scope, &mode)
                                .unwrap_or_else(|| "nenhuma memoria similar encontrada".into())
                        } else {
                            page.iter().map(fmt_hit).collect::<Vec<_>>().join("\n")
                        };
                        // O filtro efetivo e reportado: quem pediu `scope_run`
                        // tem de ver que o pedido foi atendido (e nao que o
                        // `scope` legado venceu por acaso).
                        let filter_echo = json!({
                            "user": dims_filter.user,
                            "agent": dims_filter.agent,
                            "app": dims_filter.app,
                            "run": dims_filter.run
                        });
                        let structured = if json_fmt {
                            json!({"hits": page.iter().map(hit_json).collect::<Vec<_>>(), "scope": scope, "mode": mode, "scope_dims": filter_echo})
                        } else {
                            json!({"hit_count": page.len(), "scope": scope, "mode": mode, "scope_dims": filter_echo})
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
                        let mode = match resolve_retrieval_mode(args, embedder.is_some()) {
                            Ok(m) => m,
                            Err(e) => {
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":e}],"isError":true}}));
                                continue;
                            }
                        };
                        let emb = if mode == "lexical" {
                            Vec::new()
                        } else {
                            match embed_for(embedder.as_deref(), query, args) {
                                Ok(e) => e,
                                Err(e) => {
                                    send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                        "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}}));
                                    continue;
                                }
                            }
                        };
                        let json_fmt = args["format"].as_str().unwrap_or("") == "json";
                        let result_text = match mode.as_str() {
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
                                match recall_for_mcp(
                                    &mut db,
                                    "semantic",
                                    "",
                                    &mcp_scope_filter(args),
                                    &emb,
                                    query,
                                    k,
                                ) {
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
                        let emb = match embed_for(embedder.as_deref(), query, args) {
                            Ok(e) => e,
                            Err(e) => {
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}}));
                                continue;
                            }
                        };
                        let w_sem = args["w_sem"].as_f64().unwrap_or(1.0) as f32;
                        let w_time = args["w_time"].as_f64().unwrap_or(10.0) as f32;
                        let scope = args["scope"].as_str().unwrap_or("");
                        // v1.1.26: dims nao tem rota neste sub-modo — recusa
                        // explicita em vez de responder o pool global.
                        if !mcp_scope_filter(args).is_global_only() {
                            send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":ERR_DIMS_TEMPORAL}],"isError":true}}));
                            continue;
                        }
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
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
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
                        let dims_filter = mcp_scope_filter(args);
                        let hits = if !dims_filter.is_global_only() {
                            // ADR-0010 / v1.1.14: scope_user|agent|app|run
                            db.recall_entities_dims(&entities, k, &dims_filter)
                        } else if scope.is_empty() {
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
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    // ---- Ledger de negativos (v1.1.24 item 7, ADR-0014) ----
                    // Shape JSON PROPRIO: nao e a lista de hits do `format=json`
                    // (compat: quem parseia hits nao ve campo novo nenhum).
                    "recall_absences" => {
                        let limit = args["limit"].as_u64().unwrap_or(50) as usize;
                        let scope = args["scope_user"]
                            .as_str()
                            .or(args["scope"].as_str())
                            .unwrap_or("");
                        let scope_opt = if scope.is_empty() { None } else { Some(scope) };
                        match db.recall_absences(scope_opt, limit) {
                            Ok(list) if list.is_empty() => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":if scope.is_empty() {
                                    String::from("nenhuma ausencia global registrada")
                                } else {
                                    format!("nenhuma ausencia registrada no scope {scope:?}")
                                }}],"isError":false}})),
                            Ok(list) => {
                                let text = if args["format"].as_str().unwrap_or("") == "json" {
                                    serde_json::to_string(&json!(list
                                        .iter()
                                        .map(|e| json!({"query":e.query,"scope":e.scope,"probes":e.probes,
                                            "first_tick":e.first_tick,"last_tick":e.last_tick}))
                                        .collect::<Vec<_>>()))
                                    .unwrap_or_else(|_| String::from("[]"))
                                } else {
                                    list.iter()
                                        .map(|e| format!(
                                            "- {} | scope={} probes={} first={} last={} [ausencia]",
                                            e.query, e.scope, e.probes, e.first_tick, e.last_tick
                                        ))
                                        .collect::<Vec<_>>()
                                        .join("\n")
                                };
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":text}],"isError":false}}))
                            }
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "note_absence" => {
                        let query = args["query"].as_str().unwrap_or("");
                        if query.trim().is_empty() {
                            send(&error_response(&id, -32602, "parametro 'query' obrigatorio"));
                            continue;
                        }
                        let scope = args["scope_user"]
                            .as_str()
                            .or(args["scope"].as_str())
                            .unwrap_or("");
                        let now = args["now"].as_u64().unwrap_or_else(|| {
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0)
                        });
                        match db.note_absence(query, scope, now) {
                            Ok(probes) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!(
                                    "ausencia registrada (probes={probes}, scope={scope:?}) — proximo recall desta query avisa que ja foi procurada")}],
                                "isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "forget_absence" => {
                        let query = args["query"].as_str().unwrap_or("");
                        if query.trim().is_empty() {
                            send(&error_response(&id, -32602, "parametro 'query' obrigatorio"));
                            continue;
                        }
                        let scope = args["scope_user"]
                            .as_str()
                            .or(args["scope"].as_str())
                            .unwrap_or("");
                        match db.forget_absence(query, scope) {
                            Ok(true) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":String::from("ausencia removida")}],"isError":false}})),
                            Ok(false) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":String::from("nenhuma ausencia registrada para essa query (nada a remover)")}],"isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "recall_ledger" => {
                        let query = args["query"].as_str().unwrap_or("");
                        if query.trim().is_empty() {
                            send(&error_response(&id, -32602, "parametro 'query' obrigatorio"));
                            continue;
                        }
                        let k = args["k"].as_u64().unwrap_or(5) as usize;
                        let scope = args["scope_user"]
                            .as_str()
                            .or(args["scope"].as_str())
                            .unwrap_or("");
                        let scope_opt = if scope.is_empty() { None } else { Some(scope) };
                        let now = args["now"].as_u64().unwrap_or_else(|| {
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0)
                        });
                        match db.recall_with_ledger(query, k, scope_opt, now) {
                            Ok(r) => {
                                let absences: Vec<Value> = r
                                    .absences
                                    .iter()
                                    .map(|e| json!({"query":e.query,"scope":e.scope,"probes":e.probes,
                                        "first_tick":e.first_tick,"last_tick":e.last_tick}))
                                    .collect();
                                let text = if args["format"].as_str().unwrap_or("") == "json" {
                                    let hits: Vec<Value> = r.hits.iter().map(hit_json).collect();
                                    serde_json::to_string(&json!({
                                        "hits": hits, "absences": absences, "recorded": r.recorded}))
                                    .unwrap_or_else(|_| String::from("{}"))
                                } else {
                                    let mut lines: Vec<String> =
                                        r.hits.iter().map(fmt_hit).collect();
                                    if lines.is_empty() {
                                        lines.push(String::from("(nenhum hit)"));
                                    }
                                    lines.push(format!(
                                        "[ledger] recorded={} ausencias={}",
                                        r.recorded,
                                        r.absences.len()
                                    ));
                                    for e in &r.absences {
                                        lines.push(format!(
                                            "  - {} probes={} first={} last={}",
                                            e.query, e.probes, e.first_tick, e.last_tick
                                        ));
                                    }
                                    lines.join("\n")
                                };
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":text}],"isError":false}}))
                            }
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
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
                                    "key": ex.key, "layer": layer_label(ex.layer),
                                    "state": state_label(ex.state),
                                    "memory_id": ex.memory_id, "version_id": ex.version_id,
                                    "source": ex.source, "confidence": ex.confidence,
                                    "importance": ex.importance, "created_tick": ex.created_tick,
                                    "last_reinforced": ex.last_reinforced, "parents": ex.parents,
                                    "validity": ex.validity, "children": ex.children})).unwrap_or_default()}],
                                "isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
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
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
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
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
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
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
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
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
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
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
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
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
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
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "health" => {
                        let view = args["view"].as_str().unwrap_or("status");
                        if view == "tensions" {
                            let payload = tensions_payload(&mut db);
                            let text = serde_json::to_string_pretty(&payload).unwrap_or_default();
                            send(&json!({"jsonrpc":"2.0","id":id,"result":
                                mcp_tool_result(&text, payload, false)}));
                        } else if view == "index" {
                            // v1.1.21 (ADR-0011): oraculo de equivalencia de
                            // reconstrucao (`fp(open) == fp(rebuild_indices())`)
                            // + o contrato de medicao de custo de open
                            // (ADR-0009 §4). Opt-in de proposito: o fingerprint
                            // e O(n log n), entao o `health` default nao paga.
                            let h = db.health();
                            let payload = json!({
                                "view": "index",
                                "index_fingerprint": format!("{:016x}", db.index_fingerprint()),
                                "doc_count": h.doc_count,
                                "bq_len": h.bq_len,
                                "indexed_embedding_dims": h.indexed_embedding_dims,
                                "open_rebuild_ms_last": h.open_rebuild_ms_last,
                                "open_rebuild_ms_max": h.open_rebuild_ms_max,
                                "opens": h.opens,
                            });
                            let text = serde_json::to_string_pretty(&payload).unwrap_or_default();
                            send(&json!({"jsonrpc":"2.0","id":id,"result":
                                mcp_tool_result(&text, payload, false)}));
                        } else if view == "staleness" {
                            let now = args["now"].as_u64().unwrap_or_else(|| {
                                std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_millis() as u64)
                                    .unwrap_or(0)
                            });
                            let limit = args["limit"].as_u64().unwrap_or(50) as usize;
                            let scope = args["scope"].as_str();
                            let cfg = neural_sgdb::StalenessConfig::default();
                            match db.staleness_report(now, limit, scope, &cfg) {
                                Ok(items) => {
                                    let payload = json!({
                                        "view": "staleness",
                                        "now": now,
                                        "limit": limit,
                                        "scope": scope,
                                        "count": items.len(),
                                        "items": items.iter().map(|h| json!({
                                            "key": h.key,
                                            "level": h.level.as_str(),
                                            "reasons": h.reasons.iter().map(|r| r.as_str()).collect::<Vec<_>>(),
                                            "age": h.age,
                                            "expires_at": h.expires_at,
                                            "recommendation": h.recommendation,
                                        })).collect::<Vec<_>>(),
                                        "note": "read-only — curate manual (expire_ttl/decay/forget/supersede); nao auto-forget"
                                    });
                                    let text = serde_json::to_string_pretty(&payload).unwrap_or_default();
                                    send(&json!({"jsonrpc":"2.0","id":id,"result":
                                        mcp_tool_result(&text, payload, false)}));
                                }
                                Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                            }
                        } else {
                            let payload = health_payload(&mut db, &db_path, &embedder_name);
                            let text = serde_json::to_string_pretty(&payload).unwrap_or_default();
                            send(&json!({"jsonrpc":"2.0","id":id,"result":
                                mcp_tool_result(&text, payload, false)}));
                        }
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
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
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
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
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
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "decay" => {
                        let now = args["now"].as_u64().unwrap_or(0);
                        let cfg = neural_sgdb::DecayConfig {
                            half_life_ms: args["half_life_ms"].as_u64().unwrap_or(30 * 24 * 3600 * 1000),
                            floor: args["floor"].as_f64().unwrap_or(0.05) as f32,
                            decay_state_at: args["decay_state_at"].as_f64().unwrap_or(0.05) as f32,
                            decay_confidence: args["decay_confidence"].as_bool().unwrap_or(true),
                        };
                        match db.decay_importance(now, &cfg) {
                            Ok(n) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("{n} memorias com importancia decaida (now={now}, half_life={}ms)", cfg.half_life_ms)}],"isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "consolidate" => {
                        let cfg = neural_sgdb::ConsolidateConfig {
                            min_repeats: args["min_repeats"].as_u64().unwrap_or(3) as usize,
                            min_len: args["min_len"].as_u64().unwrap_or(24) as usize,
                            max_new: args["max_new"].as_u64().unwrap_or(64) as usize,
                        };
                        let filter = mcp_scope_filter(args);
                        let result = if filter.is_global_only() {
                            db.consolidate_recurrences(&cfg)
                        } else {
                            db.consolidate_recurrences_scoped(&cfg, &filter)
                        };
                        match result {
                            Ok(n) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("{n} episodios L2 consolidados em fatos L3 (min_repeats={})", cfg.min_repeats)}],"isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "audit_checkpoint" => {
                        let now = args["now"].as_u64().unwrap_or(0);
                        match db.audit_checkpoint(now) {
                            Ok(seq) => {
                                // ADR-0009 §3: checkpoint é o gancho natural de
                                // persistência do snapshot (política != off).
                                let snap_note = if snapshot_policy != IndexSnapshotPolicy::Off {
                                    match db.persist_index_snapshot(now) {
                                        Ok(()) => " + snapshot idx persistido",
                                        Err(e) => {
                                            eprintln!("[neural-sgdb] snapshot persist falhou: {e}");
                                            ""
                                        }
                                    }
                                } else {
                                    ""
                                };
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":format!("checkpoint seq={seq} anexado ao ledger de auditoria{snap_note}")}],"isError":false}}))
                            }
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "audit_verify" => {
                        match db.audit_verify() {
                            Ok(r) => {
                                let verdict = if !r.chain_intact {
                                    "CHAIN QUEBRADA (tamper ou corrupcao de elo)"
                                } else if !r.digest_matches_last {
                                    "chain intacta; estado diverge do ultimo checkpoint (escritas pos-checkpoint)"
                                } else {
                                    "chain intacta; estado == ultimo checkpoint"
                                };
                                let text = format!(
                                    "auditoria: {} elo(s), last_seq={:?} â€” {verdict}",
                                    r.entries, r.last_seq
                                );
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":text}],"isError":false,
                                    "structuredContent": json!({
                                        "entries": r.entries, "chain_intact": r.chain_intact,
                                        "digest_matches_last": r.digest_matches_last, "last_seq": r.last_seq
                                    })}}));
                            }
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "rollback_to" => {
                        let seq = match args["seq"].as_u64() {
                            Some(s) => s,
                            None => {
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":"rollback_to exige seq=<checkpoint>"}],"isError":true}}));
                                return;
                            }
                        };
                        match db.rollback_to(seq) {
                            Ok(n) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("rollback para seq={seq}: {n} metadados restaurados (payloads intocados — ADD-only)")}],"isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "set_ttl" => {
                        let key = args["key"].as_str().unwrap_or("");
                        let now = args["now"].as_u64().unwrap_or(0);
                        // expires_at absoluto ou relativo? contrato: now + ttl relativo via "amount"? usa "now" como expires_at direto
                        let exp = args["seq"].as_u64().or(args["now"].as_u64()).unwrap_or(0);
                        let _ = now;
                        match db.set_ttl(key, exp) {
                            Ok(()) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("ttl {key} -> {exp}")}],"isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "expire_ttl" => {
                        let now = args["now"].as_u64().unwrap_or_else(|| {
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0)
                        });
                        match db.expire_ttl(now) {
                            Ok(n) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("{n} TTLs expirados em now={now}")}],"isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "set_event" => {
                        let key = args["key"].as_str().unwrap_or("");
                        let state = args["target"].as_str().unwrap_or(args["new"].as_str().unwrap_or(""));
                        let end = args["now"].as_u64().unwrap_or(0);
                        match db.set_event(key, state, end) {
                            Ok(()) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("evento {key} -> {state} end={end}")}],"isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "close_event" => {
                        let key = args["key"].as_str().unwrap_or("");
                        let now = args["now"].as_u64().unwrap_or(0);
                        match db.close_event(key, now) {
                            Ok(()) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("evento {key} fechado em {now}")}],"isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "timeline" => {
                        let state = args["target"].as_str().unwrap_or(args["key"].as_str().unwrap_or(""));
                        let limit = args["limit"].as_u64().unwrap_or(20) as usize;
                        match db.recall_timeline(state, limit) {
                            Ok(tl) => {
                                let text = tl.iter().map(|(k, w)| format!("- {k} valid={w:?}")).collect::<Vec<_>>().join("\n");
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":if text.is_empty() { "(vazio)".into() } else { text }}],"isError":false}}))
                            }
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "gc" => {
                        let now = args["now"].as_u64().unwrap_or_else(|| {
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0)
                        });
                        let cfg = neural_sgdb::GcConfig {
                            collect_decayed: args["decay_confidence"].as_bool().unwrap_or(false),
                            collect_archived: true,
                            min_age_ticks: args["seq"].as_u64().unwrap_or(0),
                            max_per_pass: args["limit"].as_u64().unwrap_or(64) as usize,
                        };
                        match db.collect_garbage(now, &cfg) {
                            Ok(r) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":format!("gc: invalidated={} ttl={} state={}", r.invalidated, r.ttl_collected, r.state_collected)}],"isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "recall_ann" => {
                        let k = args["limit"].as_u64().unwrap_or(5) as usize;
                        let emb = match args["embedding"].as_array() {
                            Some(a) => a.iter().filter_map(|v| v.as_f64().map(|x| x as f32)).collect::<Vec<_>>(),
                            None => Vec::new(),
                        };
                        match db.recall_ann_ivf(&emb, k, 0, 1) {
                            Ok(hits) => {
                                let text = hits.iter().map(|h| format!("- {} | {} (d={:.3})", h.key, h.text, h.dist)).collect::<Vec<_>>().join("\n");
                                send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                    "content":[{"type":"text","text":if text.is_empty() { "(vazio)".into() } else { text }}],"isError":false}}))
                            }
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "commit_run" => {
                        match mcp_commit_run(&mut db, args) {
                            Ok((text, structured)) => send(&json!({"jsonrpc":"2.0","id":id,"result":
                                mcp_tool_result(&text, structured, false)})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":e}],"isError":true}})),
                        }
                    }
                    "deprecate_run" => {
                        let filter = mcp_scope_filter(args);
                        let archive = args["archive_episodic"].as_bool().unwrap_or(true);
                        let now = args["now"].as_u64().unwrap_or(0);
                        let ttl = args["ttl_episodic_ms"].as_u64().map(|ms| now.saturating_add(ms));
                        match db.deprecate_run(&filter, archive, ttl) {
                            Ok(r) => {
                                let text = format!(
                                    "deprecate_run: archived={} ttl_set={}",
                                    r.archived, r.ttl_set
                                );
                                send(&json!({"jsonrpc":"2.0","id":id,"result":
                                    mcp_tool_result(&text, json!({
                                        "archived": r.archived,
                                        "ttl_set": r.ttl_set
                                    }), false)}));
                            }
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "validate" => {
                        // v1.1.28 (D3): gêmeo TIPADO — códigos estáveis e campos
                        // nomeados; a prosa continua para o consumidor humano.
                        let issues = db.validate();
                        let structured = json!({
                            "healthy": issues.is_empty(),
                            "issue_count": issues.len(),
                            "issues": issues.iter().map(|i| json!({
                                "key": i.key,
                                "message": i.message,
                            })).collect::<Vec<_>>(),
                        });
                        let text = if issues.is_empty() {
                            "banco saudavel (nenhum issue de integridade)".into()
                        } else {
                            issues.iter().map(|i| format!("[{}] {}", i.key, i.message))
                                .collect::<Vec<_>>().join("\n")
                        };
                        send(&json!({"jsonrpc":"2.0","id":id,"result":
                            mcp_tool_result(&text, structured, false)}));
                    }
                    // v1.1.28 (ADR-0017, Movimento 2): 𝒥(S,𝒬) — decisões de
                    // controle em LOTE com espaços de resposta FECHADOS, num
                    // round trip. Determinístico: o core responde com sinais
                    // e probabilidades, nunca gera texto.
                    "decide" => {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        let mut answers: Vec<Value> = Vec::new();
                        let questions = args["questions"].as_array().cloned().unwrap_or_default();
                        if questions.is_empty() {
                            send(&error_response(&id, -32602,
                                "decide requer questions[] (perguntas com espaços fechados)"));
                            continue;
                        }
                        'outer: for (qi, q) in questions.iter().enumerate() {
                            let ask = q["ask"].as_str().unwrap_or("");
                            let fail = |m: &str| -> Value {
                                json!({"index": qi, "ask": ask, "error": m})
                            };
                            let ans = match ask {
                                // Suficiência de evidência (Eq. 21 do paper): o
                                // probe do ADR-0012 vira resposta tipada, e o
                                // stop é ENRIQUECIDO com os dois sinais que o
                                // paper exige e o core já tem: c_d = contradição
                                // não-resolvida (conflicts open) e m_d = evidência
                                // obrigatória faltando (keys exigidas que não
                                // vieram nos hits).
                                "evidence_sufficient" => {
                                    let query = q["query"].as_str().unwrap_or("");
                                    if query.is_empty() {
                                        answers.push(fail("query obrigatoria")); continue 'outer;
                                    }
                                    match db.recall_adaptive_lexical(query, 5) {
                                        Ok(ar) => {
                                            let top_keys: Vec<String> = ar.hits.iter().take(3).map(|h| h.key.clone()).collect();
                                            // c_d: contradição não-resolvida no corpus
                                            let unresolved: usize = db.conflicts().iter()
                                                .filter(|c| matches!(c.status, neural_sgdb::ConflictStatus::Open))
                                                .count();
                                            // m_d: evidência OBRIGATÓRIA faltando — o
                                            // caller lista keys que PRECISAM estar no
                                            // resultado (ex.: a memory do turno anterior).
                                            let required: Vec<String> = q["required_keys"].as_array()
                                                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                                                .unwrap_or_default();
                                            let missing: Vec<String> = required.iter()
                                                .filter(|rk| {
                                                    let canon = db.resolve_known_key(rk);
                                                    !top_keys.contains(&canon)
                                                        && !ar.hits.iter().any(|h| h.key == canon)
                                                })
                                                .cloned()
                                                .collect();
                                            // Eq. 21: s_d ≥ θ ∧ m_d < θ_cont ∧ c_d < θ_cont
                                            let sufficient = ar.boundary_decisive
                                                && missing.is_empty()
                                                && unresolved == 0;
                                            json!({
                                                "index": qi, "ask": ask,
                                                "answer": {
                                                    "sufficient": sufficient,
                                                    "boundary_decisive": ar.boundary_decisive,
                                                    "unresolved_contradictions": unresolved,
                                                    "missing_required": missing,
                                                    "hits": ar.hits.len(),
                                                    "escalations": ar.escalations,
                                                    "oversample_used": ar.oversample_used,
                                                    "top_keys": top_keys,
                                                }
                                            })
                                        }
                                        Err(e) => fail(&mcp_actionable_error(e)),
                                    }
                                }
                                // Relação temporal — vocabulário COMPLETO de 7
                                // valores do paper (v1.1.28.1): os timestamps de
                                // criação dão before/after/same_time; as janelas
                                // de validade bi-temporal dão during/contains/
                                // overlaps; sem informação → unknown.
                                "temporal_relation" => {
                                    let a_key = q["a_key"].as_str().unwrap_or("");
                                    let b_key = q["b_key"].as_str().unwrap_or("");
                                    let (a, b, wa, wb) = if !a_key.is_empty() && !b_key.is_empty() {
                                        // por KEY: resolve e lê janelas reais
                                        let wa = db.validity_window_of(a_key);
                                        let wb = db.validity_window_of(b_key);
                                        match (db.created_tick_of(a_key), db.created_tick_of(b_key)) {
                                            (Some(x), Some(y)) => (x, y, wa, wb),
                                            _ => {
                                                answers.push(fail("key sem meta (created_tick desconhecido)")); continue 'outer;
                                            }
                                        }
                                    } else {
                                        let a = q["a_created"].as_u64().unwrap_or(0);
                                        let b = q["b_created"].as_u64().unwrap_or(0);
                                        if a == 0 || b == 0 {
                                            answers.push(fail("a_created e b_created obrigatorios (ou a_key+b_key)")); continue 'outer;
                                        }
                                        (a, b, None, None)
                                    };
                                    // pontos: before/after/same_time (criação)
                                    let point_rel = if a == b { "same_time" } else if a < b { "before" } else { "after" };
                                    // intervalos: only com janelas de validade
                                    let rel = match (wa, wb) {
                                        (Some((fa, ua)), Some((fb, ub))) => {
                                            if fa >= fb && ua <= ub { "during" }             // A dentro de B
                                            else if fa <= fb && ua >= ub { "contains" }      // A contém B
                                            else if fa < ub && fb < ua { "overlaps" }        // interseção sem contenção
                                            else { point_rel }
                                        }
                                        _ => point_rel,
                                    };
                                    json!({"index": qi, "ask": ask, "answer": {"relation": rel}})
                                }
                                // Vigência bi-temporal de um hit no instante `at`.
                                "valid_at" => {
                                    let key = q["key"].as_str().unwrap_or("");
                                    let at = q["at"].as_u64().unwrap_or(now);
                                    if key.is_empty() {
                                        answers.push(fail("key obrigatoria")); continue 'outer;
                                    }
                                    let valid = match db.validity_window_of(key) {
                                        Some((f, u)) => f <= at && at < u,
                                        None => true, // sem janela = sempre válido
                                    };
                                    json!({"index": qi, "ask": ask, "answer": {"valid": valid}})
                                }
                                // Relevância relativa: candidatos com sinais
                                // decompostos (Eq. 8/23) — o controlador pondera.
                                "candidate_relevance" => {
                                    let query = q["query"].as_str().unwrap_or("");
                                    if query.is_empty() {
                                        answers.push(fail("query obrigatoria")); continue 'outer;
                                    }
                                    let ents: Vec<String> = q["entities"].as_array()
                                        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                                        .unwrap_or_default();
                                    let ent_refs: Vec<&str> = ents.iter().map(|s| s.as_str()).collect();
                                    let at = q["at"].as_u64().unwrap_or(now);
                                    match db.recall_candidates(&[], query, &ent_refs, 5, at) {
                                        Ok(cands) => json!({
                                            "index": qi, "ask": ask,
                                            "answer": {
                                                "candidates": cands.iter().map(|c| json!({
                                                    "key": c.key,
                                                    "sim_vec": c.sim_vec,
                                                    "lex_overlap": c.lex_overlap,
                                                    "shared_entities": c.shared_entities,
                                                    "recency": c.recency,
                                                    "rrf": c.rrf,
                                                })).collect::<Vec<_>>(),
                                            }
                                        }),
                                        Err(e) => fail(&mcp_actionable_error(e)),
                                    }
                                }
                                other => fail(&format!(
                                    "ask desconhecida: {other} (validas: evidence_sufficient, temporal_relation, valid_at, candidate_relevance)")),
                            };
                            answers.push(ans);
                        }
                        let text = format!("decide: {} respostas", answers.len());
                        send(&json!({"jsonrpc":"2.0","id":id,"result":
                            mcp_tool_result(&text, json!({"answers": answers}), false)}));
                    }
                    // v1.1.28 (Movimento 3): prefetch de candidatos com sinais
                    // decompostos — o write path do paper, direto no MCP.
                    "recall_candidates" => {
                        let query = args["query"].as_str().unwrap_or("");
                        let ents: Vec<String> = args["entities"].as_array()
                            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                            .unwrap_or_default();
                        let ent_refs: Vec<&str> = ents.iter().map(|s| s.as_str()).collect();
                        let at = args["at"].as_u64().unwrap_or_else(|| {
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0)
                        });
                        match db.recall_candidates(&[], query, &ent_refs, 8, at) {
                            Ok(cands) => {
                                let structured = json!({ "candidates": cands.iter().map(|c| json!({
                                    "key": c.key, "text": c.text,
                                    "sim_vec": c.sim_vec, "lex_overlap": c.lex_overlap,
                                    "shared_entities": c.shared_entities,
                                    "validity": c.validity.map(|(f,u)| json!([f,u])),
                                    "recency": c.recency, "rrf": c.rrf,
                                })).collect::<Vec<_>>() });
                                let text = if cands.is_empty() {
                                    "nenhum candidato".into()
                                } else {
                                    cands.iter().map(|c| format!("- {} | lex={:.2} ents={} rrf={:.4}",
                                        c.key, c.lex_overlap, c.shared_entities.join(","), c.rrf))
                                        .collect::<Vec<_>>().join("\n")
                                };
                                send(&json!({"jsonrpc":"2.0","id":id,"result":
                                    mcp_tool_result(&text, structured, false)}));
                            }
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    "era_report" => {
                        match db.era_report_lines() {
                            Ok(lines) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":lines.join("\n")}],"isError":false}})),
                            Err(e) => send(&json!({"jsonrpc":"2.0","id":id,"result":{
                                "content":[{"type":"text","text":mcp_actionable_error(e)}],"isError":true}})),
                        }
                    }
                    _ => send(&unknown_tool_error(&id, &name)),
                }
            }
            "" => {
                // notificaÃ§Ã£o sem method vÃ¡lido / malformada
                send(&error_response(&id, -32600, "Invalid request"));
            }
            _ => {
                // -32601 em server/discover â†’ client moderno faz fallback p/ initialize
                send(&error_response(&id, -32601, "Method not found"));
            }
        }
    }
    // EOF no stdin = shutdown
    eprintln!("[neural-sgdb] stdin fechado â€” encerrando");
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
        assert_eq!(c3, None, "Ãºltima pÃ¡gina nÃ£o tem nextCursor");
        // cursor invÃ¡lido â†’ volta ao inÃ­cio
        let (p, _) = paginate(&items, Some("xyz"), 2);
        assert_eq!(p, vec![0, 1]);
    }

    #[test]
    fn paginate_hostile_size_does_not_panic() {
        let items: Vec<u32> = (0..10).collect();
        // pageSize hostil (u64::MAX â†’ usize::MAX) com cursor â‰¥ 1 nÃ£o pode
        // estourar `off + size` nem alocar alÃ©m dos itens (regressÃ£o P0-8).
        let (p, next) = paginate(&items, Some("1"), usize::MAX);
        assert_eq!(p.len(), items.len() - 1, "clamp: pÃ¡gina limitada aos itens");
        assert_eq!(next, None, "cursor 1 + tudo â†’ nÃ£o hÃ¡ next");
        // cursor no fim + size hostil
        let (p, next) = paginate(&items, Some("9"), usize::MAX);
        assert_eq!(p, vec![9]);
        assert_eq!(next, None);
        // tamanho acima do clamp nÃ£o muda semÃ¢ntica legÃ­tima (page 0)
        let (p, next) = paginate(&items, None, 2000);
        assert_eq!(p, items);
        assert_eq!(next, None);
    }

    #[test]
    fn lazy_recall_pages_match_full_topk() {
        // v1.1.3 S5: a paginaÃ§Ã£o lazy busca `off+size` hits em vez de top-100
        // fixo. O contrato Ã© que a pÃ¡gina do prefixo lazy == a pÃ¡gina do
        // top-k completo (recall Ã© determinÃ­stico por (score, key) â€” top-(n+1)
        // Ã© um prefixo de top-N). Pina o invariante contra regressÃ£o futura.
        let mut db = neural_sgdb::Sgdb::open(neural_sgdb::InMemory::new()).unwrap();
        let mut texts = Vec::new();
        for i in 0..12 {
            let t = format!("memoria de teste numero {:02} {}", i, "overlap comum");
            texts.push(t.clone());
            db.remember_semantic(&format!("k{:02}", i), &t, &[1.0, -1.0, 1.0, -1.0]).unwrap();
        }
        let q = [1.0, -1.0, 1.0, -1.0];
        // "top-k completo" = o teto antigo (100); lazy = off+size+1 por pÃ¡gina
        // (a sentinela +1 sÃ³ sonda a prÃ³xima pÃ¡gina â€” nÃ£o muda o conteÃºdo).
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
            assert!(!page.is_empty(), "pÃ¡gina vazia antes do fim do conjunto");
            // cada pÃ¡gina lazy == fatia do top-k completo (determinismo)
            for (i, h) in page.iter().enumerate() {
                assert_eq!(h.key, full[off + i].key, "pÃ¡gina lazy divergiu do top-k");
            }
            collected.extend(page.into_iter().map(|h| h.key));
            cursor = next;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(cursor, None, "deve ter iterado o conjunto inteiro");
        assert_eq!(collected.len(), 12);
        // sem duplicatas entre pÃ¡ginas
        let uniq: std::collections::HashSet<_> = collected.iter().collect();
        assert_eq!(uniq.len(), 12, "paginaÃ§Ã£o repetiu hits");
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
            expand_tool("health", &serde_json::json!({"view":"tensions"})),
            "health"
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

    /// A superficie de alias e uma TABELA, nao prosa. Antes isto era "os 23
    /// nomes antigos" num comentario — e a contagem ja estava velha (o
    /// rework v1.1.8 nao conhecia as ops cognitivas nem o harness).
    #[test]
    fn alias_surface_is_consistent() {
        // sem duplicatas
        let mut seen: Vec<&str> = ALIAS_SURFACE.to_vec();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), before, "alias duplicado em ALIAS_SURFACE");
        // as 4 tools LISTADAS nao podem aparecer como alias
        for t in LISTED_TOOLS {
            assert!(
                !ALIAS_SURFACE.contains(t),
                "'{t}' e listada — nao pode estar na superficie de alias"
            );
        }
        // nomes de tool sao snake_case minusculo
        for t in ALIAS_SURFACE {
            assert!(
                t.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "'{t}' fora do padrao snake_case"
            );
        }
        // tripwire: a superficie derivada do dispatch tem 34 nomes. Se um arm
        // novo for adicionado sem entrar aqui, o `did_you_mean` fica cego.
        assert_eq!(
        ALIAS_SURFACE.len(),
        38,
        "superficie de alias mudou (v1.1.24: +4 do ledger de negativos)"
    );
        assert_eq!(LISTED_TOOLS.len(), EXPECTED_MCP_TOOL_COUNT);
    }

    #[test]
    fn did_you_mean_suggests_prefix_and_typos() {
        assert!(
            did_you_mean("recal").first().is_some_and(|s| s == "recall"),
            "prefixo deve sugerir a tool canonica"
        );
        assert!(
            did_you_mean("remembr").iter().any(|s| s == "remember"),
            "typo de 1 char deve ser corrigido"
        );
        assert!(
            did_you_mean("remember_episodic").iter().any(|s| s == "remember_episodic"),
            "nome legado real e sugerido"
        );
        assert!(
            did_you_mean("qqzzqqzz").is_empty(),
            "nome sem nada perto nao inventa sugestao"
        );
        // determinismo: o mesmo nome devolve sempre o mesmo (retry previsivel)
        assert_eq!(did_you_mean("recal"), did_you_mean("recal"));
        assert!(did_you_mean("recal").len() <= 3, "no maximo 3 sugestoes");
    }

    #[test]
    fn edit_distance_basics() {
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("abc", "abc"), 0);
        assert_eq!(edit_distance("abc", "abd"), 1);
        assert_eq!(edit_distance("abc", ""), 3);
        assert_eq!(edit_distance("recall", "recal"), 1);
    }

    /// O erro enriquecido CONTINUA sendo um erro (`-32602`), so ganha `data`:
    /// mudar para sucesso seria quebrar o contrato de quem detecta falha.
    #[test]
    fn unknown_tool_error_stays_an_error_and_carries_hints() {
        let v = unknown_tool_error(&serde_json::json!(1), "recal");
        assert_eq!(v["error"]["code"], -32602, "o codigo de erro nao muda");
        assert_eq!(v["error"]["message"], "Unknown tool");
        assert_eq!(v["error"]["data"]["tool"], "recal");
        assert_eq!(v["error"]["data"]["alias_count"], 38);
        assert_eq!(
            v["error"]["data"]["listed_tools"].as_array().map(|a| a.len()),
            Some(4)
        );
        let hints = v["error"]["data"]["did_you_mean"].as_array().unwrap();
        assert!(hints.iter().any(|h| h == "recall"), "{v}");
    }

    /// `health(view=index)` expoe o oraculo (ADR-0011) sem pagar o custo no
    /// `health` default — e o `expand_tool` nao o desvia para outra tool.
    #[test]
    fn health_view_index_is_reachable_and_not_remapped() {
        assert_eq!(
            expand_tool("health", &serde_json::json!({"view":"index"})),
            "health"
        );
        let mut db = neural_sgdb::Sgdb::open(neural_sgdb::InMemory::new()).unwrap();
        db.remember_text_with(
            "idx/a",
            "corpo do doc de indice",
            neural_sgdb::RememberOptions::default(),
        )
        .unwrap();
        let h = db.health();
        assert_eq!(h.open_rebuild_ms_max >= h.open_rebuild_ms_last, true);
        assert!(h.opens >= 1);
        assert_ne!(db.index_fingerprint(), 0, "fingerprint de corpus nao-trivial");
    }

    #[test]
    fn mcp_actionable_error_hints_era_report() {
        let msg = mcp_actionable_error("Invalid: query dims not in indexed_embedding_dims()");
        assert!(msg.contains("era_report"), "{msg}");
    }

    #[test]
    fn adr0008_recall_default_is_lexical() {
        let lexical = resolve_retrieval_mode(&serde_json::json!({"query": "x"}), false).unwrap();
        assert_eq!(lexical, "lexical");
        let with_vec = resolve_retrieval_mode(
            &serde_json::json!({"query": "x", "embedding": [1.0, -1.0]}),
            false,
        )
        .unwrap();
        assert_eq!(with_vec, "semantic");
        let denied = resolve_retrieval_mode(
            &serde_json::json!({"query": "x", "mode": "semantic"}),
            false,
        );
        assert!(denied.unwrap_err().contains("ADR-0008"));
        let demo_host = resolve_retrieval_mode(
            &serde_json::json!({"query": "x", "mode": "hybrid"}),
            true,
        )
        .unwrap();
        assert_eq!(demo_host, "hybrid");
    }
}
