//! Ledger de NEGATIVOS (v1.1.24, ADR-0014) — "o que já verifiquei e NÃO
//! existe".
//!
//! A memória guarda o que foi DITO; um agente também precisa lembrar do que
//! foi PROCURADO e não estava lá — senão re-probe o mesmo buraco a cada
//! sessão (e cada probe é um turno pago). Este módulo é o substrato:
//!
//! - **Identidade determinística**: a entrada é a query NORMALIZADA pelos
//!   mesmos tokens do BM25 (`lexical::tokenize` re-unidos por espaço), então
//!   caixa e pontuação não fragmentam o registro. O que AINDA fragmenta:
//!   paráfrases e variantes de acento (`café` ≠ `cafe`, porque a normalização
//!   não dobra diacríticos — a mesma limitação do BM25). Como nas `entities`
//!   e no `Embedder`, o contrato é "mesma forma canônica na escrita e na
//!   busca"; quem fornece escolhe a string.
//! - **Escopado**: a chave inclui o escopo, logo a ausência de um tenant
//!   nunca é servida como evidência do outro (mesma regra do null-scoping).
//! - **Reforço, não duplicação**: repetir a mesma query incrementa `probes` e
//!   atualiza `last_tick` na mesma chave (nenhum crescimento por re-probe).
//! - **Sem wire type novo**: mora numa side-table (`sys/negative/<hex>`), como
//!   `sys/validity/` e `sys/ttl/` — o NMD1/TKLV continuam intocados.
//!
//! O core **não decide** o que é ausência: `recall_with_ledger` registra
//! quando um probe lexical volta vazio, mas quem afirma "isto não existe" é a
//! camada superior (`note_absence`), e quem apaga é `forget_absence` — ou o
//! próprio ledger, quando um probe posterior finalmente encontra a memória
//! (self-healing).

use alloc::string::String;
use alloc::vec::Vec;

/// Magic do valor do side-table. Sem `magic` no KEY (a chave é
/// `sys/negative/<hex>`); o valor se auto-descreve para sobreviver a um
/// leitor de formato errado.
pub const NEGATIVE_MAGIC: [u8; 4] = *b"NDG1";

/// Versão do layout do VALOR (não é um version bump de formato do crate: a
/// side-table pode evoluir sem tocar no NMD1/TKLV).
pub const NEGATIVE_VERSION: u8 = 1;

/// Prefixo do namespace lateral. A chave completa é sempre
/// `sys/negative/<16 hex>` — largura FIXA, porque a ART não suporta chaves
/// onde uma é prefixo de outra (regra 4 / ADR-0005).
pub const NEGATIVE_PREFIX: &[u8] = b"sys/negative/";

/// Uma ausência registrada: "a query `query`, no escopo `scope`, foi procurada
/// `probes` vez(es) e não estava lá".
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct AbsenceEntry {
    /// Query NORMALIZADA (tokens do BM25 re-unidos por espaço) — a identidade
    /// do registro, não o texto literal que o agente digitou.
    pub query: String,
    /// Escopo da pergunta (`""` = global). Nunca cruza escopos.
    pub scope: String,
    /// Quantas vezes foi procurada e não achada (`u32` saturante).
    pub probes: u32,
    /// Tick do primeiro registro.
    pub first_tick: u64,
    /// Tick do último probe.
    pub last_tick: u64,
}

impl AbsenceEntry {
    /// Layout do valor: `magic(4) | ver(1) | probes u32le | first u64le |
    /// last u64le | scope u16len+bytes | query u16len+bytes`.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(25 + self.scope.len() + self.query.len());
        out.extend_from_slice(&NEGATIVE_MAGIC);
        out.push(NEGATIVE_VERSION);
        out.extend_from_slice(&self.probes.to_le_bytes());
        out.extend_from_slice(&self.first_tick.to_le_bytes());
        out.extend_from_slice(&self.last_tick.to_le_bytes());
        out.extend_from_slice(&(self.scope.len() as u16).to_le_bytes());
        out.extend_from_slice(self.scope.as_bytes());
        out.extend_from_slice(&(self.query.len() as u16).to_le_bytes());
        out.extend_from_slice(self.query.as_bytes());
        out
    }

    /// Decodifica com bounds-check em cada campo (nunca panics; entrada
    /// truncada/corrompida → `None`). Devolve também os bytes consumidos.
    pub fn decode(data: &[u8]) -> Option<(Self, usize)> {
        if data.len() < 25 || data[0..4] != NEGATIVE_MAGIC || data[4] != NEGATIVE_VERSION {
            return None;
        }
        let probes = u32::from_le_bytes([data[5], data[6], data[7], data[8]]);
        let first_tick = u64::from_le_bytes([
            data[9], data[10], data[11], data[12], data[13], data[14], data[15], data[16],
        ]);
        let last_tick = u64::from_le_bytes([
            data[17], data[18], data[19], data[20], data[21], data[22], data[23], data[24],
        ]);
        let mut off = 25usize;
        let slen = u16::from_le_bytes([*data.get(off)?, *data.get(off + 1)?]) as usize;
        off += 2;
        let scope_bytes = data.get(off..off + slen)?;
        let scope = core::str::from_utf8(scope_bytes).ok()?.into();
        off += slen;
        let qlen = u16::from_le_bytes([*data.get(off)?, *data.get(off + 1)?]) as usize;
        off += 2;
        let query_bytes = data.get(off..off + qlen)?;
        let query = core::str::from_utf8(query_bytes).ok()?.into();
        off += qlen;
        Some((
            Self {
                query,
                scope,
                probes,
                first_tick,
                last_tick,
            },
            off,
        ))
    }
}

/// Normaliza a query para a identidade do registro: mesmos tokens do BM25
/// (lowercase, sem pontuação), re-unidos por espaço. Duas grafias com as
/// MESMAS palavras colidem no mesmo registro.
pub fn normalize_query(s: &str) -> String {
    crate::lexical::tokenize(s).join(" ")
}

/// Chave do side-table: `sys/negative/<fnv1a64(scope ‖ 0x1f ‖ query):016x>`.
/// Largura fixa de 16 hex ⇒ nenhuma chave é prefixo de outra (ART, regra 4).
/// O separador `0x1f` (unit separator) impede que `("ab","c")` colida com
/// `("a","bc")`.
pub fn negative_key(scope: &str, norm_query: &str) -> String {
    let mut buf = Vec::with_capacity(scope.len() + norm_query.len() + 1);
    buf.extend_from_slice(scope.as_bytes());
    buf.push(0x1f);
    buf.extend_from_slice(norm_query.as_bytes());
    let h = crate::tickv::fnv1a64(&buf);
    alloc::format!("sys/negative/{h:016x}")
}

/// Extrai o hex de uma chave `sys/negative/<hex>` (para não re-hashear ao
/// remover/listar). `None` se o prefixo não casa.
pub fn negative_digest(key: &[u8]) -> Option<&[u8]> {
    key.strip_prefix(NEGATIVE_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn roundtrip_and_truncation_never_panic() {
        let e = AbsenceEntry {
            query: String::from("preco do cafe hoje"),
            scope: String::from("user/ana"),
            probes: 7,
            first_tick: 1_000,
            last_tick: 42_000,
        };
        let bytes = e.encode();
        let (back, used) = AbsenceEntry::decode(&bytes).expect("decodifica");
        assert_eq!(back, e);
        assert_eq!(used, bytes.len());
        // todo prefixo truncado é None (nunca panic, nunca lixo aceito)
        for cut in 0..bytes.len() {
            assert!(
                AbsenceEntry::decode(&bytes[..cut]).is_none(),
                "prefixo de {cut} bytes deveria falhar"
            );
        }
        // magic/versão errados são rejeitados
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(AbsenceEntry::decode(&bad).is_none());
        let mut badv = bytes.clone();
        badv[4] = 9;
        assert!(AbsenceEntry::decode(&badv).is_none());
        // len mentindo (scope maior que o resto) → None
        let mut lying = bytes.clone();
        lying[25] = 0xff;
        lying[26] = 0x7f;
        assert!(AbsenceEntry::decode(&lying).is_none());
    }

    #[test]
    fn normalization_collides_same_words_and_key_is_fixed_width() {
        // A normalização é a do BM25: lowercase + sem pontuação. NÃO dobra
        // diacríticos — "café" e "cafe" são tokens distintos (limitação
        // conhecida e documentada; o oráculo aqui é o tokenizer real).
        assert_eq!(normalize_query("ONDE está o Café?"), "onde está o café");
        assert_eq!(
            normalize_query("CHA, verde!"),
            normalize_query("cha verde"),
            "caixa e pontuação não fragmentam ⇒ mesma identidade de ausência"
        );
        assert_ne!(
            normalize_query("cafe"),
            normalize_query("cha"),
            "palavras diferentes não colidem"
        );
        let k1 = negative_key("user/ana", "cafe");
        let k2 = negative_key("user/ana", "cafe");
        let k3 = negative_key("user/bruno", "cafe");
        assert_eq!(k1, k2, "determinístico");
        assert_ne!(k1, k3, "escopo entra na chave — ausências não cruzam tenants");
        assert_eq!(k1.len(), k3.len(), "largura fixa (ART regra 4)");
        // o separador evita colisão de concatenação
        assert_ne!(negative_key("ab", "c"), negative_key("a", "bc"));
        assert_eq!(
            negative_digest(k1.as_bytes()),
            Some(&k1.as_bytes()[NEGATIVE_PREFIX.len()..])
        );
        assert!(negative_digest(b"md/L4/x").is_none());
    }

    #[test]
    fn probes_saturates_instead_of_wrapping() {
        let mut e = AbsenceEntry {
            probes: u32::MAX,
            ..Default::default()
        };
        e.probes = e.probes.saturating_add(1);
        assert_eq!(e.probes, u32::MAX, "re-probe infinito não volta a zero");
        // decodifica de volta com o teto
        let (back, _) = AbsenceEntry::decode(&e.encode()).unwrap();
        assert_eq!(back.probes, u32::MAX);
        let _ = vec![0u8; 0];
    }
}
