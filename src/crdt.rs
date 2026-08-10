//! CRDT Memory Sync (port de `k_ai::sgdb::crdt_sync`, ADR-0081 C4).
//!
//! Replica o estado de memória (versões) entre nós via
//! Conflict-free Replicated Data Type: **last-writer-wins** — cada nó publica
//! sua versão local; versões maiores que a local são adotadas (merge).
//!
//! ## Diferenças vs kernel (honestidade)
//! - O kernel usa o mesh P2P assinado (`k_nano::net::udp_broadcast`, Fase A
//!   fail-closed, roles Master/Worker). Aqui o transporte é uma trait
//!   plugável (`Transport`): o merge é simétrico (todo nó publica e adota LWW).
//! - Wire format próprio: `[node_id u8][version u64 LE]` (9 bytes) — sem
//!   assinatura. **Em produção, use um transporte autenticado** (TLS/UDP com
//!   assinatura) — o `UdpTransport` entregue é demonstração/desenvolvimento.
//!
//! ## Fallback local
//! Sem transporte ativo, `CrdtMemorySync` opera localmente (nada é enviado) —
//! comportamento padrão, igual ao kernel.

use alloc::vec::Vec;

use crate::storage::SgdbError;

/// Intervalo mínimo entre syncs (unidades do relógio do caller, ex: ticks).
const SYNC_INTERVAL: u64 = 200;

/// Porta P2P default do transporte UDP (espelha o mesh do kernel).
pub const DEFAULT_P2P_PORT: u16 = 42069;

/// Transporte plugável de memórias entre nós.
///
/// Implemente para o seu meio (UDP, TCP, serial, IPC). Semântica esperada:
/// `send_crdt` entrega a versão local a peers; `recv_crdt` devolve as versões
/// recebidas desde a última chamada (o crate aplica LWW).
pub trait Transport {
    fn send_crdt(&mut self, node_id: u8, version: u64) -> Result<(), SgdbError>;
    fn recv_crdt(&mut self) -> Vec<(u8, u64)>;
}

/// Agente CRDT de sincronização de versões de memória.
///
/// Mantém versão local e versões conhecidas de outros nós. `sync()` troca
/// versões com os pares periodicamente (rate-limited); conflitos resolvidos
/// por last-writer-wins.
pub struct CrdtMemorySync {
    /// node_id local (vector clock / origem).
    node_id: u8,
    /// Versão monotônica local — incrementada a cada `record_change()`.
    local_version: u64,
    /// Versões conhecidas de outros nós: (node_id, version).
    pub node_versions: Vec<(u8, u64)>,
    /// Último `now` em que sync foi executado (rate-limit); `None` = nunca.
    last_sync_at: Option<u64>,
    /// true quando ao menos um sync real aconteceu.
    pub active: bool,
}

impl CrdtMemorySync {
    pub const fn new(node_id: u8) -> Self {
        Self {
            node_id,
            local_version: 0,
            node_versions: Vec::new(),
            last_sync_at: None,
            active: false,
        }
    }

    /// Versão local atual.
    pub fn local_version(&self) -> u64 {
        self.local_version
    }

    /// Marca uma mutação no banco local — incrementa a versão.
    /// Chamar após cada escrita (remember_*, put, checkpoint).
    pub fn record_change(&mut self) {
        self.local_version = self.local_version.saturating_add(1);
    }

    /// Sincroniza versões com peers via transporte.
    ///
    /// 1. RX: aplica versões recebidas — LWW (maior vence), registra em
    ///    `node_versions`.
    /// 2. TX: rate-limited por `SYNC_INTERVAL` (unidades de `now`) — publica a
    ///    versão local.
    ///
    /// Sem transporte ativo (nenhum peer envia), opera localmente — fallback.
    pub fn sync(&mut self, now: u64, transport: &mut dyn Transport) -> Result<(), SgdbError> {
        // (1) RX — sempre aplica (LWW)
        for (node, v) in transport.recv_crdt() {
            self.upsert_peer_version(node, v);
            if v > self.local_version {
                crate::sgdb_log!(
                    "CRDT sync: local_v={} -> node={} v={} merged",
                    self.local_version,
                    node,
                    v
                );
                self.local_version = v;
            }
        }

        // (2) TX — rate-limit
        if let Some(last) = self.last_sync_at {
            if now.wrapping_sub(last) < SYNC_INTERVAL {
                return Ok(());
            }
        }
        self.last_sync_at = Some(now);
        self.active = true;
        transport.send_crdt(self.node_id, self.local_version)
    }

    /// Insere/atualiza a versão conhecida de um peer (dedupe por node_id).
    fn upsert_peer_version(&mut self, node: u8, v: u64) {
        if let Some(slot) = self.node_versions.iter_mut().find(|(n, _)| *n == node) {
            slot.1 = v;
        } else {
            self.node_versions.push((node, v));
        }
    }
}

/// Transporte UDP broadcast (`std`) — demonstração/desenvolvimento.
///
/// Wire format: `[node_id u8][version u64 LE]` (9 bytes) broadcast na porta.
/// **Não autenticado** — substituir por transporte assinado em produção.
#[cfg(feature = "std")]
pub struct UdpTransport {
    socket: std::net::UdpSocket,
    port: u16,
    buf: [u8; 2048],
}

#[cfg(feature = "std")]
impl UdpTransport {
    /// Bind local na porta + habilita broadcast.
    pub fn new(port: u16) -> std::io::Result<Self> {
        let socket = std::net::UdpSocket::bind(("0.0.0.0", port))?;
        socket.set_broadcast(true)?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            port,
            buf: [0u8; 2048],
        })
    }

    /// Conveniência: porta default `DEFAULT_P2P_PORT`.
    pub fn new_default() -> std::io::Result<Self> {
        Self::new(DEFAULT_P2P_PORT)
    }
}

#[cfg(feature = "std")]
impl Transport for UdpTransport {
    fn send_crdt(&mut self, node_id: u8, version: u64) -> Result<(), SgdbError> {
        let mut buf = Vec::with_capacity(9);
        buf.push(node_id);
        buf.extend_from_slice(&version.to_le_bytes());
        self.socket
            .send_to(&buf, ("255.255.255.255", self.port))
            .map(|_| ())
            .map_err(|_| SgdbError::Storage("udp send"))
    }

    fn recv_crdt(&mut self) -> Vec<(u8, u64)> {
        let mut out = Vec::new();
        loop {
            match self.socket.recv(&mut self.buf) {
                Ok(n) if n >= 9 => {
                    let node = self.buf[0];
                    let v = u64::from_le_bytes(self.buf[1..9].try_into().unwrap());
                    out.push((node, v));
                }
                Ok(_) => continue,
                Err(_) => break, // WouldBlock / erro → drena o que veio
            }
        }
        out
    }
}

/// Self-test (port do `demo()` do kernel): criação, record, fallback local.
pub fn demo() -> bool {
    let mut sync = CrdtMemorySync::new(1);
    if sync.active || sync.local_version != 0 {
        return false;
    }
    sync.record_change();
    if sync.local_version != 1 {
        return false;
    }
    sync.record_change();
    if sync.local_version != 2 {
        return false;
    }
    if sync.local_version() != 2 {
        return false;
    }
    if !sync.node_versions.is_empty() {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Loopback: `sent` acumula TX e `recv_crdt` drena — simula rede
    /// conectando a fila de um nó à entrada do outro.
    #[derive(Default)]
    struct LoopTransport {
        sent: Vec<(u8, u64)>,
    }

    impl LoopTransport {
        fn take_sent(&mut self) -> Vec<(u8, u64)> {
            core::mem::take(&mut self.sent)
        }
        fn from(pkts: Vec<(u8, u64)>) -> Self {
            Self { sent: pkts }
        }
    }

    impl Transport for LoopTransport {
        fn send_crdt(&mut self, node: u8, v: u64) -> Result<(), SgdbError> {
            self.sent.push((node, v));
            Ok(())
        }
        fn recv_crdt(&mut self) -> Vec<(u8, u64)> {
            core::mem::take(&mut self.sent)
        }
    }

    #[test]
    fn demo_ok() {
        assert!(demo());
    }

    #[test]
    fn lww_merge_two_nodes() {
        let mut a = CrdtMemorySync::new(1);
        let mut b = CrdtMemorySync::new(2);
        a.record_change();
        a.record_change(); // a = 2
        b.record_change(); // b = 1

        let mut ta = LoopTransport::default();
        let mut tb = LoopTransport::default();
        a.sync(0, &mut ta).unwrap();
        b.sync(0, &mut tb).unwrap();
        assert_eq!(a.local_version(), 2);
        assert_eq!(b.local_version(), 1);

        // "rede": entrega o que cada um publicou ao outro
        let from_a = ta.take_sent(); // a publicou (1, 2)
        let from_b = tb.take_sent(); // b publicou (2, 1)
        let mut ta2 = LoopTransport::from(from_b);
        let mut tb2 = LoopTransport::from(from_a);
        a.sync(200, &mut ta2).unwrap();
        b.sync(200, &mut tb2).unwrap();

        // LWW: a mantém 2 (v=1 não supera), b adota 2
        assert_eq!(a.local_version(), 2);
        assert_eq!(b.local_version(), 2);
        assert_eq!(a.node_versions, vec![(2, 1)]);
        assert_eq!(b.node_versions, vec![(1, 2)]);
    }

    #[test]
    fn local_fallback_no_peers() {
        let mut a = CrdtMemorySync::new(7);
        a.record_change();
        let mut t = LoopTransport::default();
        a.sync(0, &mut t).unwrap();
        // sem peers: publica mas nada é recebido; versão local preservada
        assert_eq!(a.local_version(), 1);
        assert!(a.active);
        assert!(a.node_versions.is_empty());
    }

    #[test]
    fn rate_limit() {
        let mut a = CrdtMemorySync::new(1);
        let mut t = LoopTransport::default();
        a.sync(0, &mut t).unwrap();
        assert_eq!(t.take_sent().len(), 1);
        // dentro da janela → não re-publica
        a.sync(50, &mut t).unwrap();
        assert_eq!(t.take_sent().len(), 0);
        // fora da janela → publica de novo
        a.sync(200, &mut t).unwrap();
        assert_eq!(t.take_sent().len(), 1);
    }
}
