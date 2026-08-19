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

/// Detector de payload de embedding: L4/L5 com bitvec OU payload f32
/// (`payload.len() % 4 == 0 && >= 4`) — a mesma regra do `index_doc`/S1.
pub fn embedding_dim_of(payload: &[u8], has_bitvec: bool) -> Option<u32> {
    if !has_bitvec && payload.len() < 4 {
        return None;
    }
    if !payload.len().is_multiple_of(4) {
        return None;
    }
    Some((payload.len() / 4) as u32)
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
}