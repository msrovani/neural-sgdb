//! Protocolo entre DUAS inteligências (v1.1.6 item 5) — o contrato completo
//! de troca de dados máquina→máquina pelo neural-sgdb.
//!
//! O retorno de recall é lido por OUTRA IA (não por humano) — e o canal
//! carrega datums que NÃO são palavras humanas: JSON de intenção, embeddings
//! da era, binários. Este exemplo prova, de ponta a ponta, que o consumidor
//! com MENOS contexto não precisa re-tokenizar a prosa `- key | text` nem
//! adivinhar o schema:
//!
//! - **IA-A (o WRITER) DECLARA o tipo do datum** (`set_content_type`, MDM1
//!   v6 — item 2): `type="json"` num payload que o detector acharia Text
//!   (`"42"` não é `{…}`/`[…]`), `type="binary"` num blob não-UTF8,
//!   `type="embedding"` num vetor da era. Quem fornece declara (mesmo
//!   contrato de `entities`/`Embedder`).
//! - **IA-B (o READER) consome os HITS TIPADOS** (itens 1/3/4): lê
//!   `content_type` (declarado vence o detector), `payload_type` (o datum
//!   REAL do primário — Embedding(dim) para L4/L5, mesmo com companion Text),
//!   `rel` (companion `/L2/` → primário `/L4|L5|L3/`) e `matched_terms` (o
//!   "porquê" do casamento). Nunca `from_utf8_lossy` em datum Embedding/Binary.
//! - A **regra de ouro** vale para os dois lados: o vetor é re-usado com o
//!   MESMO modelo que o gravou (era ADR-0007); JSON é parseado VERBATIM; o
//!   binário é consumido cru pela key.
//!
//! Uso:
//! ```text
//! cargo run --release --example two_ai_protocol
//! ```
//! Exit code 0 sse todas as asserções passaram. Determinístico (InMemory, sem
//! LLM) — o embedding é um hash LCG do exemplo (mesmo modelo na escrita e na
//! busca, como manda o contrato).
//!
//! Seção 1 — AI-A GRAVA (declaração no write): 4 datums + rejeição de rótulo.
//! Seção 2 — AI-B LÊ (tipagem no recall): semântico, entidades, lexical com
//! rel, embedding da era.
//! Seção 3 — AI-B usa: parseia JSON verbatim, segue rel, consome o binário cru.

use neural_sgdb::{ContentType, InMemory, MemoryLayer, MemoryDoc, Sgdb};

// ── reporter minimal (PASS/FAIL) ────────────────────────────────────────────
struct Rep {
    checks: Vec<(String, bool, String)>,
}
impl Rep {
    fn new() -> Rep { Rep { checks: Vec::new() } }
    fn check(&mut self, name: &str, ok: bool, detail: String) {
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

/// Embedding determinístico do exemplo — o MESMO modelo na escrita e na busca
/// (contrato P4/ADR-0007: dimensionalidade identifica a era).
fn emb(seed: u64) -> Vec<f32> {
    let mut s = seed.wrapping_mul(1103515245).wrapping_add(12345);
    let mut v = Vec::with_capacity(8);
    for _ in 0..8 {
        s = s.wrapping_mul(1103515245).wrapping_add(12345);
        v.push(((s >> 32) as i32 % 200) as f32 / 100.0 - 1.0);
    }
    v
}

/// IA-B: "parsing" barato do datum — o `content_type` diz COMO, verbatim.
fn parse_json(datum: &str) -> bool {
    let t = datum.trim();
    (t.starts_with('{') && t.ends_with('}')) || (t.starts_with('[') && t.ends_with(']'))
}

fn main() {
    let mut db = Sgdb::open(InMemory::new()).unwrap();
    let mut rep = Rep::new();

    // ── Seção 1: IA-A GRAVA (declaração de tipo no WRITE — item 2) ──────────
    let emb_intent = emb(0xA11);
    let emb_num = emb(0xB22);
    let emb_vec = emb(0xC33);

    // 1. datum JSON de intenção máquina→máquina (delimitado — detector
    //    concordaria; a declaração é o CONTRATO, não uma muleta)
    db.remember_semantic(
        "intent/query_status",
        r#"{"intent":"query_status","target":"svc-42"}"#,
        &emb_intent,
    )
    .unwrap();
    let sk_intent = "md/L4/intent/query_status";
    db.set_content_type(sk_intent, "json").unwrap();
    rep.check(
        "AI-A: datum JSON de intenção declarado type=json",
        db.content_type_of(sk_intent).unwrap().as_deref() == Some("json"),
        format!("{:?}", db.content_type_of(sk_intent).unwrap()),
    );

    // 2. datum "42" — JSON NÚMERO válido, mas NÃO delimitado: o detector diria
    //    Text. O seam remove a adivinhação (o caso exato da saga type=Code).
    db.remember_semantic("json/numero", "42", &emb_num).unwrap();
    let sk_num = "md/L4/json/numero";
    db.set_content_type(sk_num, "json").unwrap();
    rep.check(
        "AI-A: datum '42' (JSON número) declarado type=json — detector diria Text",
        db.content_type_of(sk_num).unwrap().as_deref() == Some("json"),
        String::new(),
    );

    // 3. datum BINÁRIO cru (payload não-UTF8 em L3) + entidade p/ recuperar
    let blob: Vec<u8> = vec![0x00, 0xFF, 0x01, 0x02, 0xDE, 0xAD, 0xBE, 0xEF];
    db.put(MemoryDoc::new(MemoryLayer::L3EpisodicLong, "bin/checksum", blob.clone()))
        .unwrap();
    let sk_bin = "md/L3/bin/checksum";
    db.set_content_type(sk_bin, "binary").unwrap();
    db.set_entities(sk_bin, &["datum/checksum"]).unwrap();
    rep.check(
        "AI-A: datum binário não-UTF8 declarado type=binary + entidade datum/checksum",
        db.content_type_of(sk_bin).unwrap().as_deref() == Some("binary"),
        String::new(),
    );

    // 4. datum EMBEDDING da era (o vetor em si — text suprimido na projeção)
    db.remember_semantic("vec/era", "vetor da era neural-sgdb", &emb_vec).unwrap();
    let sk_vec = "md/L4/vec/era";
    db.set_content_type(sk_vec, "embedding").unwrap();
    rep.check(
        "AI-A: vetor da era declarado type=embedding",
        db.content_type_of(sk_vec).unwrap().as_deref() == Some("embedding"),
        String::new(),
    );

    // 5. rótulo desconhecido é rejeitado na escrita (nunca persiste lixo)
    rep.check(
        "AI-A: type=YAML rejeitado (rótulos estáveis são o contrato)",
        db.set_content_type(sk_intent, "YAML").is_err()
            && db.content_type_of(sk_intent).unwrap().as_deref() == Some("json"),
        String::new(),
    );

    // ── Seção 2: IA-B LÊ (hits TIPADOS no recall — itens 1/3/4) ─────────────
    let hits = db.recall(&emb_intent, 4).unwrap();
    let h_intent = hits.iter().find(|h| h.key == sk_intent).expect("hit da intenção");
    rep.check(
        "AI-B: recall semântico tipa o datum (content_type=Json, texto verbatim)",
        h_intent.content_type == ContentType::Json && h_intent.text.contains("query_status"),
        format!("{:?}", h_intent.content_type),
    );
    rep.check(
        "AI-B: payload_type=Embedding(8) — o datum REAL é o vetor (era ADR-0007)",
        h_intent.payload_type == ContentType::Embedding(8),
        format!("{:?}", h_intent.payload_type),
    );
    rep.check(
        "AI-B: hit de doc primário NÃO tem rel (rel é de companion /L2/)",
        h_intent.rel.is_none(),
        format!("{:?}", h_intent.rel),
    );

    let hits = db.recall(&emb_num, 4).unwrap();
    let h_num = hits.iter().find(|h| h.key == sk_num).expect("hit do '42'");
    rep.check(
        "AI-B: '42' tipa Json pela DECLARAÇÃO (seam vence o detector) e parseia verbatim",
        h_num.content_type == ContentType::Json
            && h_num.text == "42"
            && !parse_json("42") // JSON válido que o detector não pega
            && h_num.payload_type == ContentType::Embedding(8),
        format!("ct={:?} payload={:?}", h_num.content_type, h_num.payload_type),
    );

    let hits = db.recall_entities(&["datum/checksum"], 4).unwrap();
    let h_bin = hits.iter().find(|h| h.key == sk_bin).expect("hit do binário");
    rep.check(
        "AI-B: recall_entities expõe o binário como type=Binary com text VAZIO",
        h_bin.content_type == ContentType::Binary && h_bin.text.is_empty(),
        format!("ct={:?} text={:?}", h_bin.content_type, h_bin.text),
    );

    let hits = db.recall(&emb_vec, 4).unwrap();
    let h_vec = hits.iter().find(|h| h.key == sk_vec).expect("hit do vetor");
    rep.check(
        "AI-B: embedding declarado NUNCA vira prosa (text vazio, type=Embedding(8))",
        h_vec.content_type == ContentType::Embedding(8)
            && h_vec.text.is_empty()
            && h_vec.payload_type == ContentType::Embedding(8),
        format!("ct={:?}", h_vec.content_type),
    );

    let hits = db.recall_lexical("42", 4).unwrap();
    let h_lex = hits.iter().find(|h| h.rel.as_deref() == Some(sk_num)).expect("companion do '42'");
    rep.check(
        "AI-B: lexical do companion herda a declaração (type=Json) e resolve rel=",
        h_lex.content_type == ContentType::Json
            && h_lex.rel.as_deref() == Some(sk_num)
            && h_lex.payload_type == ContentType::Embedding(8),
        format!("ct={:?} rel={:?} payload={:?}", h_lex.content_type, h_lex.rel, h_lex.payload_type),
    );
    rep.check(
        "AI-B: hit lexical expõe matched_terms (o porquê do casamento)",
        !h_lex.matched_terms.is_empty(),
        format!("{:?}", h_lex.matched_terms),
    );

    // ── Seção 3: IA-B USA (verbatim, rel, binário cru) ──────────────────────
    // 10. consome o JSON verbatim (o content_type diz COMO: parse, não guess)
    let datum = &h_intent.text;
    rep.check(
        "AI-B: consome o JSON verbatim e extrai o alvo",
        parse_json(datum) && datum.contains("\"svc-42\""),
        datum.clone(),
    );

    // 11. segue rel= para o primário e re-usa o vetor com a MESMA dim (era)
    let prim = db.get(MemoryLayer::L4Semantic, "json/numero").unwrap().unwrap();
    rep.check(
        "AI-B: segue rel= e lê o payload do primário (8 floats f32 do modelo)",
        prim.payload.len() == 8 * 4,
        format!("payload={}B", prim.payload.len()),
    );

    // 12. consome o binário CRU pela key (nunca from_utf8_lossy — bytes exatos)
    let raw = db.get(MemoryLayer::L3EpisodicLong, "bin/checksum").unwrap().unwrap();
    rep.check(
        "AI-B: binário consumido cru — bytes idênticos aos que AI-A gravou",
        raw.payload == blob,
        format!("{:02X?}", raw.payload),
    );

    // fecha com o veredito do exemplo (exit code)
    if rep.done() {
        std::process::exit(0);
    }
    std::process::exit(1);
}