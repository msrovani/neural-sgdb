//! CRDT Memory Sync (port of `k_ai::sgdb::crdt_sync`, ADR-0081 C4).
//!
//! Replicates memory state (versions) between nodes via
//! Conflict-free Replicated Data Type: **last-writer-wins** — each node
//! publishes its local version; versions higher than local are adopted (merge).
//!
//! ## Differences vs the kernel (honesty)
//! - The kernel uses the signed P2P mesh (`k_nano::net::udp_broadcast`, Phase A
//!   fail-closed, Master/Worker roles). Here the transport is a pluggable trait
//!   (`Transport`): the merge is symmetric (every node publishes and adopts LWW).
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

/// Uma versão de memória conhecida — o que o protocolo de version-sync troca
/// hoje. **NÃO é transferência de memória** (ver `MemoryDelta`/`MemorySnapshot`
/// e Doc 04 §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryVersion {
    pub node_id: u8,
    pub version: u64,
}

/// Delta de memória (futuro — Doc 04 §4): versões base + documentos NMD1.
/// Abstração limpa do protocolo de replicação; NÃO implementado nesta sprint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryDelta {
    pub base: Vec<MemoryVersion>,
    pub docs: Vec<Vec<u8>>, // MemoryDoc encoded (NMD1)
}

/// Snapshot completo de memória (futuro — Doc 04 §4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemorySnapshot {
    pub versions: Vec<MemoryVersion>,
    pub docs: Vec<Vec<u8>>,
}

/// Veredicto de merge de uma versão recebida (observável — o caller decide
/// log/ação; o CRDT nunca descarta versões concorrentes silenciosamente).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeVerdict {
    /// Eco do próprio broadcast (`node_id == local`) — ignorado.
    SelfPacket,
    /// Versão antiga já conhecida — ignorada (sem regressão).
    Stale,
    /// Versão idêntica já conhecida — ignorada (duplicata).
    Duplicate,
    /// Versão nova — adotada (convergência de estado, LWW p/ L4/L5/L7).
    Applied,
    /// Versão concorrente com estado local — **PRESERVADA em `conflicts`**
    /// (nunca resolvida por LWW cego; camada superior decide).
    Conflict,
}

/// Transporte plugável de memórias entre nós.
///
/// Implemente para o seu meio (UDP, TCP, serial, IPC). Semântica esperada:
/// `send_crdt` entrega a versão local a peers; `recv_crdt` devolve as versões
/// recebidas desde a última chamada (o crate aplica o merge por veredicto).
pub trait Transport {
    fn send_crdt(&mut self, node_id: u8, version: u64) -> Result<(), SgdbError>;
    fn recv_crdt(&mut self) -> Vec<(u8, u64)>;
}

/// Agente CRDT de sincronização de versões de memória.
///
/// Keeps the local version and versions known from other nodes. `sync()`
/// exchanges versions with peers periodically (rate-limited). Merge policy:
/// - **Estado** (L4/L5/L7): LWW — maior versão vence (convergência).
/// - **Memória concorrente**: versões de peers que não dominam o estado local
///   são **preservadas em `conflicts`** — nunca descartadas por LWW cego.
/// - **Self-packet**: o eco do próprio broadcast é ignorado.
pub struct CrdtMemorySync {
    /// node_id local (vector clock / origem).
    node_id: u8,
    /// Versão monotônica local — incrementada a cada `record_change()`.
    local_version: u64,
    /// Versões conhecidas de outros nós: (node_id, version).
    pub node_versions: Vec<(u8, u64)>,
    /// Versões concorrentes preservadas (memória que LWW cego apagaria).
    /// Expostas para a camada superior resolver (multi-value, Doc 04 §2).
    pub conflicts: Vec<MemoryVersion>,
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
            conflicts: Vec::new(),
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

    /// Aplica uma versão recebida com veredicto explícito. Núcleo do merge.
    ///
    /// Regras:
    /// 1. `node == local` → `SelfPacket` (eco do próprio broadcast).
    /// 2. versão conhecida ≥ v → `Stale`/`Duplicate` (sem regressão).
    /// 3. v novo E o nó já tinha versão conhecida (domina a própria) → `Applied`
    ///    se nenhum outro nó/estado local existe; senão **`Conflict`**.
    /// 4. conflito: registra em `node_versions` E `conflicts` — nunca descarta.
    pub fn apply_remote_version(&mut self, node: u8, v: u64) -> MergeVerdict {
        if node == self.node_id {
            return MergeVerdict::SelfPacket;
        }
        let known = self.node_versions.iter().find(|(n, _)| *n == node).map(|(_, k)| *k);
        match known {
            Some(k) if v < k => return MergeVerdict::Stale,
            Some(k) if v == k => return MergeVerdict::Duplicate,
            _ => {}
        }
        // v é novo para este nó. Há estado local/peer independente?
        let has_other_state = self.local_version > 0 || self.node_versions.len() > 1
            || (self.node_versions.len() == 1 && !self.node_versions.iter().any(|(n, _)| *n == node));
        self.upsert_peer_version(node, v);
        if has_other_state {
            // versão de peer não causada pelo nosso estado → concorrente
            if !self.conflicts.iter().any(|c| c.node_id == node && c.version == v) {
                self.conflicts.push(MemoryVersion { node_id: node, version: v });
            }
            MergeVerdict::Conflict
        } else {
            // primeiro conhecimento (estado vazio) → adoção
            if v > self.local_version {
                self.local_version = v;
            }
            MergeVerdict::Applied
        }
    }

    /// Sincroniza versões com peers via transporte.
    ///
    /// 1. RX: aplica versões recebidas via `apply_remote_version` — veredicto
    ///    logado; conflitos preservados em `conflicts`.
    /// 2. TX: rate-limited por `SYNC_INTERVAL` (unidades de `now`) — publica a
    ///    versão local.
    ///
    /// Sem transporte ativo (nenhum peer envia), opera localmente — fallback.
    pub fn sync(&mut self, now: u64, transport: &mut dyn Transport) -> Result<(), SgdbError> {
        // (1) RX — merge por veredicto
        for (node, v) in transport.recv_crdt() {
            match self.apply_remote_version(node, v) {
                MergeVerdict::SelfPacket => {
                    crate::sgdb_log!("CRDT sync: self-packet node={node} ignorado");
                }
                MergeVerdict::Stale => {
                    crate::sgdb_log!("CRDT sync: node={node} v={v} stale (regressao bloqueada)");
                }
                MergeVerdict::Duplicate => {}
                MergeVerdict::Applied => {
                    crate::sgdb_log!("CRDT sync: node={node} v={v} applied (estado LWW)");
                }
                MergeVerdict::Conflict => {
                    crate::sgdb_log!(
                        "CRDT sync: node={node} v={v} CONFLITO preservado (concorrente)",
                    );
                }
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
                    // parsing safety (maturation P2): leitura checada, sem unwrap
                    let node = self.buf[0];
                    let mut vb = [0u8; 8];
                    vb.copy_from_slice(&self.buf[1..9]);
                    let v = u64::from_le_bytes(vb);
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
    fn concurrent_writes_preserved() {
        // Duas escritas independentes (A e B) são CONCORRENTES — a semântica
        // multi-value preserva ambas; LWW cego (antigo) as apagaria.
        let mut a = CrdtMemorySync::new(1);
        let mut b = CrdtMemorySync::new(2);
        a.record_change();
        a.record_change(); // a = 2 (escritas locais de A)
        b.record_change(); // b = 1 (escrita local de B)

        let mut ta = LoopTransport::default();
        let mut tb = LoopTransport::default();
        a.sync(0, &mut ta).unwrap();
        b.sync(0, &mut tb).unwrap();

        // "rede": entrega o que cada um publicou ao outro
        let from_a = ta.take_sent(); // a publicou (1, 2)
        let from_b = tb.take_sent(); // b publicou (2, 1)
        let mut ta2 = LoopTransport::from(from_b);
        let mut tb2 = LoopTransport::from(from_a);

        // Verdicto do merge para cada lado
        assert_eq!(a.apply_remote_version(2, 1), MergeVerdict::Conflict);
        assert_eq!(b.apply_remote_version(1, 2), MergeVerdict::Conflict);

        // Cada nó mantém o próprio contador (não conflado com o do peer)
        assert_eq!(a.local_version(), 2);
        assert_eq!(b.local_version(), 1);

        // Convergência de node_versions (máximo por nó) + conflitos preservados
        assert_eq!(a.node_versions, vec![(2, 1)]);
        assert_eq!(b.node_versions, vec![(1, 2)]);
        assert_eq!(a.conflicts, vec![MemoryVersion { node_id: 2, version: 1 }]);
        assert_eq!(b.conflicts, vec![MemoryVersion { node_id: 1, version: 2 }]);
    }

    #[test]
    fn self_packet_ignored() {
        // Eco do próprio broadcast (UDP broadcast devolve ao emissor)
        let mut a = CrdtMemorySync::new(7);
        a.record_change(); // a = 1
        let mut t = LoopTransport::from(vec![(7, 1)]); // self packet
        assert_eq!(a.apply_remote_version(7, 1), MergeVerdict::SelfPacket);
        a.sync(0, &mut t).unwrap();
        assert!(a.node_versions.is_empty()); // self nunca vira peer
        assert!(a.conflicts.is_empty());
        assert_eq!(a.local_version(), 1); // inalterado
    }

    #[test]
    fn older_and_duplicate_ignored() {
        let mut a = CrdtMemorySync::new(1);
        a.apply_remote_version(2, 5); // aplicado (primeiro conhecimento)
        assert_eq!(a.node_versions, vec![(2, 5)]);
        // versão mais velha → stale (sem regressão)
        assert_eq!(a.apply_remote_version(2, 3), MergeVerdict::Stale);
        assert_eq!(a.node_versions, vec![(2, 5)]);
        // duplicata → ignorada
        assert_eq!(a.apply_remote_version(2, 5), MergeVerdict::Duplicate);
        assert_eq!(a.node_versions, vec![(2, 5)]);
    }

    #[test]
    fn fresh_node_adopts_remote() {
        // Nó sem estado local adota a versão do peer (convergência de estado)
        let mut a = CrdtMemorySync::new(1);
        assert_eq!(a.apply_remote_version(2, 4), MergeVerdict::Applied);
        assert_eq!(a.node_versions, vec![(2, 4)]);
        assert!(a.conflicts.is_empty()); // sem estado local → sem conflito
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
