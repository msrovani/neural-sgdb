// v1.4.4 (Lote C) — seam do hasher da audit chain.
//
// ESTE arquivo existe por um motivo de CONCORRENCIA: `set_audit_hasher` mexe
// num global (`AtomicU8`). Um teste que o mutasse dentro do binário de lib
// correria em PARALELO com os testes da chain e poderia trocar o hasher no meio
// da verificação de outro teste — flakiness por estado global, a mesma classe
// do "testes acoplados a statics" que a triagem s413 apontou (ISSUE 17).
// Binário separado = processo separado = nenhum teste vizinho para colidir.
//
// Os valores em si (SHA-256 vs vetores do NIST, paridade do FNV com o
// histórico) são testados em `src/audit.rs`, sem tocar no global.

use neural_sgdb::audit::{audit_hasher, set_audit_hasher, Hasher};

#[test]
fn hasher_seam_is_selectable_and_restorable() {
    // default do processo: FNV-1a (a chain dos outros testes depende disso)
    assert_eq!(audit_hasher(), Hasher::Fnv1a64);

    set_audit_hasher(Hasher::Sha256Trunc);
    assert_eq!(audit_hasher(), Hasher::Sha256Trunc);
    assert_eq!(audit_hasher().label(), "sha256-trunc");

    set_audit_hasher(Hasher::Fnv1a64);
    assert_eq!(audit_hasher(), Hasher::Fnv1a64);
    assert_eq!(audit_hasher().label(), "fnv1a64");
}