//! Limites centralizados (P1-3).
//!
//! Fonte única de verdade para os tetos de tamanho do mecanismo. Antes do
//! P1-3, `MAX_KLEN`/`MAX_VLEN` existiam em duplicata (`src/storage.rs`
//! privadas, `src/tickv.rs` públicas) e `MAX_EMBEDDING_DIM` vivia em
//! `src/bq.rs` — um drift entre eles podia divergir o parser do FileStorage do
//! TKLV silenciosamente. Todas as camadas agora referenciam este módulo; os
//! antigos pontos públicos são re-exportados (`tickv::MAX_KLEN`,
//! `bq::MAX_EMBEDDING_DIM`) para não quebrar API.
//!
//! Regra de uso: valide ANTES de alocar. Todo leitor de dados externos
//! (recovery, `scan_volume`, fast-mount, decode) checa estes tetos antes de
//! `to_vec`/`Vec::with_capacity` — um `klen`/`vlen` acima do teto indica cauda
//! corrompida ou dados de outro formato, nunca deve disparar alocação.

/// Teto de comprimento de chave (paridade com o OS e com o TKLV).
///
/// 4096 bytes. Chaves acima disso são rejeitadas na escrita
/// (`SgdbError::Storage("limits")`) — aceitá-las no append mas rejeitá-las no
/// recovery truncaria o arquivo silenciosamente (bughunt #11).
pub const MAX_KLEN: usize = 4096;

/// Teto de comprimento de valor (paridade com o OS e com o TKLV).
///
/// 1 MiB. Documentos acima disso (memórias gigantes, bitvecs L4) são erro de
/// chamador — nunca truncamento silencioso.
pub const MAX_VLEN: usize = 1024 * 1024;

/// Dimensão máxima aceita para embeddings (política de entrada, P1-1).
///
/// 4096 floats = 64 words × 64 bits. Acima disso `quantize_f32`/`insert_f32`
/// crescem sem limite útil (payload NMD1 com 4B/float). `remember_semantic`/
/// `recall_impl` rejeitam com `SgdbError::Invalid` ANTES de alocar/gravar.
pub const MAX_EMBEDDING_DIM: usize = 4096;

/// Teto padrão de bytes para um bloco de contexto RAG (`rag_context`/`
/// `rag_context_oversampled`, P1-6). Evita que um `k` alto materialize um
/// prompt gigante sem teto — o contexto é truncado em fronteira de char.
pub const MAX_RAG_CONTEXT_BYTES: usize = 8192;

/// Tamanho padrão de página para `scan_prefix_page` (P1-6).
///
/// Paginar evita materializar todo o prefixo de uma vez; o caller percorre
/// com `offset` crescente (ordem lexicográfica determinística).
pub const DEFAULT_SCAN_PAGE_SIZE: usize = 100;

/// Limiar de órfãos do BQ para a recuperação proativa (v1.1.3 S4).
///
/// O flat do BQ é append-only: `delete` físico deixa o id no índice (inofensivo,
/// o recall o pula, mas infla o pool de candidatos). Quando os órfãos passam
/// deste número, `Sgdb::delete` reempacota o índice na hora (`reclaim_bq_orphans`).
/// `0` = sempre reempacota (chamada manual); o default escolhe um ponto em que
/// o custo O(N) de reempacotar compensa a poupança de candidatos.
pub const DEFAULT_BQ_ORPHAN_THRESHOLD: usize = 64;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_single_source_of_truth() {
        // P1-3: nenhum módulo pode divergir do teto central. Se alguém mudar
        // o limite aqui, estes pinos forçam revisão dos pontos de re-export.
        assert_eq!(MAX_KLEN, 4096);
        assert_eq!(MAX_VLEN, 1024 * 1024);
        assert_eq!(MAX_EMBEDDING_DIM, 4096);
        // tickv/bq re-exportam o MESMO valor (não cópias)
        assert_eq!(crate::tickv::MAX_KLEN, MAX_KLEN);
        assert_eq!(crate::tickv::MAX_VLEN, MAX_VLEN);
        assert_eq!(crate::bq::MAX_EMBEDDING_DIM, MAX_EMBEDDING_DIM);
        assert_eq!(DEFAULT_BQ_ORPHAN_THRESHOLD, 64);
    }
}