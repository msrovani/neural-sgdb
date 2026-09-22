//! Tipagem de payload dos hits (v1.1.6) — o DB é byte-oriented; o RETORNO
//! precisa dizer à inteligência consumidora QUE datum é e COMO parseá-lo.
//!
//! Duas instâncias de inteligência trocam dados que NÃO são palavras humanas:
//! embeddings (L4/L5 — floats crus, o "idioma" do modelo que os gravou),
//! JSON estruturado (intenções máquina→máquina), código, binários. O projeto
//! texto-only (`String::from_utf8_lossy` em tudo) desfigurava esses datums.
//!
//! `ContentType` é um HINT derivado (detecção barata na LEITURA — nunca
//! persistido); o writer pode declarar o tipo via seam para precisão (mesmo
//! contrato de `entities`/`Embedder`: quem fornece declara, o core sugere).
//! `RecallPath` identifica o caminho de retrieval de cada hit — crítica em
//! modo `hybrid`, onde distâncias de escalas diferentes (cosseno 0..1 vs BM25
//! normalizado) compartilham o mesmo campo `dist`.

/// Que tipo de datum o payload carrega.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentType {
    /// Prosa/verbatim (Text, Json e Code renderizam na projeção prosa).
    Text,
    /// Payload JSON (objeto/array) — máquina→máquina parseável.
    Json,
    /// Código-fonte (heurística HINT).
    Code,
    /// Embedding f32 (payload L4/L5) — floats NÃO viram prosa; o consumidor
    /// com o MESMO modelo os usa (era ADR-0007).
    Embedding(u32),
    /// Binário não-UTF8 — nunca `from_utf8_lossy`.
    Binary,
}

/// Caminho de retrieval que produziu o hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecallPath {
    Semantic,
    Lexical,
    Entities,
}

/// Detecta o tipo do payload. `embedding_dim = Some(dim)` quando o payload é
/// um embedding declarado (L4/L5 com bitvec ou payload f32) — dim = len/4.
pub fn detect_content_type(payload: &[u8], embedding_dim: Option<u32>) -> ContentType {
    if let Some(dim) = embedding_dim {
        return ContentType::Embedding(dim);
    }
    match core::str::from_utf8(payload) {
        Err(_) => ContentType::Binary,
        Ok(s) => {
            let trimmed = s.trim();
            if trimmed.len() >= 2
                && ((trimmed.starts_with('{') && trimmed.ends_with('}'))
                    || (trimmed.starts_with('[') && trimmed.ends_with(']')))
            {
                return ContentType::Json;
            }
            if looks_like_code(trimmed) {
                return ContentType::Code;
            }
            ContentType::Text
        }
    }
}

/// Heurística conservadora de código (HINT, não classificação): exige UMA
/// keyword (`fn `/`return `/`=> `…) MAIS um segundo sinal estrutural (outra
/// keyword, chave, semicolon ou arrow). Prosa sem keyword NUNCA vira code —
/// mesmo com `{key}`/`{text}` (placeholders de formato) + `;`/`->` (pontuação
/// normal: "BM25; em hybrid", "L5 -> md/L2"). Código real quase sempre tem
/// keyword (`fn main()`, `return x;`, `x => y`); o custo de rotular prosa
/// como code (consumidor com menos contexto pode tentar executar/parsear) é
/// maior que o de rotular code como text (que continua verbatim).
fn looks_like_code(s: &str) -> bool {
    let low = s.to_ascii_lowercase();
    let kws = [
        "fn ", "impl ", "struct ", "enum ", "trait ", "def ", "class ", "func ", "function ",
        "return ", "=> ",
    ];
    let kw_hits = kws.iter().filter(|kw| low.contains(**kw)).count();
    if kw_hits == 0 {
        return false;
    }
    let braces = s.bytes().filter(|b| *b == b'{' || *b == b'}').count();
    let semis = s.bytes().filter(|b| *b == b';').count();
    let arrows = s.matches("->").count();
    kw_hits >= 2 || braces >= 1 || semis >= 1 || arrows >= 1
}

/// Detector de payload de embedding: bitvec OU payload f32
/// (`payload.len() % 4 == 0 && >= 4`) — a mesma regra do `index_doc`/S1.
///
/// **Precondição de camada (v1.1.25):** só chame isto para L4/L5 — ver
/// [`payload_content_type`], que é a porta de entrada correta. Um payload de
/// PROSA cujo tamanho por acaso é múltiplo de 4 (25% dos textos!) satisfaz a
/// aritmética aqui e viraria `Embedding(len/4)`.
pub fn embedding_dim_of(payload: &[u8], has_bitvec: bool) -> Option<u32> {
    if !has_bitvec && payload.len() < 4 {
        return None;
    }
    if !payload.len().is_multiple_of(4) {
        return None;
    }
    Some((payload.len() / 4) as u32)
}

/// A CAMADA carrega vetor? Só L4 (semântica) e L5 (procedural) — exatamente as
/// que o BQ indexa (`engine::index_doc`, replicada no `validate`). Fora delas
/// um payload com `len % 4 == 0` é prosa, JSON, código ou binário.
pub fn key_carries_embedding(storage_key: &str) -> bool {
    storage_key.starts_with("md/L4/") || storage_key.starts_with("md/L5/")
}

/// Tipo do datum REAL do payload de um documento, honrando a CAMADA
/// (v1.1.25): `Embedding(dim)` só para L4/L5 (com bitvec ou payload f32);
/// fora delas o payload passa pelo detector normal (Text/Json/Code/Binary).
///
/// Nomeia a decisão que estava duplicada em quatro call sites (recall
/// semântico, dims, entidades e `primary_of`) — e que rotulava um L3 de prosa
/// como `Embedding(len/4)`: um consumidor máquina que confiasse no campo
/// tentaria reusar um vetor que não existe, que é justamente o erro que os
/// hits tipados (v1.1.6) existem para evitar.
pub fn payload_content_type(storage_key: &str, payload: &[u8], has_bitvec: bool) -> ContentType {
    let dim = if key_carries_embedding(storage_key) {
        embedding_dim_of(payload, has_bitvec)
    } else {
        None
    };
    detect_content_type(payload, dim)
}

/// Rótulo ESTÁVEL do tipo (v1.1.6 item 2 — seam de WRITE): o writer declara
/// `remember(..., type="json")` persistido em `MemoryMeta` (MDM1 v6); o
/// consumidor parseia o rótulo sem depender do detector nem do `Debug`.
/// `embedding` NÃO carrega a dim — ela vem do payload (`len/4`); o rótulo
/// só diz a FAMÍLIA do datum.
pub fn stable_label(ct: ContentType) -> &'static str {
    match ct {
        ContentType::Text => "text",
        ContentType::Json => "json",
        ContentType::Code => "code",
        ContentType::Embedding(_) => "embedding",
        ContentType::Binary => "binary",
    }
}

/// Parse do rótulo estável → `ContentType`. `Embedding(0)` é um placeholder:
/// a dim REAL vem do payload (resolve quem constrói o hit). `None` = rótulo
/// desconhecido (declaração inválida — `set_content_type` valida na escrita).
pub fn parse_stable_label(s: &str) -> Option<ContentType> {
    match s {
        "text" => Some(ContentType::Text),
        "json" => Some(ContentType::Json),
        "code" => Some(ContentType::Code),
        "embedding" => Some(ContentType::Embedding(0)),
        "binary" => Some(ContentType::Binary),
        _ => None,
    }
}

/// Renderiza verbatim na projeção prosa (v1.1.6): Text/Json/Code sim;
/// Embedding/Binary NUNCA viram prosa (`from_utf8_lossy` proíbe). O campo
/// `Hit.text` é não-vazio ⟺ `content_type` rende prosa.
pub fn renders_prose(ct: ContentType) -> bool {
    matches!(ct, ContentType::Text | ContentType::Json | ContentType::Code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_prose_verbatim() {
        assert_eq!(detect_content_type(b"clima ensolarado", None), ContentType::Text);
        assert_eq!(
            detect_content_type("memórias com acento e 数字".as_bytes(), None),
            ContentType::Text
        );
    }

    #[test]
    fn detects_json_delimited() {
        assert_eq!(
            detect_content_type(b"{\"intent\":\"query_status\",\"target\":\"svc-42\"}", None),
            ContentType::Json
        );
        assert_eq!(detect_content_type(b"[1, 2, 3]", None), ContentType::Json);
        // prosa com chave no meio NÃO é json (sem delimitador externo)
        assert_eq!(
            detect_content_type(b"use o {esquema} mas nao e json", None),
            ContentType::Text
        );
    }

    #[test]
    fn detects_code_and_binary_and_embedding() {
        assert_eq!(
            detect_content_type(b"fn main() {\n    println!(\"oi\");\n}", None),
            ContentType::Code
        );
        assert_eq!(
            detect_content_type(&[0xFF, 0xFE, 0x00, 0x01], None),
            ContentType::Binary
        );
        assert_eq!(
            detect_content_type(&[0u8; 16], Some(4)),
            ContentType::Embedding(4)
        );
    }

    #[test]
    fn prose_with_arrow_is_not_code() {
        // prosa descrevendo mapeamento com ` -> ` (memória v1.1.6): NÃO é
        // código — o custo de rotular prosa como code > o de code como text.
        assert_eq!(
            detect_content_type(
                b"o companion L5 -> md/L2/<id> quando o doc nao tem texto",
                None
            ),
            ContentType::Text
        );
        assert_eq!(detect_content_type(b"clima -> ensolarado hoje", None), ContentType::Text);
        // placeholders de formato (`{key}`/`{text}`) + arrow de prosa: ainda
        // NÃO é código (sem `;`, sem keyword) — documentação de formato.
        assert_eq!(
            detect_content_type(
                b"o formato e '- {key} | {text} (d=..)' e o companion -> md/L2",
                None
            ),
            ContentType::Text
        );
        // código real continua code: keyword + segundo sinal estrutural
        assert_eq!(
            detect_content_type(b"fn f(x) { return x + 1; }", None),
            ContentType::Code
        );
        // sem keyword NUNCA code, mesmo com chaves/`;`/`->` (prosa de
        // documentação de formato)
        assert_eq!(
            detect_content_type(b"x = { a: 1 }; y = { b: 2 };", None),
            ContentType::Text
        );
    }

    #[test]
    fn embedding_dim_matches_index_rule() {
        assert_eq!(embedding_dim_of(&[0u8; 16], true), Some(4));
        assert_eq!(embedding_dim_of(&[0u8; 16], false), Some(4));
        assert_eq!(embedding_dim_of(b"texto", true), None); // 5B não é múltiplo
        assert_eq!(embedding_dim_of(b"ab", false), None); // < 4B
    }

    #[test]
    fn payload_type_honors_the_layer_not_the_byte_count() {
        // v1.1.25: a aritmética de `embedding_dim_of` (len % 4 == 0 && >= 4)
        // é satisfeita por QUALQUER prosa de tamanho múltiplo de 4 — 25% dos
        // textos. Só a camada decide se o payload é um vetor.
        assert!(key_carries_embedding("md/L4/k"));
        assert!(key_carries_embedding("md/L5/k"));
        assert!(!key_carries_embedding("md/L3/k"), "L3 é texto, não vetor");
        assert!(!key_carries_embedding("md/L2/k"), "companion é a projeção");
        // prosa de 16 bytes (múltiplo de 4) NÃO vira Embedding(4)
        assert_eq!(
            payload_content_type("md/L3/prose", b"dezesseis bytes!", false),
            ContentType::Text
        );
        assert_eq!(
            payload_content_type("md/L2/prose", b"dezesseis bytes!", false),
            ContentType::Text
        );
        // o mesmo payload em L4 É o vetor (o consumidor reusa com o mesmo modelo)
        assert_eq!(
            payload_content_type("md/L4/k", &[0u8; 16], false),
            ContentType::Embedding(4)
        );
        assert_eq!(
            payload_content_type("md/L5/k", &[0u8; 16], true),
            ContentType::Embedding(4)
        );
        // L3 não-UTF8 continua Binary (nunca from_utf8_lossy)
        assert_eq!(
            payload_content_type("md/L3/bin", &[0xff, 0xfe, 0xfd, 0xfc], false),
            ContentType::Binary
        );
        // JSON delimitado em L3 continua Json
        assert_eq!(
            payload_content_type("md/L3/j", b"{\"num\": 42, \"x\": true}", false),
            ContentType::Json
        );
        // L4/L5 continuam reportando a dim do payload (regra do index_doc)
        assert_eq!(
            payload_content_type("md/L4/big", &[0u8; 64], false),
            ContentType::Embedding(16)
        );
        // e um L4 com payload curto demais não inventa vetor
        assert_eq!(payload_content_type("md/L4/short", b"abc", false), ContentType::Text);
    }
}