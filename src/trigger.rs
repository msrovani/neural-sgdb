//! Trigger rules (Ruflo-style conditional activation) — "em que arquivo o
//! cursor está ⇒ quais namespaces/entities ativar".
//!
//! O `recall` normal exige que o agente ESCOLHA o scope/entities. Mas num IDE
//! o contexto de execução (o arquivo em foco) já determina metade da decisão:
//! editar `src/kernel/*.rs` deve ativar as regras `bare-metal/no_std`; editar
//! `tests/*` deve ativar os cenários de teste. Este módulo guarda esse
//! MAPEAMENTO como side-table — o host diz "estou em `<path>`" e o core devolve
//! os namespaces correspondentes, sem varrer a base inteira a cada prompt.
//!
//! - **Matching no core** (no_std, zero-dep): glob com `*` (qualquer sequência
//!   SEM `/`), `**` (qualquer sequência INCLUINDO `/`) e `?` (um char não-`/`).
//!   Sem crate de regex — o padrão de caminho é limitado e determinístico.
//! - **Namespaces = entities**: um trigger devolve strings de *entity*; o
//!   recall por entity (`recall_entities`) é o mecanismo de 1-hop que já
//!   existe. Quem escreve a regra usa as MESMAS strings na memória e no
//!   trigger (mesmo contrato de `entities`/`Embedder`).
//! - **Sem wire type novo**: mora na side-table `sys/trigger/<hex>` (mesma
//!   precedência de `sys/negative/`, `sys/ttl/`, `sys/validity/`) — NMD1/TKLV
//!   intocados.
//! - **Identidade = padrão**: uma regra por padrão; `set_trigger` com o mesmo
//!   padrão sobrescreve (ADD-only de regras distintas, overwrite de config).
//!   O core NÃO decide o que ativar — ele devolve os namespaces; o host monta
//!   o prompt.

use alloc::string::String;
use alloc::vec::Vec;

/// Magic do VALOR do side-table (a chave não leva magic; o valor se
/// auto-descreve para sobreviver a um leitor de formato errado).
pub const TRIGGER_MAGIC: [u8; 4] = *b"TRG1";

/// Versão do layout do VALOR (não é bump de formato do crate: a side-table
/// evolui sem tocar no NMD1/TKLV).
pub const TRIGGER_VERSION: u8 = 1;

/// Prefixo do namespace lateral. A chave completa é
/// `sys/trigger/<16 hex>` — largura FIXA, porque a ART não suporta chaves onde
/// uma é prefixo de outra (regra 4 / ADR-0005).
pub const TRIGGER_PREFIX: &[u8] = b"sys/trigger/";

/// Uma regra de ativação: quando um caminho casa com `pattern`, os
/// `namespaces` (strings de entity) devem ser ativados no recall.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TriggerRule {
    /// Padrão de caminho (glob: `*` sem `/`, `**` com `/`, `?` um char).
    pub pattern: String,
    /// Namespaces = strings de *entity* a recallar (idênticas às da escrita
    /// da memória). Nunca vazio (regra sem efeito é rejeitada na escrita).
    pub namespaces: Vec<String>,
}

impl TriggerRule {
    /// Layout do valor: `magic(4) | ver(1) | pattern u16len+bytes |
    /// n u16le | [ns u16len+bytes]*`.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(7 + self.pattern.len());
        out.extend_from_slice(&TRIGGER_MAGIC);
        out.push(TRIGGER_VERSION);
        out.extend_from_slice(&(self.pattern.len() as u16).to_le_bytes());
        out.extend_from_slice(self.pattern.as_bytes());
        out.extend_from_slice(&(self.namespaces.len() as u16).to_le_bytes());
        for ns in &self.namespaces {
            out.extend_from_slice(&(ns.len() as u16).to_le_bytes());
            out.extend_from_slice(ns.as_bytes());
        }
        out
    }

    /// Decodifica com bounds-check em cada campo (nunca panics; entrada
    /// truncada/corrompida/namespace vazio → `None`). Devolve bytes consumidos.
    pub fn decode(data: &[u8]) -> Option<(Self, usize)> {
        if data.len() < 7 || data[0..4] != TRIGGER_MAGIC || data[4] != TRIGGER_VERSION {
            return None;
        }
        let plen = u16::from_le_bytes([*data.get(5)?, *data.get(6)?]) as usize;
        let mut off = 7usize;
        let pat_bytes = data.get(off..off + plen)?;
        let pattern = core::str::from_utf8(pat_bytes).ok()?.into();
        off += plen;
        let n = u16::from_le_bytes([*data.get(off)?, *data.get(off + 1)?]) as usize;
        off += 2;
        let mut namespaces = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            let elen = u16::from_le_bytes([*data.get(off)?, *data.get(off + 1)?]) as usize;
            off += 2;
            let ns_bytes = data.get(off..off + elen)?;
            // namespace vazio = regra sem efeito; recusa em vez de aceitar lixo
            if elen == 0 {
                return None;
            }
            namespaces.push(core::str::from_utf8(ns_bytes).ok()?.into());
            off += elen;
        }
        Some((Self { pattern, namespaces }, off))
    }
}

/// Chave do side-table: `sys/trigger/<fnv1a64(pattern):016x>`. Largura fixa de
/// 16 hex ⇒ nenhuma chave é prefixo de outra (ART, regra 4). O padrão é a
/// IDENTIDADE — a mesma regra gravada de novo sobrescreve.
pub fn trigger_key(pattern: &str) -> String {
    let h = crate::tickv::fnv1a64(pattern.as_bytes());
    alloc::format!("sys/trigger/{h:016x}")
}

/// Matching de caminho (glob no_std, zero-dep).
///
/// - `*` casa qualquer sequência de chars **sem** `/` (dentro de um segmento);
/// - `**` casa qualquer sequência **incluindo** `/` (atravessa diretórios);
/// - `?` casa exatamente um char, que **não** seja `/`;
/// - qualquer outro char é literal.
///
/// Exemplos: `src/kernel/*.rs` casa `src/kernel/mod.rs` mas NÃO
/// `src/kernel/sub/mod.rs`; `src/kernel/**` casa tudo sob `src/kernel/`;
/// `tests/*` casa `tests/a.rs` mas não `tests/a/b.rs`.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    glob_rec(pattern.as_bytes(), text.as_bytes())
}

fn glob_rec(pat: &[u8], txt: &[u8]) -> bool {
    if pat.is_empty() {
        return txt.is_empty();
    }
    if pat[0] == b'*' {
        // colapsa a sequência de estrelas: ** é globstar, * é single-star
        let mut star_len = 0;
        while star_len < pat.len() && pat[star_len] == b'*' {
            star_len += 1;
        }
        let rest = &pat[star_len..];
        if star_len >= 2 {
            // globstar: atravessa '/' — tenta o rest em toda posição
            for skip in 0..=txt.len() {
                if glob_rec(rest, &txt[skip..]) {
                    return true;
                }
            }
            false
        } else {
            // single-star: NÃO atravessa '/' — para antes do primeiro segmento
            for skip in 0..=txt.len() {
                if glob_rec(rest, &txt[skip..]) {
                    return true;
                }
                if skip < txt.len() && txt[skip] == b'/' {
                    break;
                }
            }
            false
        }
    } else if pat[0] == b'?' {
        !txt.is_empty() && txt[0] != b'/' && glob_rec(&pat[1..], &txt[1..])
    } else {
        !txt.is_empty() && txt[0] == pat[0] && glob_rec(&pat[1..], &txt[1..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec; // no_std test build: `vec!` nao esta no prelude

    #[test]
    fn roundtrip_and_truncation_never_panic() {
        let r = TriggerRule {
            pattern: String::from("src/kernel/*.rs"),
            namespaces: vec![
                String::from("bare-metal/no_std"),
                String::from("constraint/unsafe"),
            ],
        };
        let bytes = r.encode();
        let (back, used) = TriggerRule::decode(&bytes).expect("decodifica");
        assert_eq!(back, r);
        assert_eq!(used, bytes.len());
        // todo prefixo truncado é None (nunca panic, nunca lixo aceito)
        for cut in 0..bytes.len() {
            assert!(
                TriggerRule::decode(&bytes[..cut]).is_none(),
                "prefixo de {cut} bytes deveria falhar"
            );
        }
        // magic/versão errados são rejeitados
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(TriggerRule::decode(&bad).is_none());
        let mut badv = bytes.clone();
        badv[4] = 9;
        assert!(TriggerRule::decode(&badv).is_none());
        // namespace vazio (regra sem efeito) é recusado
        let mut empty_ns = r.clone();
        empty_ns.namespaces.push(String::new());
        assert!(TriggerRule::decode(&empty_ns.encode()).is_none());
    }

    #[test]
    fn key_is_deterministic_and_fixed_width() {
        let k1 = trigger_key("src/kernel/*.rs");
        let k2 = trigger_key("src/kernel/*.rs");
        let k3 = trigger_key("tests/*");
        assert_eq!(k1, k2, "determinístico");
        assert_ne!(k1, k3, "padrão diferente ⇒ chave diferente");
        assert_eq!(k1.len(), k3.len(), "largura fixa (ART regra 4)");
        assert!(k1.starts_with("sys/trigger/"));
    }

    #[test]
    fn glob_single_star_does_not_cross_slash() {
        assert!(glob_match("src/kernel/*.rs", "src/kernel/mod.rs"));
        assert!(
            !glob_match("src/kernel/*.rs", "src/kernel/sub/mod.rs"),
            "single-star não atravessa '/'"
        );
        assert!(glob_match("tests/*", "tests/a.rs"));
        assert!(!glob_match("tests/*", "tests/a/b.rs"));
        // single-star casa o segmento inteiro (inclui o ponto)
        assert!(glob_match("*.rs", "main.rs"));
        assert!(!glob_match("*.rs", "src/main.rs"));
    }

    #[test]
    fn glob_double_star_crosses_slash() {
        assert!(glob_match("src/kernel/**", "src/kernel/a.rs"));
        assert!(glob_match("src/kernel/**", "src/kernel/sub/deep/a.rs"));
        assert!(glob_match("**/*.rs", "src/a/b/c.rs"));
        assert!(glob_match("**", "qualquer/coisa"));
        // literal + globstar
        assert!(glob_match("src/**/test.rs", "src/a/b/test.rs"));
    }

    #[test]
    fn glob_question_and_literal() {
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(!glob_match("a?c", "a/c"), "? não casa '/'");
        assert!(glob_match("src/kernel/mod.rs", "src/kernel/mod.rs"));
        assert!(!glob_match("src/kernel/mod.rs", "src/kernel/mod2.rs"));
        // vazio
        assert!(glob_match("", ""));
        assert!(!glob_match("", "x"));
        assert!(glob_match("*", "abc"));
        // ** no fim casa também o diretório vazio
        assert!(glob_match("src/**", "src/"));
        assert!(!glob_match("src/**", "src"), "exige a '/' literal");
    }
}
