//! **Transporte assinado (v1.0, ADR-0006)** — como plugar autenticação REAL
//! num transporte p2p do neural-sgdb, sem crypto no core.
//!
//! Run: `cargo run --release --example signed_peer --features p2p`
//!
//! O núcleo NÃO implementa criptografia (decisão ADR-0006 — zero deps
//! `no_std`). O que o crate oferece é o **seam** na fronteira de transporte:
//!
//! - `trust::Signer` — trait `sign(payload) -> auth` / `verify(payload, auth)`
//! - `trust::TrustStore` — registro bounded de peers (identidade + auth +
//!   trust), política do HOST (o core nunca decide quem é confiável)
//! - `crdt::SignedEnvelope` — payload + campo `auth` opaco no wire
//!
//! **Produção**: seu transporte implementa `Signer` com Ed25519/HMAC real (ou
//! usa o TLS/JWT do host) e rejeita pacotes onde `verify` falha ANTES de tocar
//! o storage. Aqui usamos `HmacFnvSigner` — o DEMO determinístico do crate
//! (FNV-1a keyed, **NÃO é criptografia**) apenas para provar o fluxo ponta a
//! ponta: assinar → envelope → verificar → rejeitar adulterado e peer
//! não-autenticado.

use neural_sgdb::trust::{AuthStatus, HmacFnvSigner, Peer, Signer, TrustLevel, TrustStore};
use neural_sgdb::{CrdtMemorySync, SignedEnvelope};

/// Emissor/receptor com signer (em produção: troque `HmacFnvSigner` por
/// Ed25519/HMAC — a estrutura do fluxo é idêntica).
struct SignedPeer {
    node_id: u8,
    signer: HmacFnvSigner,
}

fn main() {
    println!("== Transporte assinado: fluxo de referência (seam ADR-0006) ==");
    println!("   Demo: HmacFnvSigner (FNV keyed, NAO e criptografia).");
    println!("   Producao: implemente `trust::Signer` com Ed25519/HMAC/TLS e");
    println!("   rejeite pacotes com `verify == false` antes do storage.\n");

    // 1) host pluga um signer (demo: chave compartilhada FNV)
    let signer = HmacFnvSigner::new(0xfeed_beef);
    let alice = SignedPeer { node_id: 1, signer };

    // 2) emissor assina o payload de protocolo e monta o envelope autenticável
    let payload = b"md/L4/k1".to_vec();
    let auth = alice.signer.sign(&payload);
    let env = SignedEnvelope::new(alice.node_id, payload.clone(), auth);
    let wire = env.try_encode().unwrap();
    println!("[1] Alice assina payload ({} B) -> SignedEnvelope ({:?})",
        payload.len(), &wire[..wire.len().min(12)]);

    // 3) receptor decodifica e VERIFICA antes de tocar o storage
    let (dec, _) = SignedEnvelope::decode(&wire).unwrap();
    assert!(alice.signer.verify(&dec.payload, &dec.auth), "assinatura valida");
    println!("[2] Bob decodifica e verifica a assinatura: OK");

    // 4) payload adulterado em transito -> verify falha -> rejeitado
    let mut tampered = dec.clone();
    tampered.payload = b"md/L4/EVIL".to_vec();
    assert!(!alice.signer.verify(&tampered.payload, &tampered.auth));
    println!("[3] payload adulterado (md/L4/EVIL) -> verify == false -> REJEITADO");

    // 5) peer nao-autenticado: mesmo com assinatura valida, a politica do host
    //    (TrustStore) rejeita no nivel do transporte
    let mut ts = TrustStore::new();
    ts.upsert(Peer {
        node_id: 1,
        identity: String::from("alice"),
        auth: AuthStatus::Authenticated,
        trust: TrustLevel::Trusted,
        capabilities: vec![String::from("delta")],
    });
    assert!(ts.is_trusted(1, TrustLevel::Trusted));
    assert!(!ts.is_trusted(9, TrustLevel::Trusted), "peer desconhecido rejeitado");
    println!("[4] TrustStore: alice (node 1) confiavel; node 9 desconhecido -> rejeitado");

    // 6) CrdtMemorySync convive com o seam: o relogio de versoes nao sabe nem
    //    precisa saber de crypto — a assinatura vive no envelope/transporte
    let crdt = CrdtMemorySync::new(1);
    assert_eq!(crdt.node_id(), 1);
    println!("[5] CRDT (node {}) intacto: crypto vive na fronteira, nao no core", crdt.node_id());

    println!("\nFluxo de transporte assinado OK (seam ADR-0006).");
}