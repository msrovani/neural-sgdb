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

use crate::memory_doc::{MemoryLayer, MemoryRecord};
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

/// Delta de memória (v0.6 — P0-5, Doc 04 §4): versões BASE que o remetente
/// assume que o receptor já conhece e `records` completos (doc NMD1 + estado
/// e validade) para as versões faltantes. Um delta carrega o necessário para
/// o merge causal correto — um número de versão sozinho NÃO basta.
///
/// Wire "MDLT" v1: `magic | ver u8 | nbase u16 (node u8 + version u64)* |
/// nrec u16 (len u32 + MemoryRecord)*`. `decode` nunca panics (parsing
/// safety — fuzz-tested).
#[derive(Clone, Debug, PartialEq)]
pub struct MemoryDelta {
    pub base: Vec<MemoryVersion>,
    pub records: Vec<MemoryRecord>,
}

impl MemoryDelta {
    /// Encode truncating-seguro (P1-2): valida counts e lengths antes de
    /// qualquer cast — um campo que não cabe no wire retorna `Err`.
    pub fn try_encode(&self) -> Result<Vec<u8>, &'static str> {
        let mut out = Vec::new();
        out.extend_from_slice(DELTA_MAGIC);
        out.push(1); // versão do formato
        encode_versions(&mut out, &self.base)?;
        encode_records(&mut out, &self.records)?;
        Ok(out)
    }

    pub fn encode(&self) -> Vec<u8> {
        self.try_encode().expect("delta wire overflow")
    }

    pub fn decode(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 5 || &data[0..4] != DELTA_MAGIC {
            return Err("bad delta magic");
        }
        if data[4] != 1 {
            return Err("bad delta version");
        }
        let mut off = 5;
        let base = decode_versions(data, &mut off).ok_or("trunc delta base")?;
        let records = decode_records(data, &mut off).ok_or("trunc delta records")?;
        Ok(MemoryDelta { base, records })
    }
}

/// Snapshot completo de memória (v0.6 — P0-5): estado conhecido (versões por
/// nó) + todos os records. Usado para bootstrap de nó novo / restauração.
///
/// Wire "MSNP" v1: `magic | ver u8 | nver u16 (node u8 + version u64)* |
/// nrec u16 (len u32 + MemoryRecord)*`. `decode` nunca panics.
#[derive(Clone, Debug, PartialEq)]
pub struct MemorySnapshot {
    pub versions: Vec<MemoryVersion>,
    pub records: Vec<MemoryRecord>,
}

impl MemorySnapshot {
    /// Encode truncating-seguro (P1-2) — ver [`MemoryDelta::try_encode`].
    pub fn try_encode(&self) -> Result<Vec<u8>, &'static str> {
        let mut out = Vec::new();
        out.extend_from_slice(SNAP_MAGIC);
        out.push(1);
        encode_versions(&mut out, &self.versions)?;
        encode_records(&mut out, &self.records)?;
        Ok(out)
    }

    pub fn encode(&self) -> Vec<u8> {
        self.try_encode().expect("snapshot wire overflow")
    }

    pub fn decode(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 5 || &data[0..4] != SNAP_MAGIC {
            return Err("bad snapshot magic");
        }
        if data[4] != 1 {
            return Err("bad snapshot version");
        }
        let mut off = 5;
        let versions = decode_versions(data, &mut off).ok_or("trunc snap versions")?;
        let records = decode_records(data, &mut off).ok_or("trunc snap records")?;
        Ok(MemorySnapshot { versions, records })
    }
}

const DELTA_MAGIC: &[u8; 4] = b"MDLT";
const SNAP_MAGIC: &[u8; 4] = b"MSNP";

fn encode_versions(out: &mut Vec<u8>, vs: &[MemoryVersion]) -> Result<(), &'static str> {
    if vs.len() > u16::MAX as usize {
        return Err("versions too many");
    }
    out.extend_from_slice(&(vs.len() as u16).to_le_bytes());
    for v in vs {
        out.push(v.node_id);
        out.extend_from_slice(&v.version.to_le_bytes());
    }
    Ok(())
}

/// Bounds-checked — `None` em truncado (nunca panics).
fn decode_versions(data: &[u8], off: &mut usize) -> Option<Vec<MemoryVersion>> {
    let n = rd_u16(data, *off)? as usize;
    *off += 2;
    let mut vs = Vec::with_capacity(n.min(4096));
    for _ in 0..n {
        let node = *data.get(*off)?;
        *off += 1;
        let b = data.get(*off..off.checked_add(8)?)?;
        let version = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
        *off += 8;
        vs.push(MemoryVersion { node_id: node, version });
    }
    Some(vs)
}

fn encode_records(out: &mut Vec<u8>, recs: &[MemoryRecord]) -> Result<(), &'static str> {
    if recs.len() > u16::MAX as usize {
        return Err("records too many");
    }
    out.extend_from_slice(&(recs.len() as u16).to_le_bytes());
    for r in recs {
        let enc = r.encode();
        if enc.len() > u32::MAX as usize {
            return Err("record too long");
        }
        out.extend_from_slice(&(enc.len() as u32).to_le_bytes());
        out.extend_from_slice(&enc);
    }
    Ok(())
}

/// Bounds-checked — `None` em truncado/inválido (nunca panics).
fn decode_records(data: &[u8], off: &mut usize) -> Option<Vec<MemoryRecord>> {
    let n = rd_u16(data, *off)? as usize;
    *off += 2;
    let mut recs = Vec::with_capacity(n.min(4096));
    for _ in 0..n {
        let len = rd_u32(data, *off)? as usize;
        *off += 4;
        let body = data.get(*off..off.checked_add(len)?)?;
        *off += len;
        recs.push(MemoryRecord::decode(body).ok()?);
    }
    Some(recs)
}

fn rd_u16(data: &[u8], off: usize) -> Option<u16> {
    let b = data.get(off..off.checked_add(2)?)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

fn rd_u32(data: &[u8], off: usize) -> Option<u32> {
    let b = data.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
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
    /// Política da CAMADA bloqueou a adoção (L0/L1 local-only, L6 reservado —
    /// P0-6). Nada é escrito nem registrado como conflito.
    Rejected,
}

/// Política de merge POR CAMADA (v0.6 — P0-6). Tabela explícita em código:
/// NÃO existe uma regra LWW universal para todas as camadas (item 8).
///
/// | Camada | Política | Semântica |
/// |---|---|---|
/// | L0 Sensory | `LocalOnly` | estritamente local — remota nunca adotada |
/// | L1 Working | `LocalWorking` | memória de trabalho local |
/// | L2/L3 Episódico | `MultiValueRegister` | concorrentes → AMBAS preservadas |
/// | L4 Semântico | `CausalLwwWithHistory` | dominante causal ativa; antiga superseded |
/// | L5 Procedural | `ControlledLww` | LWW com histórico e salvaguardas |
/// | L6 Reservado | `Reserved` | sem semântica de merge definida |
/// | L7 Identity | `ControlledLww` | identidade controlada |
///
/// Regra central: para L2/L3, escritas concorrentes são AMBAS retidas
/// (multi-value register, nunca LWW cego); para L4/L5/L7 a asserção
/// nova/dominante vira ativa e a antiga fica `Superseded` — mas NUNCA é
/// descartada silenciosamente. A política é consultada no merge path
/// (`apply_remote_version_with_policy`, `Sgdb::merge_remote`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergePolicy {
    LocalOnly,
    LocalWorking,
    MultiValueRegister,
    CausalLwwWithHistory,
    ControlledLww,
    Reserved,
}

impl MergePolicy {
    /// Tabela camada → política (ponto único de verdade).
    pub fn for_layer(layer: MemoryLayer) -> Self {
        match layer {
            MemoryLayer::L0Sensory => Self::LocalOnly,
            MemoryLayer::L1Working => Self::LocalWorking,
            MemoryLayer::L2EpisodicShort | MemoryLayer::L3EpisodicLong => Self::MultiValueRegister,
            MemoryLayer::L4Semantic => Self::CausalLwwWithHistory,
            MemoryLayer::L5Procedural | MemoryLayer::L7Identity => Self::ControlledLww,
            MemoryLayer::L6Reserved => Self::Reserved,
        }
    }

    /// Camadas que aceitam versões/records remotos (L0/L1 são estritamente
    /// locais; L6 não tem semântica definida).
    pub fn accepts_remote(&self) -> bool {
        !matches!(self, Self::LocalOnly | Self::LocalWorking | Self::Reserved)
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::LocalOnly => "L0 sensory: strictly local, remote never adopted",
            Self::LocalWorking => "L1 working: local-only working memory",
            Self::MultiValueRegister => {
                "L2/L3 episodic: concurrent versions both retained (multi-value)"
            }
            Self::CausalLwwWithHistory => {
                "L4 semantic: causally-dominant wins, older superseded with history"
            }
            Self::ControlledLww => {
                "L5/L7 procedural/identity: LWW with safeguards, history preserved"
            }
            Self::Reserved => "L6 reserved: no merge semantics defined",
        }
    }
}

/// Transporte plugável de memórias entre nós.
///
/// Implemente para o seu meio (UDP, TCP, serial, IPC). Semântica esperada:
/// `send_crdt` entrega a versão local a peers; `recv_crdt` devolve as versões
/// recebidas desde a última chamada (o crate aplica o merge por veredicto).
pub trait Transport {
    fn send_crdt(&mut self, node_id: u8, version: u64) -> Result<(), SgdbError>;
    fn recv_crdt(&mut self) -> Vec<(u8, u64)>;

    /// #10 (δ-CRDT): envia um DELTA (versão + payload opcional). Default cai
    /// para `send_crdt` (payload ignorado) — transportes que não suportam
    /// deltas continuam funcionando com a semântica antiga.
    fn send_delta(&mut self, node_id: u8, version: u64, payload: &[u8]) -> Result<(), SgdbError> {
        let _ = payload;
        self.send_crdt(node_id, version)
    }
}

/// Envelope de transporte **autenticável** (v0.2 — fronteira de segurança
/// explícita, item 12).
///
/// `payload` + `node_id` (identidade de origem) + `auth` (assinatura/HMAC
/// **opaca**). O core NÃO implementa criptografia: `auth` é preenchido e
/// verificado pelo TRANSPORTE (HMAC compartilhado, ed25519, TLS — fora deste
/// crate). Usar este envelope dá um formato de fio determinístico e
/// bounds-checked para carregar autenticação SEM acoplar o core a uma lib de
/// criptografia.
///
/// Wire: `[node_id u8][plen u32le][alen u32le][payload][auth]`. `decode`
/// nunca panics em entrada malformada/truncada (maturation P6) — retorna None.
///
/// ⚠️ O `UdpTransport` entregue é DEMO NÃO autenticado e NÃO usa este
/// envelope; em produção use um transporte que preencha/verifique `auth` e
/// rejeite pacotes inválidos.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedEnvelope {
    /// Identidade do nó de origem (nunca confundida com memory_id — item 20).
    pub node_id: u8,
    /// Payload do protocolo (ex: versão CRDT, doc NMD1, delta).
    pub payload: Vec<u8>,
    /// Autenticação opaca: assinatura/HMAC sobre o payload (verificado pelo
    /// transporte, não pelo core).
    pub auth: Vec<u8>,
}

impl SignedEnvelope {
    pub fn new(node_id: u8, payload: Vec<u8>, auth: Vec<u8>) -> Self {
        SignedEnvelope {
            node_id,
            payload,
            auth,
        }
    }

    /// Encode truncating-seguro (P1-2): payload/auth > u32::MAX → `Err`.
    pub fn try_encode(&self) -> Result<Vec<u8>, &'static str> {
        if self.payload.len() > u32::MAX as usize || self.auth.len() > u32::MAX as usize {
            return Err("envelope field overflow");
        }
        let mut out = Vec::with_capacity(9 + self.payload.len() + self.auth.len());
        out.push(self.node_id);
        out.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.auth.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.payload);
        out.extend_from_slice(&self.auth);
        Ok(out)
    }

    pub fn encode(&self) -> Vec<u8> {
        self.try_encode().expect("envelope wire overflow")
    }

    /// Bounds-checked: `None` em input truncado/corrompido — nunca panics.
    /// Retorna (envelope, bytes consumidos) para streams com múltiplos pacotes.
    pub fn decode(data: &[u8]) -> Option<(Self, usize)> {
        if data.len() < 9 {
            return None;
        }
        let node_id = data[0];
        let plen = u32::from_le_bytes(data[1..5].try_into().ok()?) as usize;
        let alen = u32::from_le_bytes(data[5..9].try_into().ok()?) as usize;
        let end = 9usize.checked_add(plen)?.checked_add(alen)?;
        if end > data.len() {
            return None;
        }
        Some((
            SignedEnvelope {
                node_id,
                payload: data[9..9 + plen].to_vec(),
                auth: data[9 + plen..end].to_vec(),
            },
            end,
        ))
    }
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
    /// Escritas PRÓPRIAS (nunca adotadas de peers) — a base de "estado
    /// independente" para detecção de concorrência. Sem isto, um `local_version`
    /// adotado de um peer (Applied) faria o sucessor causal do MESMO peer virar
    /// Conflict para sempre e o mesh nunca convergiria (review P6, MED #2).
    own_writes: u64,
    /// Versões conhecidas de outros nós: (node_id, version).
    pub node_versions: Vec<(u8, u64)>,
    /// Versões concorrentes preservadas (memória que LWW cego apagaria).
    /// Expostas para a camada superior resolver (multi-value, Doc 04 §2).
    pub conflicts: Vec<MemoryVersion>,
    /// #10: deltas locais ainda NÃO entregues (versões desde o último sync).
    /// Sync envia só o que cada peer ainda não viu (δ-CRDT).
    pending: Vec<u64>,
    /// Último `now` em que sync foi executado (rate-limit); `None` = nunca.
    last_sync_at: Option<u64>,
    /// true quando ao menos um sync real aconteceu.
    pub active: bool,
}

/// Estado durável do CRDT (P0-11, v0.7): o mínimo que um nó reiniciado
/// precisa para não regredir — node_id (identidade estável), contadores
/// locais e as versões conhecidas de outros nós. Wire "CRDT" v1:
/// `magic | ver u8 | node_id u8 | local u64le | own u64le | n u16le +
/// (node u8 + version u64le)*`. `decode` nunca panics (bounds-checked).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrdtState {
    pub node_id: u8,
    pub local_version: u64,
    pub own_writes: u64,
    pub node_versions: Vec<(u8, u64)>,
}

const CRDT_MAGIC: &[u8; 4] = b"CRDT";
const CRDT_VERSION: u8 = 1;

impl CrdtState {
    /// Encode truncating-seguro (P1-2): node_versions > u16::MAX → `Err`.
    pub fn try_encode(&self) -> Result<Vec<u8>, &'static str> {
        if self.node_versions.len() > u16::MAX as usize {
            return Err("crdt node_versions overflow");
        }
        let mut out = Vec::with_capacity(16 + self.node_versions.len() * 9);
        out.extend_from_slice(CRDT_MAGIC);
        out.push(CRDT_VERSION);
        out.push(self.node_id);
        out.extend_from_slice(&self.local_version.to_le_bytes());
        out.extend_from_slice(&self.own_writes.to_le_bytes());
        out.extend_from_slice(&(self.node_versions.len() as u16).to_le_bytes());
        for &(n, v) in &self.node_versions {
            out.push(n);
            out.extend_from_slice(&v.to_le_bytes());
        }
        Ok(out)
    }

    pub fn encode(&self) -> Vec<u8> {
        self.try_encode().expect("crdt wire overflow")
    }

    pub fn decode(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 15 || &data[0..4] != CRDT_MAGIC {
            return Err("bad crdt magic");
        }
        if data[4] != CRDT_VERSION {
            return Err("bad crdt version");
        }
        let node_id = data[5];
        let local_version = u64::from_le_bytes(
            data.get(6..14).ok_or("trunc crdt local")?.try_into().map_err(|_| "crdt local")?,
        );
        let own_writes = u64::from_le_bytes(
            data.get(14..22).ok_or("trunc crdt own")?.try_into().map_err(|_| "crdt own")?,
        );
        let mut off = 22;
        let n = rd_u16(data, off).ok_or("trunc crdt n")? as usize;
        off += 2;
        let mut node_versions = Vec::with_capacity(n.min(4096));
        for _ in 0..n {
            let node = *data.get(off).ok_or("trunc crdt node")?;
            off += 1;
            let end = off.checked_add(8).ok_or("crdt off")?;
            let b = data.get(off..end).ok_or("trunc crdt v")?;
            let v = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
            off += 8;
            node_versions.push((node, v));
        }
        Ok(CrdtState {
            node_id,
            local_version,
            own_writes,
            node_versions,
        })
    }
}

impl CrdtMemorySync {
    pub const fn new(node_id: u8) -> Self {
        Self {
            node_id,
            local_version: 0,
            own_writes: 0,
            node_versions: Vec::new(),
            conflicts: Vec::new(),
            pending: Vec::new(),
            last_sync_at: None,
            active: false,
        }
    }

    /// node_id local (identidade estável, nunca confundida com memory_id).
    pub fn node_id(&self) -> u8 {
        self.node_id
    }

    /// Clock completo conhecido (próprio + relayado) — anti-entropy de
    /// estado (P0-7): cada ronda anuncia o que SABE, não só as escritas
    /// próprias, para que versões atravessem nós intermediários (gossip).
    /// Ordem determinística: próprio primeiro, depois node_versions.
    pub fn announce(&self) -> Vec<(u8, u64)> {
        let mut out = Vec::with_capacity(self.node_versions.len() + 1);
        out.push((self.node_id, self.local_version));
        out.extend(self.node_versions.iter().copied());
        out
    }

    /// O que um peer deve informar como "o que eu conheço" num pedido de
    /// delta (`missing_after` espera o clock do peer).
    pub fn known_clock(&self) -> Vec<(u8, u64)> {
        self.announce()
    }

    /// Estado durável (P0-11): serializa node_id + contadores + versões
    /// conhecidas — um nó reiniciado restaura com `restore` e não regride.
    pub fn state(&self) -> CrdtState {
        CrdtState {
            node_id: self.node_id,
            local_version: self.local_version,
            own_writes: self.own_writes,
            node_versions: self.node_versions.clone(),
        }
    }

    /// Restaura estado durável. Retorna `false` (e NÃO altera nada) se o
    /// estado pertence a outro node_id — nunca adota identidade alheia.
    pub fn restore(&mut self, s: CrdtState) -> bool {
        if s.node_id != self.node_id {
            return false;
        }
        self.local_version = s.local_version;
        self.own_writes = s.own_writes;
        self.node_versions = s.node_versions;
        true
    }

    /// Versão local atual.
    pub fn local_version(&self) -> u64 {
        self.local_version
    }

    /// Número de escritas locais (estado independente — base da detecção de
    /// concorrência).
    pub fn own_writes(&self) -> u64 {
        self.own_writes
    }

    /// Marca uma mutação no banco local — incrementa a versão e registra o
    /// delta (#10). Chamar após cada escrita (remember_*, put, checkpoint).
    pub fn record_change(&mut self) {
        self.local_version = self.local_version.saturating_add(1);
        self.own_writes = self.own_writes.saturating_add(1);
        self.pending.push(self.local_version);
    }

    /// Deltas locais ainda não entregues (diagnóstico/medição, #10).
    pub fn pending_deltas(&self) -> usize {
        self.pending.len()
    }

    /// Aplica uma versão recebida sob a política da CAMADA do doc associado
    /// (P0-6). Camadas locais (L0/L1) e reservadas (L6) nunca adotam versão
    /// remota → `Rejected`; as demais seguem a detecção de concorrência
    /// padrão (`apply_remote_version`).
    pub fn apply_remote_version_with_policy(
        &mut self,
        node: u8,
        v: u64,
        policy: MergePolicy,
    ) -> MergeVerdict {
        if !policy.accepts_remote() {
            return MergeVerdict::Rejected;
        }
        self.apply_remote_version(node, v)
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
        if v == 0 {
            // versão 0 = "nada a sincronizar": um nó sem escritas locais não
            // tem versão causal a adotar nem a conflitar. Registrar (node, 0)
            // como estado de peer criaria conflitos FANTASMA (ex: um nó que
            // só faz relay/publica heartbeat vira "concorrente" de todos).
            return MergeVerdict::Duplicate;
        }
        let known = self.node_versions.iter().find(|(n, _)| *n == node).map(|(_, k)| *k);
        match known {
            Some(k) if v < k => return MergeVerdict::Stale,
            Some(k) if v == k => return MergeVerdict::Duplicate,
            _ => {}
        }
        // v é novo para este nó. Há ESTADO PRÓPRIO independente?
        // (review P6, MED #2: usa own_writes — escritas locais — e NÃO
        // local_version, que é adotado de peers no Applied; senão o sucessor
        // causal do mesmo peer vira Conflict para sempre e o mesh nunca
        // converge em estado estacionário)
        let has_other_state = self.own_writes > 0 || self.node_versions.len() > 1
            || (self.node_versions.len() == 1 && !self.node_versions.iter().any(|(n, _)| *n == node));
        self.upsert_peer_version(node, v);
        if has_other_state {
            // versão de peer não causada pelo nosso estado → concorrente
            if !self.conflicts.iter().any(|c| c.node_id == node && c.version == v) {
                self.conflicts.push(MemoryVersion { node_id: node, version: v });
            }
            MergeVerdict::Conflict
        } else {
            // primeiro conhecimento (estado vazio) → Applied, mas `local_version`
            // NUNCA adota a versão do peer: local_version = contador de ESCRITAS
            // PRÓPRIAS. Se um nó fresh adotasse, um relay publicaria (self, v)
            // como se fosse autoria — criando conflitos fantasma em todos
            // (ex: C recebe (1,1) de A, re-broadcasta (3,1), e A/B veem C
            // "concorrente"). O conhecimento do peer fica em `node_versions`.
            MergeVerdict::Applied
        }
    }

    /// Faixa causal FALTANTE para um peer (P0-5): versões (por nó) que
    /// `self` conhece e o peer (com `peer_versions`) ainda não — o que o
    /// receptor deve PEDIR num protocolo de delta. As versões de um nó são
    /// contíguas (`record_change` incrementa de 1), então o watermark por nó
    /// cobre a faixa `(conhecida_do_peer, watermark]`. Determinístico
    /// (ordenado por node_id).
    pub fn missing_after(&self, peer_versions: &[(u8, u64)]) -> Vec<MemoryVersion> {
        let known = |n: u8| -> u64 {
            peer_versions
                .iter()
                .find(|(nn, _)| *nn == n)
                .map(|(_, v)| *v)
                .unwrap_or(0)
        };
        let mut out: Vec<MemoryVersion> = Vec::new();
        for &(n, k) in &self.node_versions {
            if known(n) < k {
                out.push(MemoryVersion { node_id: n, version: k });
            }
        }
        if known(self.node_id) < self.local_version {
            out.push(MemoryVersion {
                node_id: self.node_id,
                version: self.local_version,
            });
        }
        out.sort_by_key(|v| (v.node_id, v.version));
        out
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
                MergeVerdict::Rejected => {
                    crate::sgdb_log!("CRDT sync: node={node} v={v} rejeitado (politica da camada)");
                }
            }
        }

        // (2) TX — rate-limit + δ-CRDT: envia SÓ os deltas que cada peer ainda
        // não viu (versão conhecida < delta), nunca a história completa.
        if let Some(last) = self.last_sync_at {
            if now.wrapping_sub(last) < SYNC_INTERVAL {
                return Ok(());
            }
        }
        self.last_sync_at = Some(now);
        self.active = true;
        let pending = core::mem::take(&mut self.pending);
        for &v in &pending {
            // precisa de delta se algum peer conhecido ainda não tem v, ou se
            // não conhecemos peers (broadcast inicial)
            let needs = self.node_versions.is_empty()
                || self.node_versions.iter().any(|(_, k)| *k < v);
            if needs {
                transport.send_delta(self.node_id, v, &[])?;
            }
        }
        // heartbeat agregado (paridade com a semântica antiga)
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

/// Transporte UDP broadcast (`std`) — **demonstração/desenvolvimento apenas**.
///
/// Wire format: `[node_id u8][version u64 LE]` (9 bytes) broadcast na porta.
/// **NÃO autenticado**: qualquer host na rede pode injetar versões. Em
/// produção substitua por um transporte autenticado que use o
/// [`SignedEnvelope`] (HMAC/ed25519/TLS — verificado fora do core) ou rejeite
/// pacotes inválidos na origem.
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
    fn delta_sync_sends_only_unseen() {
        // #10: com o peer já convergido até v2, um sync envia SÓ os deltas
        // > v2 (+heartbeat) — payload proporcional ao NÃO-visto, não à história.
        #[derive(Default)]
        struct Dt {
            delta_calls: usize,
            sent: Vec<(u8, u64)>,
        }
        impl Transport for Dt {
            fn send_crdt(&mut self, n: u8, v: u64) -> Result<(), SgdbError> {
                self.sent.push((n, v));
                Ok(())
            }
            fn send_delta(&mut self, n: u8, v: u64, _p: &[u8]) -> Result<(), SgdbError> {
                self.delta_calls += 1;
                self.sent.push((n, v));
                Ok(())
            }
            fn recv_crdt(&mut self) -> Vec<(u8, u64)> {
                Vec::new()
            }
        }

        // peer parcialmente convergido: conhece até v2
        let mut a = CrdtMemorySync::new(1);
        a.record_change(); // v1
        a.record_change(); // v2
        a.record_change(); // v3
        a.node_versions.push((2, 2));
        let mut t = Dt::default();
        a.sync(0, &mut t).unwrap();
        // medição: 3 deltas gravados, mas só v3 (não-visto pelo peer) é enviado
        let deltas: Vec<u64> = t.sent.iter().filter(|(n, _)| *n == 1).map(|(_, v)| *v).collect();
        assert_eq!(t.delta_calls, 1, "deveria enviar 1 delta (v3), não 3");
        assert!(deltas.contains(&3) && !deltas.contains(&1) && !deltas.contains(&2));
        // heartbeat agregado ainda vai (paridade)
        assert!(t.sent.iter().any(|(_, v)| *v == 3));

        // peer FRESCO (sem histórico): todos os deltas pendentes são enviados
        let mut b = CrdtMemorySync::new(1);
        b.record_change();
        b.record_change();
        let mut t2 = Dt::default();
        b.sync(0, &mut t2).unwrap();
        assert_eq!(t2.delta_calls, 2, "peer fresco recebe todos os deltas");
        // após o sync, pendentes limpos
        assert_eq!(b.pending_deltas(), 0);
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

        // "rede": cada um publicou (ta = (1,2), tb = (2,1)). O merge é
        // aplicado diretamente via apply_remote_version (o sync() com
        // loopback entregaria o mesmo pacote — verificado nos node_versions)
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

    /// Adversarial (maturation P6): `apply_remote_version` nunca panics com
    /// pacotes malformados; self/stale/duplicate são tratados com veredicto.
    #[test]
    fn apply_remote_version_never_panics() {
        let mut a = CrdtMemorySync::new(1);
        // self packet
        assert_eq!(a.apply_remote_version(1, u64::MAX), MergeVerdict::SelfPacket);
        // pacotes arbitrários em sequência adversarial — veredictos válidos
        for &(node, v) in &[(0, 0), (255, u64::MAX), (2, 0), (2, u64::MAX), (2, u64::MAX)] {
            let _ = a.apply_remote_version(node, v); // nunca panics
        }
        // node_versions coerente (sem self, sem regressão)
        assert!(!a.node_versions.iter().any(|(n, _)| *n == 1));
    }

    /// Serialização determinística (maturation P6): versões estáveis.
    #[test]
    fn memory_version_eq_deterministic() {
        let v1 = MemoryVersion { node_id: 2, version: 7 };
        let v2 = MemoryVersion { node_id: 2, version: 7 };
        let v3 = MemoryVersion { node_id: 2, version: 8 };
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
        let rec = MemoryRecord::new(
            crate::memory_doc::MemoryDoc::new(
                crate::memory_doc::MemoryLayer::L4Semantic,
                "k",
                vec![1, 2, 3, 4],
            ),
            crate::memory_doc::MemoryState::Active,
            None,
        );
        let d = MemoryDelta { base: vec![v1], records: vec![rec.clone()] };
        let d2 = MemoryDelta { base: vec![v2], records: vec![rec] };
        assert_eq!(d, d2); // igualdade determinística
    }

    // ── SignedEnvelope: fronteira de segurança explícita (item 12) ─────────

    #[test]
    fn signed_envelope_roundtrip_and_prefix() {
        let env = SignedEnvelope::new(3, b"payload".to_vec(), b"sig".to_vec());
        let enc = env.encode();
        let (dec, n) = SignedEnvelope::decode(&enc).unwrap();
        assert_eq!(n, enc.len());
        assert_eq!(dec, env);
        // payload/auth vazios são válidos (envelope mínimo = 9 bytes)
        let empty = SignedEnvelope::new(1, Vec::new(), Vec::new());
        let (d2, n2) = SignedEnvelope::decode(&empty.encode()).unwrap();
        assert_eq!(d2, empty);
        assert_eq!(n2, 9);
        // bytes sobrando após o envelope → retorna só o consumido (stream)
        let mut ext = enc.clone();
        ext.extend_from_slice(b"next");
        let (d3, n3) = SignedEnvelope::decode(&ext).unwrap();
        assert_eq!(d3, env);
        assert_eq!(n3, enc.len());
    }

    #[test]
    fn signed_envelope_never_panics_on_malformed() {
        let env = SignedEnvelope::new(1, vec![0xAB; 16], vec![0xCD; 8]);
        let enc = env.encode();
        for cut in 0..enc.len() {
            let _ = SignedEnvelope::decode(&enc[..cut]); // truncado em todo ponto
        }
        // lengths absurdos (u32::MAX) → None, nunca panic
        let mut bad = vec![0u8; 9];
        bad[1..5].copy_from_slice(&u32::MAX.to_le_bytes());
        bad[5..9].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(SignedEnvelope::decode(&bad).is_none());
        // LCG fuzz determinístico
        let mut state = 0x0DDB_1A5Eu64;
        for len in 0..64usize {
            let data: Vec<u8> = (0..len)
                .map(|_| {
                    state = state.wrapping_mul(1103515245).wrapping_add(12345);
                    ((state >> 32) & 0xFF) as u8
                })
                .collect();
            let _ = SignedEnvelope::decode(&data);
        }
    }

    // ── P0-5: MemoryDelta/MemorySnapshot codecs (v0.6) ─────────────────────

    fn sample_record() -> MemoryRecord {
        let mut doc = MemoryDoc::new(MemoryLayer::L4Semantic, "k", vec![1, 2, 3, 4]);
        doc.clock.tick(1);
        MemoryRecord::new(doc, MemoryState::Active, None)
    }

    #[test]
    fn delta_snapshot_roundtrip() {
        let d = MemoryDelta {
            base: vec![
                MemoryVersion { node_id: 1, version: 3 },
                MemoryVersion { node_id: 2, version: 1 },
            ],
            records: vec![sample_record()],
        };
        assert_eq!(MemoryDelta::decode(&d.encode()).unwrap(), d);
        let s = MemorySnapshot {
            versions: vec![MemoryVersion { node_id: 1, version: 3 }],
            records: vec![sample_record()],
        };
        assert_eq!(MemorySnapshot::decode(&s.encode()).unwrap(), s);
        // vazio também é válido (bootstrap de nó novo sem estado)
        let e = MemoryDelta { base: Vec::new(), records: Vec::new() };
        assert_eq!(MemoryDelta::decode(&e.encode()).unwrap(), e);
    }

    #[test]
    fn delta_snapshot_never_panics_on_malformed() {
        let d = MemoryDelta {
            base: vec![MemoryVersion { node_id: 1, version: 3 }],
            records: vec![sample_record()],
        };
        let enc = d.encode();
        // truncado em todo ponto
        for cut in 0..enc.len() {
            let _ = MemoryDelta::decode(&enc[..cut]);
        }
        // fuzz LCG determinístico
        let mut state = 0xC0FF_EE00_BAAD_F00Du64;
        for len in 0..96usize {
            for _ in 0..8 {
                let data: Vec<u8> = (0..len)
                    .map(|_| {
                        state = state.wrapping_mul(1103515245).wrapping_add(12345);
                        ((state >> 32) & 0xFF) as u8
                    })
                    .collect();
                let _ = MemoryDelta::decode(&data);
                let _ = MemorySnapshot::decode(&data);
            }
        }
        // magic errado / versão desconhecida
        let mut bad = enc.clone();
        bad[0] = b'X';
        assert!(MemoryDelta::decode(&bad).is_err());
        let mut bad2 = enc;
        bad2[4] = 9;
        assert!(MemoryDelta::decode(&bad2).is_err());
        assert!(MemoryDelta::decode(b"MDLT").is_err());
    }

    #[test]
    fn try_encode_rejects_wire_overflow() {
        // P1-2: campos que não cabem no wire → Err (nunca cast silencioso)
        // MemoryDelta: base/records > u16::MAX
        let v = MemoryVersion { node_id: 1, version: 3 };
        let many_vers = vec![v; u16::MAX as usize + 1];
        let d = MemoryDelta { base: many_vers, records: Vec::new() };
        assert!(d.try_encode().is_err());
        let many_recs = vec![sample_record(); u16::MAX as usize + 1];
        let d2 = MemoryDelta { base: Vec::new(), records: many_recs };
        assert!(d2.try_encode().is_err());
        // MemorySnapshot: versions > u16::MAX
        let s = MemorySnapshot { versions: vec![v; u16::MAX as usize + 1], records: Vec::new() };
        assert!(s.try_encode().is_err());
        // SignedEnvelope: payload > u32::MAX (impraticável alocar 4GiB no
        // teste — validação lógica: payload de 1 byte passa, erro só por len)
        let ok_env = SignedEnvelope::new(1, vec![1u8; 8], vec![2u8; 8]);
        assert!(ok_env.try_encode().is_ok());
        // CrdtState: node_versions > u16::MAX
        let st = CrdtState {
            node_id: 1,
            local_version: 0,
            own_writes: 0,
            node_versions: vec![(1, 1u64); u16::MAX as usize + 1],
        };
        assert!(st.try_encode().is_err());
    }

    // ── P0-5: missing_after — faixa causal faltante para um peer ──────────

    #[test]
    fn missing_after_requests_unseen_range() {
        let mut a = CrdtMemorySync::new(1);
        a.record_change(); // v1
        a.record_change(); // v2
        a.apply_remote_version(2, 4);
        a.apply_remote_version(3, 7);
        // peer fresco: tudo o que conhecemos está faltando
        assert_eq!(
            a.missing_after(&[]),
            vec![
                MemoryVersion { node_id: 1, version: 2 },
                MemoryVersion { node_id: 2, version: 4 },
                MemoryVersion { node_id: 3, version: 7 },
            ]
        );
        // peer parcial: conhece self até 1 e nó 2 até 4 → só os gaps
        assert_eq!(
            a.missing_after(&[(1, 1), (2, 4)]),
            vec![
                MemoryVersion { node_id: 1, version: 2 },
                MemoryVersion { node_id: 3, version: 7 },
            ]
        );
        // peer convergido → nada faltando
        assert!(a.missing_after(&[(1, 2), (2, 4), (3, 7)]).is_empty());
    }

    #[test]
    fn version_zero_is_ignored() {
        // nó sem escritas (heartbeat v=0) nunca vira peer state nem conflito
        let mut a = CrdtMemorySync::new(1);
        a.record_change(); // v1
        assert_eq!(a.apply_remote_version(2, 0), MergeVerdict::Duplicate);
        assert!(a.node_versions.is_empty());
        assert!(a.conflicts.is_empty());
        // mas v>0 é registrado normalmente
        assert_eq!(a.apply_remote_version(2, 1), MergeVerdict::Conflict);
        assert_eq!(a.node_versions, vec![(2, 1)]);
    }

    // ── P0-6: política de merge por camada ────────────────────────────────

    #[test]
    fn merge_policy_table() {
        use MemoryLayer::*;
        assert_eq!(MergePolicy::for_layer(L0Sensory), MergePolicy::LocalOnly);
        assert_eq!(MergePolicy::for_layer(L1Working), MergePolicy::LocalWorking);
        assert_eq!(MergePolicy::for_layer(L2EpisodicShort), MergePolicy::MultiValueRegister);
        assert_eq!(MergePolicy::for_layer(L3EpisodicLong), MergePolicy::MultiValueRegister);
        assert_eq!(MergePolicy::for_layer(L4Semantic), MergePolicy::CausalLwwWithHistory);
        assert_eq!(MergePolicy::for_layer(L5Procedural), MergePolicy::ControlledLww);
        assert_eq!(MergePolicy::for_layer(L7Identity), MergePolicy::ControlledLww);
        assert_eq!(MergePolicy::for_layer(L6Reserved), MergePolicy::Reserved);
        // aceitação remota
        assert!(!MergePolicy::LocalOnly.accepts_remote());
        assert!(!MergePolicy::LocalWorking.accepts_remote());
        assert!(!MergePolicy::Reserved.accepts_remote());
        for p in [
            MergePolicy::MultiValueRegister,
            MergePolicy::CausalLwwWithHistory,
            MergePolicy::ControlledLww,
        ] {
            assert!(p.accepts_remote(), "{} deveria aceitar remoto", p.description());
        }
        // todas as camadas têm política definida (exaustivo)
        for l in [
            MemoryLayer::L0Sensory, MemoryLayer::L1Working, MemoryLayer::L2EpisodicShort,
            MemoryLayer::L3EpisodicLong, MemoryLayer::L4Semantic, MemoryLayer::L5Procedural,
            MemoryLayer::L6Reserved, MemoryLayer::L7Identity,
        ] {
            let _ = MergePolicy::for_layer(l);
        }
    }

    #[test]
    fn layer_policy_rejects_local_layers() {
        let mut a = CrdtMemorySync::new(1);
        // L0/L1/L6: versão remota NUNCA adotada
        for layer in [
            MemoryLayer::L0Sensory,
            MemoryLayer::L1Working,
            MemoryLayer::L6Reserved,
        ] {
            assert_eq!(
                a.apply_remote_version_with_policy(2, 1, MergePolicy::for_layer(layer)),
                MergeVerdict::Rejected
            );
            assert!(a.node_versions.is_empty(), "camada local não pode registrar peer");
        }
        // L4 (semântica): aceita — primeiro conhecimento → Applied
        assert_eq!(
            a.apply_remote_version_with_policy(2, 1, MergePolicy::for_layer(MemoryLayer::L4Semantic)),
            MergeVerdict::Applied
        );
    }

    // ── P0-7: harness de 3 nós + partition/rejoin ─────────────────────────

    use crate::memory_doc::{MemoryDoc, MemoryRecord, MemoryState};
    use crate::sgdb::Sgdb;
    use crate::storage::InMemory;

    struct MeshNode {
        db: Sgdb,
        crdt: CrdtMemorySync,
    }

    /// Malha de teste: N nós (Sgdb + CRDT) com arestas direcionais.
    /// `round()` = TX (publica) → RX (drena) → pull de records via
    /// `merge_remote` ao longo das arestas. Simula partições (sem aresta),
    /// duplicatas (injeção 2x) e entrega atrasada (versão antiga re-aplicada).
    struct Mesh {
        ids: Vec<u8>,
        nodes: Vec<MeshNode>,
        /// edges[i][j] = i pode entregar para j
        edges: Vec<Vec<bool>>,
    }

    impl Mesh {
        fn new(ids: &[u8]) -> Self {
            let nodes = ids
                .iter()
                .map(|&id| MeshNode {
                    db: Sgdb::open_with_node_id(id, InMemory::new()).unwrap(),
                    crdt: CrdtMemorySync::new(id),
                })
                .collect();
            let n = ids.len();
            Mesh {
                ids: ids.to_vec(),
                nodes,
                edges: vec![vec![false; n]; n],
            }
        }

        fn connect(&mut self, i: usize, j: usize) {
            self.edges[i][j] = true;
            self.edges[j][i] = true;
        }

        fn remember(&mut self, i: usize, key: &str, text: &str, emb: &[f32]) {
            self.nodes[i].db.remember_semantic(key, text, emb).unwrap();
            self.nodes[i].crdt.record_change();
        }

        fn l2_text(&mut self, i: usize, key: &str) -> String {
            String::from_utf8_lossy(
                &self.nodes[i]
                    .db
                    .get(MemoryLayer::L2EpisodicShort, key)
                    .unwrap()
                    .unwrap()
                    .payload,
            )
            .into_owned()
        }

        fn doc_count(&mut self, i: usize) -> usize {
            let mut n = 0;
            for layer in [MemoryLayer::L2EpisodicShort, MemoryLayer::L4Semantic] {
                n += self.nodes[i]
                    .db
                    .scan_prefix(&format!("md/{}/", layer.as_str()))
                    .unwrap()
                    .len();
            }
            n
        }

        /// Uma ronda de ANTI-ENTROPY no mesh. `dup` injeta cada anúncio 2x
        /// (teste de duplicata). Retorna records aplicados no pull.
        ///
        /// 1. Cada nó ANUNCIA o clock completo (próprio + relayado) — as
        ///    versões atravessam nós intermediários (gossip, P0-7).
        /// 2. Cada nó aplica os anúncios recebidos (veredicto por versão).
        /// 3. Pull DIRECIONADO: cada versão que `src` conhece é reconciliada
        ///    em `dst` via `merge_remote` (idempotente — Duplicate quando
        ///    nada mudou; Applied quando o doc é novo OU o side-metadata
        ///    avançou).
        fn round(&mut self, _now: u64, dup: bool) -> Result<usize, SgdbError> {
            let n = self.nodes.len();
            // clocks PRÉ-RONDA: o que cada nó sabia ANTES desta ronda. O pull
            // mede a faixa faltante contra isso — se medisse contra o clock
            // já atualizado (passo 2), a lacuna "sumiria" e nada seria puxado.
            let pre_clocks: Vec<Vec<(u8, u64)>> =
                (0..n).map(|i| self.nodes[i].crdt.known_clock()).collect();
            // 1) anúncios
            let mut outboxes: Vec<Vec<(u8, u64)>> = Vec::with_capacity(n);
            for i in 0..n {
                outboxes.push(self.nodes[i].crdt.announce());
            }
            // 2) entrega por arestas + merge de versões
            for j in 0..n {
                for (i, outbox) in outboxes.iter().enumerate() {
                    if i == j || !self.edges[i][j] {
                        continue;
                    }
                    for &(node, v) in outbox {
                        if dup {
                            let _ = self.nodes[j].crdt.apply_remote_version(node, v);
                        }
                        let _ = self.nodes[j].crdt.apply_remote_version(node, v);
                    }
                }
            }
            // 3) pull/reconciliação por versão
            let mut applied = 0;
            for i in 0..n {
                for j in 0..n {
                    if i == j || !self.edges[i][j] {
                        continue;
                    }
                    if i < j {
                        let (l, r) = self.nodes.split_at_mut(j);
                        applied += pull_delta(&mut l[i], &mut r[0], &pre_clocks[j])?;
                    } else {
                        let (l, r) = self.nodes.split_at_mut(i);
                        applied += pull_delta(&mut r[0], &mut l[j], &pre_clocks[j])?;
                    }
                }
            }
            Ok(applied)
        }

        fn converge(&mut self, rounds: usize) -> Result<usize, SgdbError> {
            let mut applied = 0;
            for r in 0..rounds {
                applied += self.round((r as u64) * 200, false)?;
            }
            Ok(applied)
        }
    }

    /// Anti-entropy: para cada versão que `src` conhece (anúncio), reconcilia
    /// em `dst` SOMENTE a faixa causal faltante — `known(dst)+1..=v` por nó
    /// (o anúncio anuncia o MÁXIMO; um peer que entra depois precisa de toda
    /// a série). Os records vêm do `clock_index` (vínculo versão ↔ docs) +
    /// o companion de texto L2/L4. `merge_remote` é idempotente (Duplicate
    /// quando nada mudou) — rondas repetidas não reescrevem.
    ///
    /// `dst_pre` = o clock que `dst` conhecia ANTES da ronda (parâmetro: o
    /// `known_clock()` corrente já reflete os anúncios aplicados no passo 2).
    fn pull_delta(
        src: &mut MeshNode,
        dst: &mut MeshNode,
        dst_pre: &[(u8, u64)],
    ) -> Result<usize, SgdbError> {
        let src_clock = src.crdt.announce();
        let mut applied = 0;
        for &(node, v) in &src_clock {
            // nunca puxa a própria versão nem heartbeats vazios
            if node == dst.crdt.node_id() || v == 0 {
                continue;
            }
            let known = dst_pre
                .iter()
                .find(|(n, _)| *n == node)
                .map(|(_, k)| *k)
                .unwrap_or(0);
            if v <= known {
                continue;
            }
            for c in (known + 1)..=v {
                for sk in src.db.keys_for_clock(node, c) {
                    if let Ok(Some(rec)) = src.db.export_record(&sk) {
                        if dst.db.merge_remote(rec)? == MergeVerdict::Applied {
                            applied += 1;
                        }
                    }
                    let companion = sk.replacen("/L4/", "/L2/", 1);
                    if companion != sk {
                        if let Ok(Some(rec)) = src.db.export_record(&companion) {
                            if dst.db.merge_remote(rec)? == MergeVerdict::Applied {
                                applied += 1;
                            }
                        }
                    }
                }
            }
        }
        Ok(applied)
    }

    fn emb16(seed: u64) -> Vec<f32> {
        let mut s = seed.wrapping_mul(1103515245).wrapping_add(12345);
        let mut v = Vec::with_capacity(16);
        for _ in 0..16 {
            s = s.wrapping_mul(1103515245).wrapping_add(12345);
            v.push(((s >> 32) as i32 % 200) as f32 / 100.0 - 1.0);
        }
        v
    }

    #[test]
    fn three_node_triangle_convergence() {
        // A ── B, \ /, C — malha completa; convergência multi-direcional
        let mut m = Mesh::new(&[1, 2, 3]);
        m.connect(0, 1);
        m.connect(1, 2);
        m.connect(0, 2);
        m.remember(0, "m1", "memoria do no A", &emb16(1));
        m.remember(1, "m2", "memoria do no B", &emb16(2));
        m.remember(2, "m3", "memoria do no C", &emb16(3));
        let applied = m.converge(8).unwrap();
        assert!(applied >= 6, "deveria replicar as 3 memórias (L4+L2): {applied}");
        // convergência: todo nó tem as 3 memórias (6 docs L4+L2)
        for i in 0..3 {
            assert_eq!(m.doc_count(i), 6, "nó {i} deveria ter 6 docs");
        }
        // versões convergem (máximo por nó em toda a malha)
        for i in 0..3 {
            let mut vs = m.nodes[i].crdt.node_versions.clone();
            vs.sort();
            let expected: Vec<(u8, u64)> = (0..3)
                .filter(|j| *j != i)
                .map(|j| (m.ids[j], 1))
                .collect();
            assert_eq!(vs, expected, "nó {i} com versões incompletas");
        }
        // ponto-fixo: mais rondas não aplicam docs novos
        assert_eq!(m.converge(4).unwrap(), 0);
    }

    #[test]
    fn partition_rejoin_preserves_concurrent_writes() {
        // A e B escrevem a MESMA chave (concorrentes) enquanto desconectados;
        // C (relay) recebe de ambos; na reconexão NENHUMA versão é perdida.
        let mut m = Mesh::new(&[1, 2, 3]);
        m.connect(0, 2); // A↔C
        m.connect(1, 2); // B↔C (A e B SEM conexão direta)
        m.remember(0, "pref", "usuario prefere DARK", &emb16(1));
        m.remember(1, "pref", "usuario prefere LIGHT", &emb16(2));
        // fase particionada: C recebe de ambos
        m.converge(4).unwrap();
        assert_eq!(m.l2_text(0, "pref"), "usuario prefere DARK");
        assert_eq!(m.l2_text(1, "pref"), "usuario prefere LIGHT");
        // C aplicou a primeira (A) e PRESERVOU o conflito com a de B
        assert_eq!(m.l2_text(2, "pref"), "usuario prefere DARK");
        assert!(
            m.nodes[2].crdt.conflicts.iter().any(|c| c.node_id == 2),
            "C deveria ter o conflito com B: {:?}",
            m.nodes[2].crdt.conflicts
        );
        // reconexão A↔B
        m.connect(0, 1);
        m.converge(6).unwrap();
        // nenhuma versão foi destruída: cada autor mantém a sua
        assert_eq!(m.l2_text(0, "pref"), "usuario prefere DARK");
        assert_eq!(m.l2_text(1, "pref"), "usuario prefere LIGHT");
        assert_eq!(m.l2_text(2, "pref"), "usuario prefere DARK");
        // conflitos preservados em TODOS (nunca LWW cego)
        assert!(m.nodes[0].crdt.conflicts.iter().any(|c| c.node_id == 2));
        assert!(m.nodes[1].crdt.conflicts.iter().any(|c| c.node_id == 1));
        assert!(m.nodes[2].crdt.conflicts.iter().any(|c| c.node_id == 2));
        // estado causal convergido (máximo por nó)
        assert_eq!(m.nodes[0].crdt.node_versions, vec![(2, 1)]);
        assert_eq!(m.nodes[1].crdt.node_versions, vec![(1, 1)]);
        // sincronização repetida é idempotente
        assert_eq!(m.converge(4).unwrap(), 0, "pós-convergência não aplica nada");
    }

    #[test]
    fn duplicate_and_stale_delivery_is_idempotent() {
        let mut m = Mesh::new(&[1, 2]);
        m.connect(0, 1);
        m.remember(0, "d1", "doc um", &emb16(1));
        // ronda com duplicata: B recebe (1,1) 2x
        let applied = m.round(0, true).unwrap();
        assert!(applied >= 2, "d1 (L4+L2) deveria ser aplicado: {applied}");
        assert_eq!(m.nodes[1].crdt.node_versions, vec![(1, 1)]);
        // segunda escrita + ronda normal
        m.remember(0, "d2", "doc dois", &emb16(2));
        m.round(200, false).unwrap();
        assert_eq!(m.nodes[1].crdt.node_versions, vec![(1, 2)]);
        // entrega atrasada de versão antiga (out-of-order) → Stale
        assert_eq!(m.nodes[1].crdt.apply_remote_version(1, 1), MergeVerdict::Stale);
        assert_eq!(m.nodes[1].crdt.node_versions, vec![(1, 2)]);
        // docs presentes UMA vez cada (recall não duplica)
        let hits = m.nodes[1].db.recall(&emb16(1), 10).unwrap();
        assert_eq!(hits.iter().filter(|h| h.key.ends_with("/d1")).count(), 1);
        assert_eq!(m.doc_count(1), 4, "2 memórias × (L4+L2)");
    }

    #[test]
    fn fresh_node_catches_up_after_restart() {
        // A e B convergem; C entra NOVO (db vazio + relógio zerado — simula
        // restart sem estado durável) e alcança tudo.
        let mut m = Mesh::new(&[1, 2]);
        m.connect(0, 1);
        m.remember(0, "m1", "memoria um", &emb16(1));
        m.remember(1, "m2", "memoria dois", &emb16(2));
        m.converge(4).unwrap();
        // nó C entra (restart)
        m.ids.push(3);
        m.nodes.push(MeshNode {
            db: Sgdb::open_with_node_id(3, InMemory::new()).unwrap(),
            crdt: CrdtMemorySync::new(3),
        });
        let n = m.nodes.len();
        m.edges.push(vec![false; n]);
        for e in &mut m.edges {
            e.resize(n, false);
        }
        m.connect(2, 0);
        m.connect(2, 1);
        m.converge(8).unwrap();
        // nó novo alcançou tudo, sem perda nos antigos
        assert_eq!(m.doc_count(2), 4, "nó novo deveria alcançar as 2 memórias");
        assert_eq!(m.doc_count(0), 4);
        assert_eq!(m.doc_count(1), 4);
        assert_eq!(m.nodes[2].crdt.node_versions, vec![(1, 1), (2, 1)]);
    }

    #[test]
    fn crdt_state_roundtrip_and_restore() {
        // serialização durável (P0-11): encode/decode preserva tudo
        let mut a = CrdtMemorySync::new(7);
        a.record_change();
        a.record_change(); // v2 local
        assert_eq!(
            a.apply_remote_version(3, 5),
            MergeVerdict::Conflict,
            "7 tem estado próprio → versão alheia nova = conflito preservado"
        );
        let s = a.state();
        let bytes = s.encode();
        assert_eq!(&bytes[0..4], b"CRDT");
        let dec = CrdtState::decode(&bytes).unwrap();
        assert_eq!(dec, s);
        // restore em nó NOVO com o mesmo node_id recupera o clock (sem regressão)
        let mut b = CrdtMemorySync::new(7);
        assert!(b.restore(dec.clone()));
        assert_eq!(b.local_version(), 2);
        assert_eq!(b.own_writes, 2);
        assert_eq!(b.node_versions, vec![(3, 5)]);
        // identidade é inviolável: estado de outro nó é RECUSADO
        let mut c = CrdtMemorySync::new(9);
        assert!(!c.restore(dec));
        assert_eq!(c.local_version(), 0, "nada deve ser adotado");
        // decode malformado/truncado nunca panics
        for n in [0usize, 1, 4, 14, 15, 16, 21, 22, 23, bytes.len() - 1] {
            let mut t = bytes.clone();
            t.truncate(n);
            assert!(CrdtState::decode(&t).is_err(), "truncado em {n} deveria errar");
        }
        assert!(CrdtState::decode(b"XXXX").is_err());
        assert!(CrdtState::decode(&[]).is_err());
    }

    #[test]
    fn restart_preserves_clock_no_regression() {
        // A e B convergem; A "reinicia" (Sgdb novo + crdt novo) e restaura o
        // estado durável via side-table `sys/crdt/` — o relógio não regride,
        // versões não são re-anunciadas como novas e docs não re-puxam.
        let mut m = Mesh::new(&[1, 2]);
        m.connect(0, 1);
        m.remember(0, "m1", "memoria um", &emb16(1));
        m.remember(0, "m2", "memoria dois", &emb16(2));
        m.converge(4).unwrap();
        assert_eq!(m.doc_count(1), 4, "B deveria ter as 2 memórias");
        // persiste o estado do CRDT de A na side-table (escape hatch)
        let st = m.nodes[0].crdt.state().encode();
        m.nodes[0].db.write_side_bytes("sys/crdt/node1", &st).unwrap();
        // restart de A: Sgdb NOVO (mesmo backend InMemory? não — InMemory novo
        // perde docs; o teste do REINÍCIO é sobre o relógio do CRDT, então o
        // db novo é vazio e o crdt restaura o clock: versões antigas voltam
        // como Duplicate/Stale, nunca como Applied novo)
        let mut fresh = CrdtMemorySync::new(1);
        let restored = CrdtState::decode(&m.nodes[0].db.read_side_bytes("sys/crdt/node1").unwrap().unwrap()).unwrap();
        assert!(fresh.restore(restored));
        assert_eq!(fresh.local_version(), 2, "clock do A preservado pós-restart");
        // o RECEPTOR (B) que já conhecia (1,2) trata a reentrega pós-restart
        // como Duplicate e a versão atrasada (1,1) como Stale — nada regride
        let mut b_clock = CrdtMemorySync::new(2);
        assert_eq!(b_clock.apply_remote_version(1, 2), MergeVerdict::Applied);
        assert_eq!(b_clock.apply_remote_version(1, 2), MergeVerdict::Duplicate);
        assert_eq!(b_clock.apply_remote_version(1, 1), MergeVerdict::Stale);
        assert_eq!(b_clock.apply_remote_version(3, 0), MergeVerdict::Duplicate); // heartbeat
        // anúncio pós-restart reflete o clock restaurado (o relay não re-infla)
        assert_eq!(fresh.announce().first(), Some(&(1, 2)));
    }

    #[test]
    fn versions_relay_through_intermediate_node() {
        // gossip (P0-7): A escreve; C (relay) repassa versão + docs a B, que
        // NUNCA fala com A. Anti-entropy atravessa nós intermediários.
        let mut m = Mesh::new(&[1, 2, 3]);
        m.connect(0, 2); // A↔C
        m.connect(1, 2); // B↔C — A e B NUNCA conectados
        m.remember(0, "relay1", "memoria do A", &emb16(7));
        m.converge(3).unwrap();
        // C aprendeu e repassou: B tem a versão de A e os docs
        assert_eq!(m.nodes[2].crdt.node_versions, vec![(1, 1)]);
        assert_eq!(m.nodes[1].crdt.node_versions, vec![(1, 1)]);
        assert_eq!(m.doc_count(1), 2, "B recebeu os docs via relay");
        assert_eq!(m.l2_text(1, "relay1"), "memoria do A");
        // B escreve também; A alcança através de C
        m.remember(1, "relay2", "memoria do B", &emb16(8));
        m.converge(3).unwrap();
        assert_eq!(m.doc_count(0), 4, "A alcançou a memória de B via C");
        assert_eq!(m.l2_text(0, "relay2"), "memoria do B");
        // estado estacionário: rondas extras não aplicam nada
        assert_eq!(m.converge(3).unwrap(), 0);
    }

    #[test]
    fn directed_pull_fetches_full_version_range() {
        // um peer que entra DEPOIS de várias escritas precisa de TODA a faixa
        // causal 1..=v do autor, não só a última versão anunciada
        let mut m = Mesh::new(&[1, 2]);
        m.connect(0, 1);
        m.remember(0, "v1", "primeira", &emb16(1));
        m.remember(0, "v2", "segunda", &emb16(2));
        m.remember(0, "v3", "terceira", &emb16(3));
        // B NÃO participou de nenhuma ronda — entra agora (clock zerado)
        m.round(0, false).unwrap();
        assert_eq!(m.doc_count(1), 6, "B deveria puxar as 3 memórias (L4+L2)");
        assert_eq!(m.nodes[1].crdt.node_versions, vec![(1, 3)]);
        assert_eq!(m.l2_text(1, "v1"), "primeira");
        assert_eq!(m.l2_text(1, "v3"), "terceira");
        // idempotente: segunda ronda aplica nada
        assert_eq!(m.round(0, false).unwrap(), 0);
    }

    // ── P2-1: convergência em topologias aleatórias (CRDT formal) ────────
    // LCG determinístico (zero deps, decisão P1-4). Gera grafos direcionais
    // CONEXOS (espinha em anel + arestas extras aleatórias) para que o
    // anti-entropy tenha caminho entre todos os pares — gossip assimétrico
    // + escritas distribuídas devem convergir ao MESMO estado em qualquer
    // topologia conexa.

    fn lcg(state: &mut u64) -> u64 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *state
    }

    fn connect_directed(m: &mut Mesh, i: usize, j: usize) {
        m.edges[i][j] = true;
    }

    fn assert_converged(m: &mut Mesh) {
        let n = m.nodes.len();
        // todos os nós: mesma contagem de docs (L2+L4)
        let counts: Vec<usize> = (0..n).map(|i| m.doc_count(i)).collect();
        for i in 1..n {
            assert_eq!(
                counts[i], counts[0],
                "nó {i} com contagem divergente: {counts:?}"
            );
        }
        // todos os nós: mesmo conjunto de keys (L2+L4)
        let keys0: Vec<String> = {
            let mut k: Vec<String> = m.nodes[0]
                .db
                .scan_prefix("md/L2/")
                .unwrap()
                .into_iter()
                .map(|(k, _)| k)
                .chain(
                    m.nodes[0]
                        .db
                        .scan_prefix("md/L4/")
                        .unwrap()
                        .into_iter()
                        .map(|(k, _)| k),
                )
                .collect();
            k.sort();
            k
        };
        for i in 1..n {
            let mut k: Vec<String> = m.nodes[i]
                .db
                .scan_prefix("md/L2/")
                .unwrap()
                .into_iter()
                .map(|(k, _)| k)
                .chain(
                    m.nodes[i]
                        .db
                        .scan_prefix("md/L4/")
                        .unwrap()
                        .into_iter()
                        .map(|(k, _)| k),
                )
                .collect();
            k.sort();
            assert_eq!(k, keys0, "nó {i} com keys divergentes");
        }
        // ESTADO CONVERGENTE REAL: para cada storage key, o `MemoryRecord`
        // exportado (NMD1 + clock causal + state + validade + meta/identidade)
        // é byte-idêntico em todos os nós. NOTA: `node_versions` do CRDT é
        // conhecimento de GOSSIP (parcial em topologias direcionais) — NÃO é
        // o estado convergido; a igualdade de conteúdo/clock/identidade é.
        for key in &keys0 {
            let rec0 = m.nodes[0]
                .db
                .export_record(key)
                .unwrap()
                .unwrap_or_else(|| panic!("nó 0 sem {key}"));
            let enc0 = rec0.encode();
            for i in 1..n {
                let rec = m.nodes[i]
                    .db
                    .export_record(key)
                    .unwrap()
                    .unwrap_or_else(|| panic!("nó {i} sem {key}"));
                assert_eq!(
                    rec.encode(),
                    enc0,
                    "nó {i} com record divergente para {key}"
                );
            }
        }
    }

    #[test]
    fn random_topology_convergence() {
        // 6 nós, 3 seeds de topologia distintas, cada uma com 30 writes
        // distribuídos — todas devem convergir ao mesmo estado.
        for seed in 1..=3u64 {
            let mut s = seed;
            let n = 6;
            let mut m = Mesh::new(&[1, 2, 3, 4, 5, 6]);
            // espinha em anel (garante conectividade fraca mínima)
            for i in 0..n {
                connect_directed(&mut m, i, (i + 1) % n);
            }
            // arestas extras aleatórias (assimetria/gossip parcial)
            let extra = 4 + (lcg(&mut s) % 8) as usize;
            for _ in 0..extra {
                let i = (lcg(&mut s) % n as u64) as usize;
                let j = (lcg(&mut s) % n as u64) as usize;
                if i != j {
                    connect_directed(&mut m, i, j);
                }
            }
            // 30 writes distribuídos por autores/chaves fixas-width
            for w in 0..30u64 {
                let author = (lcg(&mut s) % n as u64) as usize;
                m.remember(author, &format!("m{w:03}"), &format!("memoria {w}"), &emb16(w));
            }
            let applied = m.converge(20).unwrap();
            assert!(applied >= 60, "seed {seed}: replicou pouco: {applied}");
            assert_converged(&mut m);
            // ponto-fixo: rondas extras não mudam nada
            assert_eq!(m.converge(5).unwrap(), 0, "seed {seed}: não é ponto-fixo");
        }
    }

    #[test]
    fn random_topology_with_partitions_rejoins() {
        // Mesma seed de escrita, mas com fases: 1) cluster [0,1] ↔ [2,3]
        // separado de [4,5]; 2) writes concorrentes; 3) rejoin completo;
        // 4) converge — nenhuma versão perdida, nenhum conflito cego.
        let mut s = 42u64;
        let n = 6;
        let mut m = Mesh::new(&[1, 2, 3, 4, 5, 6]);
        // fase 1: dois cliques desconexos
        for i in 0..3 {
            for j in 0..3 {
                if i != j {
                    connect_directed(&mut m, i, j);
                    connect_directed(&mut m, j, i);
                }
            }
        }
        for i in 3..6 {
            for j in 3..6 {
                if i != j {
                    connect_directed(&mut m, i, j);
                    connect_directed(&mut m, j, i);
                }
            }
        }
        // fase 2: writes concorrentes — cluster 1 grava k_a/k_b, cluster 2
        // grava a mesma k_a (conflito) + k_c (só do cluster 2)
        m.remember(0, "ka", "cluster-1 ka", &emb16(1));
        m.remember(1, "kb", "cluster-1 kb", &emb16(2));
        m.remember(4, "ka", "cluster-2 ka", &emb16(3));
        m.remember(5, "kc", "cluster-2 kc", &emb16(4));
        m.converge(6).unwrap();
        // cada cluster converge internamente
        assert_eq!(m.doc_count(0), m.doc_count(1));
        assert_eq!(m.doc_count(2), m.doc_count(3));
        assert_eq!(m.doc_count(4), m.doc_count(5));
        // fase 3: rejoin completo (anel + extras)
        for i in 0..n {
            connect_directed(&mut m, i, (i + 1) % n);
        }
        for _ in 0..6 {
            let i = (lcg(&mut s) % n as u64) as usize;
            let j = (lcg(&mut s) % n as u64) as usize;
            if i != j {
                connect_directed(&mut m, i, j);
            }
        }
        m.converge(20).unwrap();
        // convergência com conflito: keys IDÊNTICAS em todos + records
        // byte-idênticos para as chaves NÃO concorrentes (kb, kc) + conflito
        // ka preservado em todos (nunca LWW cego — versões concorrentes
        // distintas continuam distintas, sem perda).
        let keys0: Vec<String> = {
            let mut k: Vec<String> = m.nodes[0]
                .db
                .scan_prefix("md/L2/")
                .unwrap()
                .into_iter()
                .map(|(k, _)| k)
                .chain(
                    m.nodes[0]
                        .db
                        .scan_prefix("md/L4/")
                        .unwrap()
                        .into_iter()
                        .map(|(k, _)| k),
                )
                .collect();
            k.sort();
            k
        };
        for i in 1..n {
            let mut k: Vec<String> = m.nodes[i]
                .db
                .scan_prefix("md/L2/")
                .unwrap()
                .into_iter()
                .map(|(k, _)| k)
                .chain(
                    m.nodes[i]
                        .db
                        .scan_prefix("md/L4/")
                        .unwrap()
                        .into_iter()
                        .map(|(k, _)| k),
                )
                .collect();
            k.sort();
            assert_eq!(k, keys0, "nó {i} com keys divergentes pós-rejoin");
        }
        // chaves não-conflitantes: byte-idênticas (kb, kc + companions)
        for key in &keys0 {
            if key.ends_with("ka") || key.contains("ka") {
                continue; // conflito — validado abaixo
            }
            let rec0 = m.nodes[0].db.export_record(key).unwrap().unwrap();
            let enc0 = rec0.encode();
            for i in 1..n {
                let rec = m.nodes[i].db.export_record(key).unwrap().unwrap();
                assert_eq!(rec.encode(), enc0, "nó {i}: record divergente em {key}");
            }
        }
        // conflito ka preservado: a EVIDÊNCIA (ConflictRecord persistido via
        // `merge_remote`, com records MDR1 dos dois lados) deve EXISTIR para
        // a camada superior resolver (nunca LWW cego). NOTA: o ConflictRecord
        // é evidência LOCAL do merge (criado onde o merge concorrente
        // aconteceu) — NÃO é a unidade de replicação (MemoryRecord MDR1),
        // então não se exige convergência de conflitos entre nós, apenas que
        // a evidência exista e nenhuma versão seja descartada.
        let cfl0: Vec<String> = {
            let mut v: Vec<String> = m.nodes[0]
                .db
                .conflicts()
                .into_iter()
                .map(|c| c.conflict_id)
                .collect();
            v.sort();
            v
        };
        assert!(!cfl0.is_empty(), "conflito ka deveria existir");
        // autores preservam o PRÓPRIO conteúdo (nenhuma versão perdida):
        // nó 0 (node_id 1) manteve "cluster-1 ka"; nó 4 (node_id 5) manteve
        // "cluster-2 ka".
        assert!(m.l2_text(0, "ka").contains("cluster-1 ka"));
        assert!(m.l2_text(4, "ka").contains("cluster-2 ka"));
        // ponto-fixo
        assert_eq!(m.converge(5).unwrap(), 0);
    }

    // ── P2-5: telepatia multi-IA em camadas (simulação) ───────────────────
    // Cenário honesto: as "IAs" são POLÍTICAS DETERMINÍSTICAS stub (sem LLM,
    // sem embeddings reais — demo). O que se prova é o SUBSTRATO de memória:
    // 5 camadas de agentes (Sgdb + CRDT) em mesh; a "IA externa" fala com a
    // camada 1; cada camada responde via recall do PRÓPRIO banco; telepatia
    // (anti-entropy) propaga as memórias camada-acima; uma camada PROFUNDA
    // deve recuperar por recall semântico uma memória que entrou na camada 1.

    #[test]
    fn layered_ai_telepathy_mesh() {
        // 8 agentes em 5 camadas (limite do cenário). Camadas:
        //   L1 superfície (fala com a IA externa): índices 0,1
        //   L2: 2,3 | L3: 4,5 | L4: 6 | L5 consolidação/identidade: 7
        let n = 8;
        let mut m = Mesh::new(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let layers: [&[usize]; 5] = [&[0, 1], &[2, 3], &[4, 5], &[6], &[7]];

        // mesh em camadas: arestas dirigidas camada k ↔ k+1 (telepatia sobe/
        // desce) + intra-camada (redundância) + extras aleatórias (gossip).
        for k in 0..layers.len() - 1 {
            for &a in layers[k] {
                for &b in layers[k + 1] {
                    connect_directed(&mut m, a, b);
                    connect_directed(&mut m, b, a);
                }
            }
        }
        for l in layers {
            for w in 0..l.len() {
                let a = l[w];
                let b = l[(w + 1) % l.len()];
                if a != b {
                    connect_directed(&mut m, a, b);
                    connect_directed(&mut m, b, a);
                }
            }
        }
        let mut s = 7u64;
        for _ in 0..6 {
            let i = (lcg(&mut s) % n as u64) as usize;
            let j = (lcg(&mut s) % n as u64) as usize;
            if i != j {
                connect_directed(&mut m, i, j);
            }
        }

        // IA EXTERNA fala com a camada 1 (mensagens viram memórias na
        // superfície). Stub: cada agente guarda e responde com o que RECALLA.
        let msgs = [
            "usuario prefere dark mode",
            "reuniao marcada as 14h",
            "o deploy quebrou a CI",
            "gosta de cafe espresso",
            "proximo sprint e de features",
        ];
        for (idx, text) in msgs.iter().enumerate() {
            let agent = if idx % 2 == 0 { 0 } else { 1 };
            m.remember(agent, &format!("ext{idx:02}"), text, &emb16(100 + idx as u64));
        }
        // camada 1 responde a partir do PRÓPRIO recall (IA stub usando o
        // substrato): "camada-1 responde: <top hit>"
        for i in 0..2 {
            let hits = m.nodes[i].db.recall(&emb16(50), 1).unwrap();
            let top = hits.first().map(|h| h.text.clone()).unwrap_or_default();
            m.remember(
                i,
                &format!("rsp{i:02}"),
                &format!("camada-1 responde: {top}"),
                &emb16(60 + i as u64),
            );
        }

        // telepatia: convergência sobe as memórias L1 → L5
        let applied = m.converge(12).unwrap();
        assert!(applied >= 30, "replicou pouco entre camadas: {applied}");

        // camadas PROFUNDAS (L4/L5) recuperam por recall o que entrou na
        // camada 1 — telepatia: a memória EXATA `ext02` (seed 102) ressurge
        // na camada mais profunda, e ambos criam memória consolidada derivada.
        for (deep, seed) in [(6usize, 102u64), (7, 102)] {
            let hits = m.nodes[deep].db.recall(&emb16(seed), 1).unwrap();
            assert!(
                hits.iter().any(|h| h.text.contains("deploy quebrou a CI")),
                "camada {deep} não telepatizou a memória da camada 1: {:?}",
                hits.iter().map(|h| &h.text).collect::<Vec<_>>()
            );
            let top = hits.first().unwrap().text.clone();
            m.remember(
                deep,
                &format!("cons{deep:02}"),
                &format!("consolidado: {top}"),
                &emb16(200 + deep as u64),
            );
        }

        // convergência final: byte-idêntica em TODOS (chaves distintas, sem
        // conflitos) + ponto-fixo
        m.converge(12).unwrap();
        assert_converged(&mut m);
        assert_eq!(m.converge(5).unwrap(), 0, "não é ponto-fixo após telepatia");
    }
}

// ── P1-4: property tests decode∘encode dos codecs CRDT (p2p) ────────
// Harness LCG determinístico (zero deps; decisão P1-4). Respeita as
// invariantes do wire (counts ≤ u16::MAX via try_encode; MemoryRecord usa
// doc válido com clock nos slots fixos).
#[cfg(all(test, feature = "p2p"))]
mod prop_tests {
    use super::*;
    use crate::memory_doc::{MemoryDoc, MemoryState};
    use alloc::vec::Vec;

    fn rng(state: &mut u64) -> u64 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *state >> 32
    }

    fn rec(state: &mut u64) -> MemoryRecord {
        let payload: Vec<u8> = (0..(rng(state) % 50) as usize).map(|_| rng(state) as u8).collect();
        let mut doc = MemoryDoc::new(MemoryLayer::L4Semantic, "k", payload);
        doc.clock.tick((rng(state) % 8) as u8);
        MemoryRecord::new(doc, MemoryState::Active, None)
    }

    fn versions(state: &mut u64, n: usize) -> Vec<MemoryVersion> {
        (0..n)
            .map(|_| MemoryVersion { node_id: (rng(state) % 8) as u8, version: rng(state) })
            .collect()
    }

    #[test]
    fn prop_delta_roundtrip_lcg() {
        let mut state = 0xC0FF_EE00_5EED_0020u64;
        for _ in 0..1000 {
            let n = (rng(&mut state) % 8) as usize;
            let recs: Vec<MemoryRecord> = (0..n).map(|_| rec(&mut state)).collect();
            let n2 = (rng(&mut state) % 8) as usize;
            let base = versions(&mut state, n2);
            let d = MemoryDelta { base, records: recs };
            let enc = d.try_encode().unwrap();
            let dec = MemoryDelta::decode(&enc).unwrap();
            assert_eq!(dec, d);
        }
    }

    #[test]
    fn prop_snapshot_roundtrip_lcg() {
        let mut state = 0xC0FF_EE00_5EED_0021u64;
        for _ in 0..1000 {
            let n = (rng(&mut state) % 8) as usize;
            let recs: Vec<MemoryRecord> = (0..n).map(|_| rec(&mut state)).collect();
            let n2 = (rng(&mut state) % 8) as usize;
            let versions = versions(&mut state, n2);
            let s = MemorySnapshot { versions, records: recs };
            let enc = s.try_encode().unwrap();
            let dec = MemorySnapshot::decode(&enc).unwrap();
            assert_eq!(dec, s);
        }
    }

    #[test]
    fn prop_envelope_roundtrip_lcg() {
        let mut state = 0xC0FF_EE00_5EED_0022u64;
        for _ in 0..1000 {
            let payload: Vec<u8> = (0..(rng(&mut state) % 100) as usize)
                .map(|_| rng(&mut state) as u8)
                .collect();
            let auth: Vec<u8> = (0..(rng(&mut state) % 100) as usize)
                .map(|_| rng(&mut state) as u8)
                .collect();
            let e = SignedEnvelope::new(rng(&mut state) as u8, payload, auth);
            let enc = e.try_encode().unwrap();
            let (dec, _) = SignedEnvelope::decode(&enc).unwrap();
            assert_eq!(dec, e);
        }
    }

    #[test]
    fn prop_crdt_state_roundtrip_lcg() {
        let mut state = 0xC0FF_EE00_5EED_0023u64;
        for _ in 0..1000 {
            let nvs: Vec<(u8, u64)> = (0..(rng(&mut state) % 16) as usize)
                .map(|_| ((rng(&mut state) % 8) as u8, rng(&mut state)))
                .collect();
            let st = CrdtState {
                node_id: rng(&mut state) as u8,
                local_version: rng(&mut state),
                own_writes: rng(&mut state),
                node_versions: nvs,
            };
            let enc = st.try_encode().unwrap();
            let dec = CrdtState::decode(&enc).unwrap();
            assert_eq!(dec, st);
        }
    }
}
