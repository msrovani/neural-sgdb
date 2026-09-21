//! Fingerprint canônico do estado DERIVADO (v1.1.21, ADR-0011) — o oráculo de
//! equivalência de reconstrução.
//!
//! `Sgdb::index_fingerprint` responde uma pergunta que antes só se respondia por
//! comportamento: **o índice derivado em RAM é o mesmo que um rebuild
//! produziria?** Sem isso, qualquer aceleração do `open` (ADR-0009) era fé —
//! não havia como provar que um snapshot montado equivale ao rebuild.
//!
//! Regras do hash (todas deliberadas):
//!
//! - **Ordem canônica, nunca ordem de inserção.** Todo componente entra em
//!   ordem determinística: `BTreeMap`/`BTreeSet` já ordenam; o que não ordena
//!   (chaves do ART) é ordenado antes de entrar.
//! - **IDs ficam FORA.** `NEXT_ID` é um contador **global de processo**
//!   (`engine.rs`): o mesmo corpus em dois `open` produz ids DIFERENTES, logo
//!   hash de id não é estável entre processos. O fingerprint resolve
//!   `id → storage_key` e hasheia a CHAVE.
//! - **Órfãos do BQ ficam FORA.** O BQ é append-only: `delete` deixa entradas
//!   inertes até a recompacção (`reclaim_bq_orphans`). Excluir os órfãos faz o
//!   fingerprint medir o índice EFETIVO (o que o recall enxerga) e ficar
//!   estável quando a recompacção roda — o que também torna a deleção
//!   simétrica (`fp` volta ao valor anterior à escrita).
//! - **Floats ficam FORA.** A soma f64 do corpus (`corpus_mean`) NÃO é
//!   associativa: o rebuild acumula numa ordem e a escrita incremental noutra,
//!   então os últimos bits podem divergir legitimamente. A média é verificada
//!   à parte, com tolerância relativa (`Sgdb::validate`).
//!
//! `no_std`-safe (alloc-only, sem atomics).

/// Offset basis do FNV-1a 64.
pub(crate) const FP_SEED: u64 = 0xcbf2_9ce4_8422_2325;
const FP_PRIME: u64 = 0x0000_0100_0000_01B3;

/// Mistura bytes num hash FNV-1a 64 incremental (determinístico, sem alloc).
pub(crate) fn fp_mix(h: u64, bytes: &[u8]) -> u64 {
    let mut h = h;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(FP_PRIME);
    }
    h
}

/// Mistura um escalar em largura fixa (LE) — sem ambiguidade de fronteira.
pub(crate) fn fp_mix_u64(h: u64, v: u64) -> u64 {
    fp_mix(h, &v.to_le_bytes())
}

/// Mistura uma string **com prefixo de largura**. Sem a largura, `("ab","c")` e
/// `("a","bc")` gerariam o mesmo fluxo de bytes e colidiriam.
pub(crate) fn fp_mix_str(h: u64, s: &str) -> u64 {
    fp_mix(fp_mix_u64(h, s.len() as u64), s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vetor conhecido do FNV-1a 64 — mesmo valor pinado em `tickv.rs`
    /// (`fnv1a64_known_vector`), o que confirma paridade de implementação.
    #[test]
    fn fp_known_vector_matches_fnv1a64() {
        assert_eq!(fp_mix(FP_SEED, b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(crate::tickv::fnv1a64(b"a"), fp_mix(FP_SEED, b"a"));
    }

    /// A largura prefixada é o que impede colisão por deslocamento de fronteira.
    #[test]
    fn fp_mix_str_is_length_prefixed() {
        let left = fp_mix_str(fp_mix_str(FP_SEED, "ab"), "c");
        let right = fp_mix_str(fp_mix_str(FP_SEED, "a"), "bc");
        assert_ne!(left, right, "concatenado sem largura colidiria");
    }

    /// Escalares de largura fixa não têm ambiguidade nem colisão trivial.
    #[test]
    fn fp_mix_u64_distinguishes_fields() {
        let a = fp_mix_u64(fp_mix_u64(FP_SEED, 1), 2);
        let b = fp_mix_u64(fp_mix_u64(FP_SEED, 2), 1);
        assert_ne!(a, b);
    }

    /// Ordem dos bytes LE é estável e independente da plataforma.
    #[test]
    fn fp_mix_deterministic_and_seed_sensitive() {
        assert_eq!(fp_mix(FP_SEED, b"abc"), fp_mix(FP_SEED, b"abc"));
        assert_ne!(fp_mix(FP_SEED, b"abc"), fp_mix(FP_SEED + 1, b"abc"));
        assert_eq!(fp_mix(FP_SEED, b""), FP_SEED, "vazio não move o hash");
    }
}
