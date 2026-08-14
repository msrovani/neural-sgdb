//! v1.1.3 S2 — pluga o trait `Embedder` num endpoint HTTP real, de ponta a
//! ponta, sem adicionar dependência ao crate (raw HTTP/1.1 via `std::net` +
//! `serde_json`, já dev-dep).
//!
//! Run: `cargo run --release --example embedder_http`
//!
//! Dois atores:
//!   1. `MockEmbedServer` — mini-servidor HTTP (thread local) que responde
//!      `POST /embed {"text": "..."}` → `{"embedding": [8 floats]}`. É o
//!      stand-in de um modelo real (BGE/OpenAI/ONNX) atrás de uma porta HTTP.
//!   2. `HttpEmbedder` — implementa `neural_sgdb::Embedder`: serializa o
//!      texto, faz o POST, valida a resposta e devolve `Vec<f32>`.
//!
//! O exemplo então: grava memórias com o `HttpEmbedder`, faz recall com o
//! MESMO modelo (contrato P4 — gravação e busca no mesmo espaço de 8 dims) e
//! demonstra o guard S1: uma query do `DemoEmbedder` (256 dims) contra o
//! corpus de 8 dims devolve `SgdbError::Invalid` em vez de ruído de hamming.
//!
//! Exit code 0 iff all checks pass.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use neural_sgdb::{Embedder, InMemory, Sgdb, SgdbError};

const MOCK_DIM: usize = 8;

/// Embedding determinístico do mock — substitui o modelo real. 8 dims, de
/// propósito diferente do demo (256) para provar que o caminho HTTP é usado.
fn mock_embed(text: &str) -> Vec<f32> {
    let mut v = vec![0f32; MOCK_DIM];
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in text.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
        let idx = (h % MOCK_DIM as u64) as usize;
        v[idx] += if (h >> 8) & 1 == 1 { 1.0 } else { -1.0 };
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().max(1e-8).sqrt();
    v.iter_mut().for_each(|x| *x /= norm);
    v
}

/// Servidor HTTP de embeddings (thread local). Conta requests recebidos para
/// o exemplo provar que o client realmente passou pela rede.
fn spawn_mock_server() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
    let addr = listener.local_addr().expect("local addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_c = Arc::clone(&hits);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            hits_c.fetch_add(1, Ordering::SeqCst);
            handle_conn(stream);
        }
    });
    (format!("http://127.0.0.1:{}", addr.port()), hits)
}

fn handle_conn(mut stream: TcpStream) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // lê até ter headers + body completo (Content-Length), então responde —
    // não espera EOF (o client pode manter a conexão aberta até ler a resposta)
    loop {
        if let Some(body) = body_from(&buf) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
                let text = v["text"].as_str().unwrap_or("");
                let emb = mock_embed(text);
                let resp = serde_json::json!({ "embedding": emb }).to_string();
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", resp.len(), resp);
                let _ = stream.flush();
                return;
            }
        }
        if buf.len() > 16_384 {
            return;
        }
        match stream.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => return,
        }
    }
}

/// Extrai o corpo JSON de uma mensagem HTTP/1.1 (`Content-Length`).
fn body_from(buf: &[u8]) -> Option<&str> {
    let head_end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&buf[..head_end]).ok()?;
    let len = head
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length:").or_else(|| l.strip_prefix("content-length:")))
        ?.trim()
        .parse::<usize>()
        .ok()?;
    let body_start = head_end + 4;
    if buf.len() >= body_start + len {
        Some(std::str::from_utf8(&buf[body_start..body_start + len]).ok()?)
    } else {
        None
    }
}

/// Embedder via HTTP/1.1 cru — o ponto do exemplo. A implementação real num
/// produto usaria um client TLS (reqwest/ureq); aqui provamos o CONTRATO do
/// trait contra um endpoint HTTP de verdade.
struct HttpEmbedder {
    base: String,
}

impl Embedder for HttpEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, SgdbError> {
        let body = serde_json::json!({ "text": text }).to_string();
        // parse http://host:port
        let rest = self
            .base
            .strip_prefix("http://")
            .ok_or(SgdbError::Invalid("HttpEmbedder: base deve ser http://host:port"))?;
        let (host, port) = rest
            .rsplit_once(':')
            .ok_or(SgdbError::Invalid("HttpEmbedder: base deve ser http://host:port"))?;
        let port: u16 = port
            .parse()
            .map_err(|_| SgdbError::Invalid("HttpEmbedder: porta inválida"))?;
        let mut stream = TcpStream::connect((host, port))
            .map_err(|_| SgdbError::Storage("http embed connect"))?;
        let req = format!(
            "POST /embed HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(req.as_bytes())
            .map_err(|_| SgdbError::Storage("http embed write"))?;
        stream
            .flush()
            .map_err(|_| SgdbError::Storage("http embed flush"))?;
        // sinaliza fim do request — o server pode responder sem esperar EOF
        let _ = stream.shutdown(std::net::Shutdown::Write);
        let mut resp = Vec::new();
        let mut chunk = [0u8; 4096];
        // lê até Connection: close do server (ou um limite de segurança)
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    resp.extend_from_slice(&chunk[..n]);
                    if resp.len() > 65_536 {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let body_str = body_from(&resp)
            .ok_or(SgdbError::Invalid("HttpEmbedder: resposta sem Content-Length"))?;
        let v: serde_json::Value = serde_json::from_str(body_str)
            .map_err(|_| SgdbError::Invalid("HttpEmbedder: resposta não-JSON"))?;
        let arr = v["embedding"]
            .as_array()
            .ok_or(SgdbError::Invalid("HttpEmbedder: sem campo embedding"))?;
        if arr.is_empty() || arr.len() > neural_sgdb::MAX_EMBEDDING_DIM {
            return Err(SgdbError::Invalid("HttpEmbedder: dimensionalidade fora do contrato"));
        }
        let mut out = Vec::with_capacity(arr.len());
        for x in arr {
            let f = x.as_f64().ok_or(SgdbError::Invalid("HttpEmbedder: embedding não numérico"))? as f32;
            if !f.is_finite() {
                return Err(SgdbError::Invalid("HttpEmbedder: embedding não-finito"));
            }
            out.push(f);
        }
        Ok(out)
    }
}

fn main() {
    let (base, hits) = spawn_mock_server();
    let embedder = HttpEmbedder { base };
    let mut db = Sgdb::open(InMemory::new()).expect("open");
    let mut checks = 0usize;
    let mut fails = 0usize;
    let mut check = |name: &str, ok: bool, detail: &str| {
        checks += 1;
        if !ok {
            fails += 1;
        }
        println!("{} {}", if ok { "PASS" } else { "FAIL" }, name);
        if !ok {
            println!("     {detail}");
        }
    };

    // grava com o HttpEmbedder (8 dims)
    let e1 = embedder.embed("integridade do banco").expect("embed 1");
    assert_eq!(e1.len(), MOCK_DIM, "HttpEmbedder deve devolver a dim do mock");
    db.remember_semantic("k1", "integridade do banco de dados", &e1).expect("remember 1");
    let e2 = embedder.embed("clippy zero warnings").expect("embed 2");
    db.remember_semantic("k2", "clippy zero warnings no CI", &e2).expect("remember 2");

    // recall com o MESMO modelo → acha (contrato P4)
    let q = embedder.embed("integridade do banco").expect("query 1");
    let r = db.recall(&q, 3).expect("recall");
    check(
        "recall HTTP acha o doc do mesmo modelo",
        r.iter().any(|h| h.key.contains("k1")),
        &format!("hits: {}", r.iter().map(|h| h.key.clone()).collect::<Vec<_>>().join(", ")),
    );
    check(
        "indexed_embedding_dims reflete o modelo HTTP (8 dims)",
        db.indexed_embedding_dims() == vec![MOCK_DIM],
        &format!("{:?}", db.indexed_embedding_dims()),
    );

    // guard S1: query do DemoEmbedder (256 dims) contra corpus 8 dims → erro
    let demo_q = neural_sgdb::demo_embed("integridade do banco");
    let err = db.recall(&demo_q, 3).unwrap_err();
    check(
        "S1: query de outra dimensionalidade avisa em vez de silenciar",
        matches!(err, SgdbError::Invalid(_)),
        &format!("{err:?}"),
    );

    // prova de que o caminho HTTP foi usado (requests de verdade na rede)
    // 2 remember + 1 query = 3; se fosse o DemoEmbedder local, seria 0
    check(
        "HttpEmbedder passou pela rede (mock recebeu requests)",
        hits.load(Ordering::SeqCst) >= 3,
        &format!("requests: {}", hits.load(Ordering::SeqCst)),
    );

    println!("\nembedder_http: {checks} checks, {fails} failures");
    std::process::exit(if fails == 0 { 0 } else { 1 });
}