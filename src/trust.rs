//! Trust seam (v1.0, roadmap Phase 23/28) — peer trust at the transport /
//! protocol boundary, WITHOUT contaminating the storage core.
//!
//! The core (memory model, clock, CRDT, lifecycle) stays free of networking
//! security. This module provides the *seam* a host/transport fills in:
//!
//! - [`Peer`] — identity + authentication status + trust state + provenance;
//! - [`TrustStore`] — bounded registry of known peers (node_id → Peer);
//! - [`Signer`] — trait for delta/payload authentication; the demo
//!   [`HmacFnvSigner`] is a **development-only** keyed hash (NOT
//!   cryptographic). A production host supplies a real signer (Ed25519/HMAC
//!   over TLS) at the transport boundary and rejects bad `auth` fields.
//!
//! The `SignedEnvelope` in `crdt.rs` already carries an opaque `auth` field;
//! this module is the trust model around it. `no_std`-safe (alloc only).

use alloc::string::String;
use alloc::vec::Vec;

/// Status de autenticação de um peer (opaco para o core — preenchido pelo
/// transporte/host, ex: handshake TLS, assinatura de pacote).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthStatus {
    /// Nunca autenticado (ou handshake ainda não concluído).
    Unauthenticated,
    /// Identidade verificada pelo transporte.
    Authenticated,
    /// Credencial revogada — transportes devem rejeitar pacotes.
    Revoked,
}

/// Nível de confiança atribuído pelo host (política externa, nunca o core).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TrustLevel {
    /// Não confiável: dados aceitos apenas como referência, nunca como fonte.
    Untrusted,
    /// Confiável para dados comuns.
    Trusted,
    /// Alta confiança (ex: réplica própria, peer de infra).
    HighlyTrusted,
}

/// Peer conhecido pelo host (roadmap Phase 28):
/// node_id + identity + auth + trust + capabilities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub node_id: u8,
    /// Identidade opaca (fingerprint, cert, chave pública) — nunca confundida
    /// com memory_id nem com node_id.
    pub identity: String,
    pub auth: AuthStatus,
    pub trust: TrustLevel,
    /// Capacidades declaradas (ex: `"delta"`, `"snapshot"`, `"lifecycle"`).
    pub capabilities: Vec<String>,
}

/// Registro bounded de peers. O host decide quem é `Trusted`/`Revoked`; o
/// core apenas consulta ([`TrustStore::is_trusted`]) sem implementar política.
#[derive(Clone, Debug, Default)]
pub struct TrustStore {
    peers: Vec<Peer>,
    /// Máximo de peers registrados (política de nó bounded — roadmap §6).
    max_peers: usize,
}

impl TrustStore {
    pub const DEFAULT_MAX_PEERS: usize = 64;

    pub fn new() -> Self {
        TrustStore {
            peers: Vec::new(),
            max_peers: Self::DEFAULT_MAX_PEERS,
        }
    }

    /// Registra/atualiza um peer (upsert por node_id). Bounded: além de
    /// `max_peers`, retorna `false` (não estoura memória).
    pub fn upsert(&mut self, peer: Peer) -> bool {
        if let Some(p) = self.peers.iter_mut().find(|p| p.node_id == peer.node_id) {
            *p = peer;
            return true;
        }
        if self.peers.len() >= self.max_peers {
            return false;
        }
        self.peers.push(peer);
        true
    }

    pub fn get(&self, node_id: u8) -> Option<&Peer> {
        self.peers.iter().find(|p| p.node_id == node_id)
    }

    pub fn peers(&self) -> &[Peer] {
        &self.peers
    }

    /// Peers com `auth == Authenticated` e `trust >= min`.
    pub fn trusted_peers(&self, min: TrustLevel) -> Vec<Peer> {
        self.peers
            .iter()
            .filter(|p| p.auth == AuthStatus::Authenticated && p.trust >= min)
            .cloned()
            .collect()
    }

    /// Conveniência: um peer é fonte confiável?
    pub fn is_trusted(&self, node_id: u8, min: TrustLevel) -> bool {
        match self.get(node_id) {
            Some(p) => p.auth == AuthStatus::Authenticated && p.trust >= min,
            None => false,
        }
    }

    /// Revoga um peer (host chamou; transportes passam a rejeitar).
    pub fn revoke(&mut self, node_id: u8) {
        if let Some(p) = self.peers.iter_mut().find(|p| p.node_id == node_id) {
            p.auth = AuthStatus::Revoked;
        }
    }

    pub fn set_trust(&mut self, node_id: u8, trust: TrustLevel) -> bool {
        match self.peers.iter_mut().find(|p| p.node_id == node_id) {
            Some(p) => {
                p.trust = trust;
                true
            }
            None => false,
        }
    }

    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }
}

/// Assinatura/autenticação de payloads no seam de transporte.
///
/// O CORE não assina nem verifica — transportes usam o trait para preencher
/// `SignedEnvelope.auth` e rejeitar pacotes inválidos antes de tocar o
/// storage. Implementações reais devem ser criptográficas (fora do core).
pub trait Signer {
    /// Assina `payload`, produzindo o `auth` a anexar ao envelope.
    fn sign(&self, payload: &[u8]) -> Vec<u8>;
    /// Verifica `auth` contra `payload`.
    fn verify(&self, payload: &[u8], auth: &[u8]) -> bool;
}

/// Signer DEMO (v1.0) — **NÃO é criptografia**. FNV-1a 64 keyed por um
/// segredo compartilhado, deterministic e sem deps — suficiente para testes
/// de contrato e para provar o fluxo (assinatura → envelope → verificação).
/// Produção: troque por Ed25519/HMAC real no transporte.
pub struct HmacFnvSigner {
    key: u64,
}

impl HmacFnvSigner {
    pub const OFFSET: u64 = 0xcbf29ce484222325;
    pub const PRIME: u64 = 0x100000001b3;

    pub fn new(key: u64) -> Self {
        HmacFnvSigner { key }
    }
}

impl Signer for HmacFnvSigner {
    fn sign(&self, payload: &[u8]) -> Vec<u8> {
        // FNV-1a sobre payload || key — demo, não criptográfico.
        let mut h = Self::OFFSET;
        for b in payload {
            h ^= *b as u64;
            h = h.wrapping_mul(Self::PRIME);
        }
        for i in 0..8 {
            let b = (self.key >> (i * 8)) as u8;
            h ^= b as u64;
            h = h.wrapping_mul(Self::PRIME);
        }
        h.to_le_bytes().to_vec()
    }

    fn verify(&self, payload: &[u8], auth: &[u8]) -> bool {
        auth.len() == 8 && auth == self.sign(payload).as_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn trust_store_upsert_get_and_bounded() {
        let mut ts = TrustStore::new();
        ts.max_peers = 2;
        assert!(ts.upsert(Peer {
            node_id: 1,
            identity: String::from("alice"),
            auth: AuthStatus::Authenticated,
            trust: TrustLevel::Trusted,
            capabilities: vec![String::from("delta")],
        }));
        assert!(ts.upsert(Peer {
            node_id: 2,
            identity: String::from("bob"),
            auth: AuthStatus::Unauthenticated,
            trust: TrustLevel::Untrusted,
            capabilities: Vec::new(),
        }));
        // bounded: terceiro peer rejeitado
        assert!(!ts.upsert(Peer {
            node_id: 3,
            identity: String::from("carol"),
            auth: AuthStatus::Authenticated,
            trust: TrustLevel::Trusted,
            capabilities: Vec::new(),
        }));
        assert_eq!(ts.peer_count(), 2);
        // upsert existente atualiza sem crescer
        assert!(ts.upsert(Peer {
            node_id: 1,
            identity: String::from("alice2"),
            auth: AuthStatus::Authenticated,
            trust: TrustLevel::HighlyTrusted,
            capabilities: vec![String::from("delta")],
        }));
        assert_eq!(ts.peer_count(), 2);
        assert_eq!(ts.get(1).unwrap().identity, "alice2");
    }

    #[test]
    fn trust_store_is_trusted_and_revoke() {
        let mut ts = TrustStore::new();
        ts.upsert(Peer {
            node_id: 7,
            identity: String::from("infra"),
            auth: AuthStatus::Authenticated,
            trust: TrustLevel::Trusted,
            capabilities: Vec::new(),
        });
        assert!(ts.is_trusted(7, TrustLevel::Trusted));
        assert!(!ts.is_trusted(7, TrustLevel::HighlyTrusted));
        assert!(!ts.is_trusted(8, TrustLevel::Trusted));
        ts.set_trust(7, TrustLevel::HighlyTrusted);
        assert!(ts.is_trusted(7, TrustLevel::HighlyTrusted));
        ts.revoke(7);
        assert!(!ts.is_trusted(7, TrustLevel::Trusted));
        assert_eq!(ts.get(7).unwrap().auth, AuthStatus::Revoked);
        // trusted_peers filtra auth + trust
        assert!(ts.trusted_peers(TrustLevel::Untrusted).is_empty());
    }

    #[test]
    fn demo_signer_signs_and_verifies() {
        let s = HmacFnvSigner::new(0xdead_beef);
        let payload = b"hello trust seam";
        let auth = s.sign(payload);
        assert!(s.verify(payload, &auth));
        // payload adulterado → falha
        assert!(!s.verify(b"hello trust se@m", &auth));
        // key diferente → assinatura diferente
        let s2 = HmacFnvSigner::new(0x1234_5678);
        assert!(!s2.verify(payload, &auth));
        // auth de tamanho errado → falha
        assert!(!s.verify(payload, &auth[..4]));
    }

    #[test]
    fn demo_signer_deterministic() {
        let a = HmacFnvSigner::new(42).sign(b"payload");
        let b = HmacFnvSigner::new(42).sign(b"payload");
        assert_eq!(a, b);
    }
}
