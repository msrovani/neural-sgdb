//! ADR-0063 F2 — MemoryDoc L0–L7: layout binário zero-copy-ish (NoProto-inspired).
//! Contrato de formato NMD1 — byte-idêntico ao `k_ai::sgdb::memory_doc` do OS mãe.

use alloc::string::String;
use alloc::vec::Vec;

/// Cognitive memory layers (ADR-0060 / 0063).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryLayer {
    L0Sensory = 0,
    L1Working = 1,
    L2EpisodicShort = 2,
    L3EpisodicLong = 3,
    L4Semantic = 4,
    L5Procedural = 5,
    L6Reserved = 6,
    L7Identity = 7,
}

impl MemoryLayer {
    /// Ponto ÚNICO de validação de layer a partir de bytes externos
    /// (maturation P5 — decode rejeita layer inválida; nenhum valor inválido
    /// entra no storage silenciosamente).
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::L0Sensory),
            1 => Some(Self::L1Working),
            2 => Some(Self::L2EpisodicShort),
            3 => Some(Self::L3EpisodicLong),
            4 => Some(Self::L4Semantic),
            5 => Some(Self::L5Procedural),
            6 => Some(Self::L6Reserved),
            7 => Some(Self::L7Identity),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::L0Sensory => "L0",
            Self::L1Working => "L1",
            Self::L2EpisodicShort => "L2",
            Self::L3EpisodicLong => "L3",
            Self::L4Semantic => "L4",
            Self::L5Procedural => "L5",
            Self::L6Reserved => "L6",
            Self::L7Identity => "L7",
        }
    }
}

/// Estado lógico de uma memória (maturation P5 — modelo mínimo explícito).
///
/// Distingue **deleção física** (remove do storage, via `Storage::delete` /
/// tombstone) de **estado lógico** (memória continua representável na
/// história, apenas marcada). `Active` é o default; `Superseded` preserva a
/// cadeia causal (ex: "mudou para Y" supersede "morava em X") sem apagar X.
///
/// **NÃO é serializado no NMD1** — o contrato byte-idêntico com o OS fica
/// intacto; o estado vive em memória e é persistido em namespace lateral
/// (`sys/state/`, via `Storage` cru — ver engine).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MemoryState {
    #[default]
    Active = 0,
    Superseded = 1,
    Archived = 2,
    Invalidated = 3,
    /// Importância caiu abaixo do limiar — candidata a GC (v0.6).
    /// O ESTADO existe; o motor de decay é fase posterior (sem `tick` ainda).
    Decayed = 4,
}

impl MemoryState {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Active),
            1 => Some(Self::Superseded),
            2 => Some(Self::Archived),
            3 => Some(Self::Invalidated),
            4 => Some(Self::Decayed),
            _ => None,
        }
    }
}

/// Relógio vetorial — **dinâmico** (v0.6): fast path de 8 nós em arrays
/// fixos + `overflow` para nós além do 8º. O NMD1 serializa SÓ os 72B fixos
/// (contrato byte-idêntico com o OS); o overflow persiste via side-table
/// `sys/meta/` (MemoryMeta::clock_overflow) e é re-fundido no `get`.
#[derive(Clone, Debug, Default)]
pub struct VectorClock {
    /// Pares (node_id, counter) densos; slots não usados = 0xFF / 0.
    pub nodes: [u8; 8],
    pub counts: [u64; 8],
    /// Nós além do 8º — registro dinâmico (item 6: dynamic node identity).
    /// Invariante: um nó nunca aparece nos fixos E no overflow.
    pub overflow: Vec<(u8, u64)>,
}

/// Limite do registro dinâmico (política bounded): o espaço de u8 tem 256
/// nós; o overflow comporta no máximo 248 (8 fixos). Acima disso o nó é
/// IGNORADO — política explícita e determinística (nunca cresce sem limite).
const MAX_OVERFLOW_NODES: usize = 248;

/// Igualdade SEMÂNTICA: dois relógios são iguais sse o mapeamento
/// (nó → contador) é idêntico — **independente da ordem de inserção nos
/// slots e do overflow**. O derive compararia slots por posição, o que faria
/// dois relógios com a mesma causalidade serem "desiguais".
impl PartialEq for VectorClock {
    fn eq(&self, other: &Self) -> bool {
        for (n, c) in self.iter_nodes() {
            if other.counter_of(n) != c {
                return false;
            }
        }
        for (n, c) in other.iter_nodes() {
            if self.counter_of(n) != c {
                return false;
            }
        }
        true
    }
}

impl Eq for VectorClock {}

impl VectorClock {
    pub fn new() -> Self {
        Self {
            nodes: [0xFF; 8],
            counts: [0; 8],
            overflow: Vec::new(),
        }
    }

    /// Todos os pares (nó, contador): fixos + overflow, sem duplicatas.
    fn iter_nodes(&self) -> impl Iterator<Item = (u8, u64)> + '_ {
        self.nodes
            .iter()
            .zip(self.counts.iter())
            .filter(|(n, _)| **n != 0xFF)
            .map(|(n, c)| (*n, *c))
            .chain(self.overflow.iter().copied())
    }

    /// Pares (nó, contador) — API pública (proveniência: quem escreveu esta
    /// memória e com que contador). Ordem: fixos (ordem de inserção) +
    /// overflow (ordem de inserção). Determinístico para o MESMO histórico.
    pub fn entries(&self) -> Vec<(u8, u64)> {
        self.iter_nodes().collect()
    }

    /// Registra/atualiza o contador de um nó — **registro dinâmico de nós**
    /// (item 6). Slots fixos têm prioridade; cheio → overflow (bounded em
    /// MAX_OVERFLOW_NODES). O caller garante monotonia (tick/merge usam add
    /// saturante / max).
    pub fn set_counter(&mut self, node_id: u8, count: u64) {
        for i in 0..8 {
            if self.nodes[i] == node_id {
                self.counts[i] = count;
                return;
            }
        }
        if let Some(free) = self.nodes.iter().position(|n| *n == 0xFF) {
            self.nodes[free] = node_id;
            self.counts[free] = count;
            return;
        }
        for (n, c) in self.overflow.iter_mut() {
            if *n == node_id {
                *c = count;
                return;
            }
        }
        if self.overflow.len() < MAX_OVERFLOW_NODES {
            self.overflow.push((node_id, count));
        }
    }

    /// Incrementa o contador de um nó (cria com 1 se ausente). Satura em
    /// u64::MAX — nunca decresce (determinístico).
    pub fn tick(&mut self, node_id: u8) {
        let c = self.counter_of(node_id);
        if c == u64::MAX {
            return; // saturado
        }
        self.set_counter(node_id, c.saturating_add(1));
    }

    /// Contador de um nó (0 se ausente) — semântica de relógio vetorial:
    /// nó ausente = contador 0 (nó ausente nunca domina).
    pub fn counter_of(&self, node_id: u8) -> u64 {
        for i in 0..8 {
            if self.nodes[i] == node_id {
                return self.counts[i];
            }
        }
        for &(n, c) in &self.overflow {
            if n == node_id {
                return c;
            }
        }
        0
    }

    /// Causal: `self` aconteceu-antes de `other` sse todo contador de `self`
    /// é ≤ o correspondente em `other` E pelo menos um é estritamente <.
    /// (Relógios vetoriais: `self ≺ other`.) Relógios iguais NÃO são
    /// happened-before. Considera fixos + overflow.
    pub fn happens_before(&self, other: &Self) -> bool {
        if self == other {
            return false;
        }
        let mut strictly_less = false;
        // cada nó de `self` deve ser ≤ em `other`
        for (n, c) in self.iter_nodes() {
            let oc = other.counter_of(n);
            if c > oc {
                return false;
            }
            if c < oc {
                strictly_less = true;
            }
        }
        // cada nó de `other` ausente em `self` (0 < oc) é estritamente maior
        for (n, c) in other.iter_nodes() {
            if self.counter_of(n) < c {
                strictly_less = true;
            }
        }
        strictly_less
    }

    /// Concorrente: nem `self ≺ other` nem `other ≺ self` **e não são
    /// iguais**. Relógios iguais = estado idêntico = SEM conflito; a
    /// concorrência é uma divergência real que deve ser preservada
    /// (CRDT multi-value, Doc 04) — nunca resolvida por LWW cego.
    pub fn concurrent(&self, other: &Self) -> bool {
        self != other && !self.happens_before(other) && !other.happens_before(self)
    }

    /// Merge element-wise (max por nó, união de nós) — considera fixos +
    /// overflow. Satura em u64::MAX (determinístico; nunca decresce).
    pub fn merge(&mut self, other: &Self) {
        for (n, c) in other.iter_nodes() {
            if self.counter_of(n) < c {
                self.set_counter(n, c);
            }
        }
    }

    /// Serializa SÓ os 72B fixos (NMD1) — o overflow NÃO entra no wire
    /// (contrato byte-idêntico com o OS); persiste via `MemoryMeta`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.nodes);
        for c in &self.counts {
            out.extend_from_slice(&c.to_le_bytes());
        }
    }

    pub fn decode(data: &[u8]) -> Option<(Self, usize)> {
        if data.len() < 8 + 8 * 8 {
            return None;
        }
        let mut vc = Self::new();
        vc.nodes.copy_from_slice(&data[0..8]);
        for i in 0..8 {
            let o = 8 + i * 8;
            vc.counts[i] = u64::from_le_bytes(data[o..o + 8].try_into().ok()?);
        }
        Some((vc, 8 + 64))
    }
}

/// Proveniência/identidade de memória (v0.6 — Phase 1: memory identity +
/// provenance). SOMENTE em memória: **não é serializado no NMD1** (contrato
/// byte-idêntico com o OS) — persiste em side-table `sys/meta/` e viaja com
/// o doc na replicação (`Sgdb::put(doc)` preserva `doc.meta`).
///
/// Wire "MDM1" v2 (v0.7 — Phase 3): campos v1 + `version_id` (identidade
/// POR VERSÃO, distinta de `memory_id`, que identifica o SLOT (layer,key)).
/// `decode` aceita v1 (version_id = memory_id — migração explícita, nunca
/// reinterpreta bytes silenciosamente) e v2. Nunca panics em entrada
/// malformada/truncada — retorna Err.
///
/// Layout v1: `magic | ver u8 | id u16len+bytes | source u8 |
/// confidence f32le | importance f32le | created u64le | nparents u16le +
/// (len u16le + bytes)* | noverflow u16le + (node u8 + count u64le)*`.
/// Layout v2: idem + `vid u16len + bytes` no fim.
#[derive(Clone, Debug, PartialEq)]
pub struct MemoryMeta {
    /// Identidade estável do SLOT (32 hex chars). Gerada UMA vez na criação
    /// (`generate_memory_id`), independente de node_id e estável através de
    /// overwrites e replicação — NUNCA re-derivada. `memory_id` identifica a
    /// memória; `version_id` identifica a VERSÃO corrente do DAG causal.
    pub memory_id: String,
    /// Identidade POR VERSÃO (Phase 3, v0.7): cada escrita local que muda o
    /// conteúdo cria uma versão nova (`generate_memory_id` no tick da
    /// escrita) e registra a versão anterior em `parent_ids` (DAG causal).
    /// Igual a `memory_id` na primeira versão do slot. Persistido e
    /// replicado junto com a meta (wire MDM1 v2).
    pub version_id: String,
    /// Nó criador (origem — "quem criou").
    pub source: u8,
    /// Confiança [0..1] na informação.
    pub confidence: f32,
    /// Importância [0..1] para retenção/reforço (default por camada).
    pub importance: f32,
    /// Tick de criação (contador do relógio local do criador).
    pub created_tick: u64,
    /// Pais causais (VERSION ids) — lineage/supersessão/derivação (DAG
    /// causal, Phase 3).
    pub parent_ids: Vec<String>,
    /// Overflow do VectorClock (>8 nós) — o NMD1 guarda só 72B fixos.
    pub clock_overflow: Vec<(u8, u64)>,
    /// Último tick de reforço (v0.9 — `Sgdb::reinforce`): o contador do
    /// relógio próprio no momento do reforço. 0 = nunca reforçada. Exposta
    /// no `explain` e consultável pela política de decay (reforço recente
    /// contrabalança o decaimento).
    pub last_reinforced: u64,
}

/// Um elo da linhagem causal (Phase 3, v0.7): a versão corrente e seus
/// parents (version_ids) resolvidos de volta à storage key. `Sgdb::lineage`
/// caminha a cadeia (parent mais recente) com guarda de ciclos.
#[derive(Clone, Debug, PartialEq)]
pub struct LineageEntry {
    pub version_id: String,
    pub memory_id: String,
    pub storage_key: String,
    pub source: u8,
    pub created_tick: u64,
    pub parent_ids: Vec<String>,
}

const META_MAGIC: &[u8; 4] = b"MDM1";
/// v1 (v0.6): memória + proveniência · v2 (v0.7): version_id · v3 (v0.9):
/// last_reinforced. `decode` aceita as três — migração explícita, nunca
/// reinterpreta bytes antigos.
const META_VERSION: u8 = 3;

impl MemoryMeta {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(48 + self.memory_id.len() + self.parent_ids.len() * 4);
        out.extend_from_slice(META_MAGIC);
        out.push(META_VERSION);
        out.extend_from_slice(&(self.memory_id.len() as u16).to_le_bytes());
        out.extend_from_slice(self.memory_id.as_bytes());
        out.push(self.source);
        out.extend_from_slice(&self.confidence.to_le_bytes());
        out.extend_from_slice(&self.importance.to_le_bytes());
        out.extend_from_slice(&self.created_tick.to_le_bytes());
        out.extend_from_slice(&(self.parent_ids.len() as u16).to_le_bytes());
        for p in &self.parent_ids {
            out.extend_from_slice(&(p.len() as u16).to_le_bytes());
            out.extend_from_slice(p.as_bytes());
        }
        out.extend_from_slice(&(self.clock_overflow.len() as u16).to_le_bytes());
        for &(n, c) in &self.clock_overflow {
            out.push(n);
            out.extend_from_slice(&c.to_le_bytes());
        }
        out.extend_from_slice(&(self.version_id.len() as u16).to_le_bytes());
        out.extend_from_slice(self.version_id.as_bytes());
        // v3: último tick de reforço
        out.extend_from_slice(&self.last_reinforced.to_le_bytes());
        out
    }

    pub fn decode(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 5 || &data[0..4] != META_MAGIC {
            return Err("bad meta magic");
        }
        let ver = data[4];
        if ver != 1 && ver != 2 && ver != 3 {
            return Err("bad meta version");
        }
        let mut off = 5;
        let id_len = rd_u16(data, off).ok_or("trunc idlen")? as usize;
        off += 2;
        if off + id_len > data.len() {
            return Err("trunc id");
        }
        let memory_id: String =
            core::str::from_utf8(&data[off..off + id_len]).map_err(|_| "utf8 id")?.into();
        off += id_len;
        let source = *data.get(off).ok_or("trunc source")?;
        off += 1;
        let confidence = rd_f32(data, off).ok_or("trunc confidence")?;
        off += 4;
        let importance = rd_f32(data, off).ok_or("trunc importance")?;
        off += 4;
        let created_tick = rd_u64(data, off).ok_or("trunc created")?;
        off += 8;
        let nparents = rd_u16(data, off).ok_or("trunc nparents")? as usize;
        off += 2;
        let mut parent_ids = Vec::with_capacity(nparents.min(64));
        for _ in 0..nparents {
            let plen = rd_u16(data, off).ok_or("trunc plen")? as usize;
            off += 2;
            if off + plen > data.len() {
                return Err("trunc parent");
            }
            parent_ids.push(core::str::from_utf8(&data[off..off + plen]).map_err(|_| "utf8 p")?.into());
            off += plen;
        }
        let nover = rd_u16(data, off).ok_or("trunc nover")? as usize;
        off += 2;
        let mut clock_overflow = Vec::with_capacity(nover.min(256));
        for _ in 0..nover {
            let n = *data.get(off).ok_or("trunc over node")?;
            off += 1;
            let c = rd_u64(data, off).ok_or("trunc over count")?;
            off += 8;
            clock_overflow.push((n, c));
        }
        // v2: identidade por versão; v1: migração explícita (versão = slot)
        let version_id = if ver >= 2 {
            let vid_len = rd_u16(data, off).ok_or("trunc vidlen")? as usize;
            off += 2;
            if off + vid_len > data.len() {
                return Err("trunc vid");
            }
            let vid = core::str::from_utf8(&data[off..off + vid_len]).map_err(|_| "utf8 vid")?;
            off += vid_len;
            String::from(vid)
        } else {
            memory_id.clone()
        };
        // v3: último tick de reforço (v1/v2 = nunca reforçada)
        let last_reinforced = if ver >= 3 {
            rd_u64(data, off).ok_or("trunc last_reinforced")?
        } else {
            0
        };
        Ok(MemoryMeta {
            memory_id,
            version_id,
            source,
            confidence,
            importance,
            created_tick,
            parent_ids,
            clock_overflow,
            last_reinforced,
        })
    }
}

/// ID determinístico de memória (v0.6): FNV-1a 128 bits sobre
/// (node_id, created_tick, layer, key) → 32 hex chars. Determinístico por
/// evento de criação; **persistido** em `sys/meta/` e estável através de
/// overwrites e replicação (nunca re-derivado após a criação). Não é node_id
/// e não é um storage key. Colisão: 2^-128; mesmo (node, tick, layer, key)
/// ⇒ mesmo id (testado).
pub fn generate_memory_id(
    node_id: u8,
    created_tick: u64,
    layer: MemoryLayer,
    key: &str,
) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut h0 = FNV_OFFSET;
    let mut h1 = FNV_OFFSET ^ 0x9e37_79b9_7f4a_7c15;
    let mut mix = |b: u8| {
        h0 ^= b as u64;
        h0 = h0.wrapping_mul(FNV_PRIME);
        h1 ^= b as u64;
        h1 = h1.wrapping_mul(FNV_PRIME);
    };
    mix(node_id);
    for b in created_tick.to_le_bytes() {
        mix(b);
    }
    mix(layer as u8);
    for b in key.as_bytes() {
        mix(*b);
    }
    let mut s = String::with_capacity(32);
    for w in [h0, h1] {
        for b in w.to_le_bytes() {
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 0x0F) as usize] as char);
        }
    }
    s
}

/// Tipo de relação associativa (L6, v0.8 — roadmap Phase 12). Memória-NATIVA:
/// persistida em side-table `sys/rel/<kind>/<a>#<b>` (storage = fonte da
/// verdade) e indexada no ART (forward `rel/…` + reverse `rev/…` — índices
/// derivados, reconstruídos no rebuild). Nenhuma inferência aqui: a camada
/// superior (agente/LLM) afirma a relação, o SGDB armazena.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RelationKind {
    /// afim/semântica fraca — simétrica no recall
    RelatedTo = 0,
    /// A --causes--> B (causalidade)
    Causes = 1,
    /// A --supports--> B (evidência)
    Supports = 2,
    /// A --contradicts--> B (contradição explícita)
    Contradicts = 3,
    /// A --derived_from--> B (linhagem semântica; usado pela consolidação)
    DerivedFrom = 4,
    /// A --supersedes--> B (B obsoletado por A; a inversa do supersede de docs)
    Supersedes = 5,
}

impl RelationKind {
    pub const ALL: [RelationKind; 6] = [
        RelationKind::RelatedTo,
        RelationKind::Causes,
        RelationKind::Supports,
        RelationKind::Contradicts,
        RelationKind::DerivedFrom,
        RelationKind::Supersedes,
    ];

    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::RelatedTo),
            1 => Some(Self::Causes),
            2 => Some(Self::Supports),
            3 => Some(Self::Contradicts),
            4 => Some(Self::DerivedFrom),
            5 => Some(Self::Supersedes),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::RelatedTo => "related_to",
            Self::Causes => "causes",
            Self::Supports => "supports",
            Self::Contradicts => "contradicts",
            Self::DerivedFrom => "derived_from",
            Self::Supersedes => "supersedes",
        }
    }

    /// Parse inverso do `as_str`. Nome `from_str` colide com o da trait
    /// `std::str::FromStr` (que retorna `Result`); esta é uma helper com
    /// retorno `Option` — `#[allow]` documentado em vez de renomear (API
    /// pública estável).
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }
}

impl MemoryLayer {
    /// Importância default [0..1] por camada (v0.6) — espelha o peso de
    /// `recall_weighted` (L4/L7 alta, L5 média-alta, L3 média, L2 baixa,
    /// L0/L1 mínima). Aplicada na criação; `set_importance` sobrescreve.
    pub fn default_importance(&self) -> f32 {
        match self {
            MemoryLayer::L4Semantic | MemoryLayer::L7Identity => 1.0,
            MemoryLayer::L5Procedural => 0.8,
            MemoryLayer::L3EpisodicLong => 0.4,
            MemoryLayer::L2EpisodicShort => 0.2,
            MemoryLayer::L0Sensory | MemoryLayer::L1Working => 0.0,
            MemoryLayer::L6Reserved => 0.5,
        }
    }
}

/// Documento de memória — encode length-prefixed (magic "NMD1").
#[derive(Clone, Debug, PartialEq)]
pub struct MemoryDoc {
    pub layer: MemoryLayer,
    pub key: String,
    pub clock: VectorClock,
    pub payload: Vec<u8>,
    /// Opcional: embedding binário (para L4/L5 BQ) — bits empacotados.
    pub bitvec: Option<Vec<u64>>,
    /// Proveniência/identidade (v0.6) — **SOMENTE em memória**; NÃO entra no
    /// NMD1 (contrato byte-idêntico com o OS). Persiste em `sys/meta/` e é
    /// anexada pelo engine no `get`; viaja com o doc na replicação.
    pub meta: Option<MemoryMeta>,
}

const MAGIC: &[u8; 4] = b"NMD1";

impl MemoryDoc {
    pub fn new(layer: MemoryLayer, key: &str, payload: Vec<u8>) -> Self {
        MemoryDoc {
            layer,
            key: String::from(key),
            clock: VectorClock::new(),
            payload,
            bitvec: None,
            meta: None,
        }
    }

    pub fn storage_key(&self) -> String {
        alloc::format!("md/{}/{}", self.layer.as_str(), self.key)
    }

    /// NoProto-pattern: troca payload (+ clock tick) sem mudar layer/key.
    pub fn patch_payload(&mut self, new_payload: Vec<u8>, node_id: u8) {
        self.payload = new_payload;
        self.clock.tick(node_id);
    }

    /// Key temporal byte-wise sortable (u64 BE hex) — scan_prefix `ts/`.
    pub fn sortable_ts_key(tick: u64) -> String {
        alloc::format!("ts/{:016x}", tick)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.push(self.layer as u8);
        let kb = self.key.as_bytes();
        out.extend_from_slice(&(kb.len() as u32).to_le_bytes());
        out.extend_from_slice(kb);
        self.clock.encode(&mut out);
        out.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.payload);
        match &self.bitvec {
            Some(bv) => {
                out.push(1);
                out.extend_from_slice(&(bv.len() as u32).to_le_bytes());
                for w in bv {
                    out.extend_from_slice(&w.to_le_bytes());
                }
            }
            None => out.push(0),
        }
        out
    }

    pub fn decode(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 5 || &data[0..4] != MAGIC {
            return Err("bad magic");
        }
        let layer = MemoryLayer::from_u8(data[4]).ok_or("bad layer")?;
        let mut off = 5;
        // Parsing safety (maturation P2): leitura checada, sem unwrap em dados
        // externos — helpers `rd_u32`/`rd_u64` retornam Option (bounds-checked)
        let klen = rd_u32(data, off).ok_or("trunc keylen")? as usize;
        off += 4;
        if off + klen > data.len() {
            return Err("trunc key");
        }
        let key = core::str::from_utf8(&data[off..off + klen])
            .map_err(|_| "utf8")?
            .into();
        off += klen;
        let (clock, n) = VectorClock::decode(&data[off..]).ok_or("trunc clock")?;
        off += n;
        let plen = rd_u32(data, off).ok_or("trunc plen")? as usize;
        off += 4;
        if off + plen > data.len() {
            return Err("trunc payload");
        }
        let payload = data[off..off + plen].to_vec();
        off += plen;
        if off >= data.len() {
            return Err("trunc bitflag");
        }
        let has_bv = data[off];
        off += 1;
        let bitvec = if has_bv == 1 {
            let n = rd_u32(data, off).ok_or("trunc bvlen")? as usize;
            off += 4;
            if off + n * 8 > data.len() {
                return Err("trunc bv");
            }
            let mut bv = Vec::with_capacity(n);
            for _ in 0..n {
                bv.push(rd_u64(data, off).ok_or("trunc bv")?);
                off += 8;
            }
            Some(bv)
        } else {
            None
        };
        Ok(MemoryDoc {
            layer,
            key,
            clock,
            payload,
            bitvec,
            meta: None, // meta NÃO é serializado no NMD1 — engine anexa via sys/meta/
        })
    }
}

/// Memória completa como uma UNIDADE de replicação (v0.6 — P0-5): doc NMD1 +
/// estado lógico + janela de validade, serializados juntos.
///
/// Resolve a contradição #2 (docs/implementation-status §6): o diff/pull
/// doc-a-doc silenciosamente descartava `sys/state/`/`sys/validity/` na
/// replicação — aqui estado e validade VIAJAM com o doc. `meta` não é
/// serializado aqui (não está no NMD1): viaja dentro de `doc.meta` em
/// memória, e o importador a preserva (identidade do criador).
///
/// Wire "MDR1" v1: `magic | state u8 | vflag u8 [from u64le until u64le] |
/// metaflag u8 [mlen u32le + meta "MDM1"] | nmd1 bytes`. A meta (identidade
/// e proveniência) viaja NO record para que um delta serializado não perca o
/// criador. `decode` nunca panics em entrada malformada/truncada (parsing
/// safety) — retorna Err.
#[derive(Clone, Debug, PartialEq)]
pub struct MemoryRecord {
    pub doc: MemoryDoc,
    pub state: MemoryState,
    /// Janela `from ≤ now < until` (None = sempre válido).
    pub validity: Option<(u64, u64)>,
}

const REC_MAGIC: &[u8; 4] = b"MDR1";

impl MemoryRecord {
    pub fn new(doc: MemoryDoc, state: MemoryState, validity: Option<(u64, u64)>) -> Self {
        MemoryRecord {
            doc,
            state,
            validity,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(24 + self.doc.encode().len());
        out.extend_from_slice(REC_MAGIC);
        out.push(self.state as u8);
        match self.validity {
            Some((from, until)) => {
                out.push(1);
                out.extend_from_slice(&from.to_le_bytes());
                out.extend_from_slice(&until.to_le_bytes());
            }
            None => out.push(0),
        }
        match &self.doc.meta {
            Some(m) => {
                out.push(1);
                let m = m.encode();
                out.extend_from_slice(&(m.len() as u32).to_le_bytes());
                out.extend_from_slice(&m);
            }
            None => out.push(0),
        }
        out.extend_from_slice(&self.doc.encode());
        out
    }

    /// Bounds-checked: Err em magic errado, estado inválido, validade/meta
    /// truncadas ou NMD1 truncado — nunca panics (fuzz-tested).
    pub fn decode(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 6 || &data[0..4] != REC_MAGIC {
            return Err("bad record magic");
        }
        let state = MemoryState::from_u8(data[4]).ok_or("bad record state")?;
        let vflag = data[5];
        let mut off = 6;
        let validity = match vflag {
            0 => None,
            1 => {
                if off + 16 > data.len() {
                    return Err("trunc validity");
                }
                let from = u64::from_le_bytes(data[off..off + 8].try_into().map_err(|_| "v")?);
                let until =
                    u64::from_le_bytes(data[off + 8..off + 16].try_into().map_err(|_| "v")?);
                off += 16;
                Some((from, until))
            }
            _ => return Err("bad validity flag"),
        };
        let meta = match data.get(off).copied().ok_or("trunc metaflag")? {
            0 => {
                off += 1;
                None
            }
            1 => {
                off += 1;
                let mlen = rd_u32(data, off).ok_or("trunc metalen")? as usize;
                off += 4;
                if off + mlen > data.len() {
                    return Err("trunc meta");
                }
                let m = MemoryMeta::decode(&data[off..off + mlen]).map_err(|_| "bad meta")?;
                off += mlen;
                Some(m)
            }
            _ => return Err("bad meta flag"),
        };
        let mut doc = MemoryDoc::decode(&data[off..]).map_err(|_| "trunc nmd1")?;
        doc.meta = meta;
        Ok(MemoryRecord {
            doc,
            state,
            validity,
        })
    }
}

/// Lê u32 LE em `off` sem unwrap — `None` se fora dos limites
/// (parsing safety, maturation P2).
fn rd_u32(data: &[u8], off: usize) -> Option<u32> {
    let b = data.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Lê u16 LE em `off` sem unwrap.
fn rd_u16(data: &[u8], off: usize) -> Option<u16> {
    let b = data.get(off..off.checked_add(2)?)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

/// Lê f32 LE em `off` sem unwrap.
fn rd_f32(data: &[u8], off: usize) -> Option<f32> {
    let b = data.get(off..off.checked_add(4)?)?;
    Some(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Lê u64 LE em `off` sem unwrap — `None` se fora dos limites.
fn rd_u64(data: &[u8], off: usize) -> Option<u64> {
    let b = data.get(off..off.checked_add(8)?)?;
    Some(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

/// Overlay zero-copy sobre buffer NMD1 (sem clonar payload).
pub struct MemoryDocView<'a> {
    data: &'a [u8],
    layer: MemoryLayer,
    key: &'a str,
    payload: &'a [u8],
    clock_off: usize,
}

impl<'a> MemoryDocView<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, &'static str> {
        if data.len() < 5 || &data[0..4] != MAGIC {
            return Err("bad magic");
        }
        let layer = MemoryLayer::from_u8(data[4]).ok_or("bad layer")?;
        let mut off = 5;
        if off + 4 > data.len() {
            return Err("trunc keylen");
        }
        let klen = u32::from_le_bytes(data[off..off + 4].try_into().unwrap()) as usize;
        off += 4;
        if off + klen > data.len() {
            return Err("trunc key");
        }
        let key = core::str::from_utf8(&data[off..off + klen]).map_err(|_| "utf8")?;
        off += klen;
        let clock_off = off;
        if off + 72 > data.len() {
            return Err("trunc clock");
        }
        off += 72; // VectorClock fixed size
        let plen = rd_u32(data, off).ok_or("trunc plen")? as usize;
        off += 4;
        if off + plen > data.len() {
            return Err("trunc payload");
        }
        let payload = &data[off..off + plen];
        Ok(MemoryDocView {
            data,
            layer,
            key,
            payload,
            clock_off,
        })
    }

    pub fn layer(&self) -> MemoryLayer {
        self.layer
    }
    pub fn key(&self) -> &str {
        self.key
    }
    pub fn payload(&self) -> &[u8] {
        self.payload
    }
    pub fn clock_bytes(&self) -> &[u8] {
        &self.data[self.clock_off..self.clock_off + 72]
    }
    pub fn to_owned_doc(&self) -> Result<MemoryDoc, &'static str> {
        MemoryDoc::decode(self.data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec; // no_std test builds: `vec!` não está no prelude

    #[test]
    fn roundtrip_doc() {
        let mut doc = MemoryDoc::new(MemoryLayer::L1Working, "hello", b"world".to_vec());
        doc.clock.tick(1);
        let enc = doc.encode();
        assert_eq!(&enc[0..4], b"NMD1");
        let view = MemoryDocView::parse(&enc).unwrap();
        assert_eq!(view.key(), "hello");
        assert_eq!(view.payload(), b"world");
        assert_eq!(view.clock_bytes().len(), 72);
        let dec = MemoryDoc::decode(&enc).unwrap();
        assert_eq!(dec.key, "hello");
        assert_eq!(dec.payload, b"world");
        assert_eq!(dec.layer, MemoryLayer::L1Working);
    }

    #[test]
    fn roundtrip_with_bitvec() {
        let mut doc = MemoryDoc::new(MemoryLayer::L4Semantic, "emb1", vec![0u8; 16]);
        doc.bitvec = Some(vec![0xDEAD_BEEF_CAFE_F00Du64, 0x1234]);
        let enc = doc.encode();
        let dec = MemoryDoc::decode(&enc).unwrap();
        assert_eq!(dec.bitvec, Some(vec![0xDEAD_BEEF_CAFE_F00Du64, 0x1234]));
    }

    #[test]
    fn bad_magic_rejected() {
        assert!(MemoryDoc::decode(b"XXXXgarbage").is_err());
        assert!(MemoryDocView::parse(b"XXXX").is_err());
    }

    /// Fuzz determinístico (maturation P6): decoders NUNCA panics com entrada
    /// corrompida/truncada — falha segura (Err) em vez de panic.
    #[test]
    fn decode_never_panics_on_malformed() {
        // sementes determinísticas de LCG — entrada adversarial variada
        let mut state = 0x1234_5678_9ABC_DEF0u64;
        let bytes = |n: usize, s: &mut u64| -> Vec<u8> {
            (0..n)
                .map(|_| {
                    *s = s.wrapping_mul(1103515245).wrapping_add(12345);
                    ((*s >> 32) & 0xFF) as u8
                })
                .collect()
        };
        for len in 0..64usize {
            for _ in 0..32 {
                let data = bytes(len, &mut state);
                // decode e view nunca panics
                let _ = MemoryDoc::decode(&data);
                let _ = MemoryDocView::parse(&data);
            }
        }
        // casos específicos: magic ok mas campos truncados
        let mut good = MemoryDoc::new(MemoryLayer::L4Semantic, "k", vec![0xAA]);
        good.bitvec = Some(vec![1, 2, 3]);
        let enc = good.encode();
        for cut in 0..enc.len() {
            let _ = MemoryDoc::decode(&enc[..cut]); // truncado em todo ponto
        }
    }

    #[test]
    fn golden_nmd1_bytes() {
        // Byte-exact golden (P2 — format versioning): fixed NMD1 layout by
        // contract with the OS. doc L1 "k" payload [0xAA], no clock tick, no
        // bitvec:
        //   magic NMD1 | layer 0x01 | klen 0x01 u32le | 'k' |
        //   VectorClock 72B (8×0xFF nodes + 8×0 u64) | plen 0x01 u32le | 0xAA |
        //   bitflag 0x00
        let doc = MemoryDoc::new(MemoryLayer::L1Working, "k", vec![0xAA]);
        let enc = doc.encode();
        let mut want: Vec<u8> = Vec::new();
        want.extend_from_slice(b"NMD1");
        want.push(0x01); // L1
        want.extend_from_slice(&1u32.to_le_bytes());
        want.push(b'k');
        want.extend_from_slice(&[0xFFu8; 8]); // nodes
        want.extend_from_slice(&[0u8; 64]); // counts
        want.extend_from_slice(&1u32.to_le_bytes());
        want.push(0xAA);
        want.push(0x00); // bitflag: sem bitvec
        assert_eq!(enc, want);
    }

    // ── VectorClock: causal ordering, concurrency, merge (P2) ──────────────

    fn vc(pairs: &[(u8, u64)]) -> VectorClock {
        let mut c = VectorClock::new();
        for &(n, cnt) in pairs {
            for _ in 0..cnt {
                c.tick(n);
            }
        }
        c
    }

    #[test]
    fn vc_equal() {
        let a = vc(&[(1, 3), (2, 1)]);
        let b = vc(&[(2, 1), (1, 3)]); // mesma contagem, ordem de insert diferente
        assert_eq!(a, b);
        assert!(!a.happens_before(&b));
        assert!(!b.happens_before(&a));
        assert!(!a.concurrent(&b)); // iguais não são concorrentes
        assert!(!b.concurrent(&a));
    }

    #[test]
    fn vc_happens_before_less() {
        let a = vc(&[(1, 2)]);
        let b = vc(&[(1, 3)]);
        assert!(a.happens_before(&b)); // a ≺ b (contador menor)
        assert!(!b.happens_before(&a));
        assert!(!a.concurrent(&b));

        // nó presente em b e ausente em a → a ≺ b (0 < oc)
        let c = vc(&[]);
        let d = vc(&[(5, 1)]);
        assert!(c.happens_before(&d));
        assert!(!d.happens_before(&c));
    }

    #[test]
    fn vc_happens_before_greater() {
        let a = vc(&[(1, 5)]);
        let b = vc(&[(1, 2)]);
        assert!(b.happens_before(&a));
        assert!(!a.happens_before(&b));
    }

    #[test]
    fn vc_concurrent_detection() {
        // a incrementou nó 1; b incrementou nó 2 — incomparáveis → concorrentes
        let a = vc(&[(1, 1)]);
        let b = vc(&[(2, 1)]);
        assert!(!a.happens_before(&b));
        assert!(!b.happens_before(&a));
        assert!(a.concurrent(&b));
        assert!(b.concurrent(&a));

        // mesmo nó, contadores iguais, mas com divergência em outro nó
        let c = vc(&[(1, 2), (3, 1)]);
        let d = vc(&[(1, 2), (4, 1)]);
        assert!(c.concurrent(&d));
    }

    #[test]
    fn vc_merge_union_and_max() {
        let mut a = vc(&[(1, 2)]);
        let b = vc(&[(2, 3)]);
        a.merge(&b);
        assert_eq!(a.counter_of(1), 2);
        assert_eq!(a.counter_of(2), 3);

        // max por nó (não soma)
        let mut c = vc(&[(1, 2)]);
        let d = vc(&[(1, 5)]);
        c.merge(&d);
        assert_eq!(c.counter_of(1), 5);
    }

    #[test]
    fn vc_merge_commutative() {
        let a = vc(&[(1, 2), (2, 1)]);
        let b = vc(&[(1, 1), (2, 3)]);
        let mut ab = a.clone();
        ab.merge(&b);
        let mut ba = b.clone();
        ba.merge(&a);
        assert_eq!(ab, ba); // merge é comutativo e idempotente
        assert_eq!(ab.counter_of(1), 2);
        assert_eq!(ab.counter_of(2), 3);
    }

    #[test]
    fn vc_overflow_saturates() {
        let mut c = VectorClock::new();
        c.nodes[0] = 1;
        c.counts[0] = u64::MAX;
        c.tick(1); // satura, não wrappa
        assert_eq!(c.counts[0], u64::MAX);
    }

    #[test]
    fn vc_encode_decode_roundtrip() {
        let c = vc(&[(1, 3), (2, 1), (7, 9)]);
        let mut enc = Vec::new();
        c.encode(&mut enc);
        let (dec, n) = VectorClock::decode(&enc).unwrap();
        assert_eq!(n, 72);
        assert_eq!(dec, c);
        // decode rejeita truncado
        assert!(VectorClock::decode(&enc[..70]).is_none());
    }

    // ── VectorClock dinâmico (>8 nós, v0.6) ────────────────────────────────

    #[test]
    fn vc_dynamic_nodes_beyond_eight() {
        let mut c = VectorClock::new();
        for n in 1..=9u8 {
            c.tick(n);
        }
        assert_eq!(c.counter_of(9), 1);
        assert_eq!(c.counter_of(1), 1);
        // fast path: ≤8 nós não toca o overflow
        let mut d = VectorClock::new();
        for n in 1..=8u8 {
            d.tick(n);
        }
        assert!(d.overflow.is_empty());
        // NMD1 = 72B fixos; o overflow NÃO entra no wire (por design)
        let mut enc = Vec::new();
        c.encode(&mut enc);
        assert_eq!(enc.len(), 72);
        let (dec, _) = VectorClock::decode(&enc).unwrap();
        assert_eq!(dec.counter_of(9), 0); // overflow perdido no NMD1
        assert_eq!(dec.counter_of(1), 1); // fixo sobrevive
        // igualdade semântica inclui o overflow
        let mut c2 = VectorClock::new();
        for n in 1..=9u8 {
            c2.tick(n);
        }
        assert_eq!(c, c2);
    }

    #[test]
    fn vc_dynamic_happens_before_and_concurrent() {
        // 10 nós: b (1..=9) ≺ a (1..=10)
        let mut a = VectorClock::new();
        for n in 1..=10u8 {
            a.tick(n);
        }
        let mut b = VectorClock::new();
        for n in 1..=9u8 {
            b.tick(n);
        }
        assert!(b.happens_before(&a));
        assert!(!a.happens_before(&b));
        assert!(!a.concurrent(&b));
        // concorrência via overflow (nós 11 e 12)
        let mut c = VectorClock::new();
        c.tick(11);
        let mut d = VectorClock::new();
        d.tick(12);
        assert!(c.concurrent(&d));
        assert!(d.concurrent(&c));
    }

    #[test]
    fn vc_dynamic_merge_union_max_and_properties() {
        let mut a = VectorClock::new();
        a.tick(1);
        for n in 2..=10u8 {
            a.tick(n); // 2..=9 fixos, 10 overflow
        }
        let mut b = VectorClock::new();
        b.tick(9);
        b.tick(10);
        b.tick(10); // contador 2
        b.tick(2);
        b.tick(2); // contador 2
        a.merge(&b);
        assert_eq!(a.counter_of(1), 1);
        assert_eq!(a.counter_of(2), 2); // max (1 vs 2)
        assert_eq!(a.counter_of(9), 1); // max (1 vs 1)
        assert_eq!(a.counter_of(10), 2); // max no overflow (1 vs 2)
        // merge nunca decresce: re-merge é no-op
        let snapshot = a.clone();
        a.merge(&b);
        assert_eq!(a, snapshot);
        // comutativo + idempotente (propriedade CRDT, item 30)
        let mut ba = b.clone();
        ba.merge(&a);
        assert_eq!(ba, a);
        let mut aa = a.clone();
        aa.merge(&a);
        assert_eq!(aa, a);
    }

    #[test]
    fn vc_bounded_node_policy() {
        // registro dinâmico limitado (nunca cresce sem limite)
        let mut c = VectorClock::new();
        for n in 0..=255u8 {
            c.set_counter(n, 1);
        }
        let total = 8 + c.overflow.len();
        assert!(total <= 256);
        // nós no limite ainda respondem
        assert_eq!(c.counter_of(255), 1);
        assert_eq!(c.counter_of(9), 1);
    }

    #[test]
    fn vc_tick_saturates_dynamic() {
        let mut c = VectorClock::new();
        c.set_counter(200, u64::MAX);
        c.tick(200); // satura, não wrappa
        assert_eq!(c.counter_of(200), u64::MAX);
    }

    // ── MemoryMeta: roundtrip + parsing safety (v0.6) ──────────────────────

    fn sample_meta() -> MemoryMeta {
        MemoryMeta {
            memory_id: String::from("aabbccddeeff00112233445566778899"),
            version_id: String::from("00112233445566778899aabbccddeeff"),
            source: 7,
            confidence: 0.9,
            importance: 0.6,
            created_tick: 42,
            parent_ids: vec![String::from("p1"), String::from("p2")],
            clock_overflow: vec![(9, 3), (12, 1)],
            last_reinforced: 99,
        }
    }

    #[test]
    fn meta_roundtrip() {
        let m = sample_meta();
        let dec = MemoryMeta::decode(&m.encode()).unwrap();
        assert_eq!(dec, m);
        // meta vazia (sem parents/overflow)
        let m2 = MemoryMeta {
            memory_id: String::from("00"),
            parent_ids: Vec::new(),
            clock_overflow: Vec::new(),
            ..sample_meta()
        };
        let dec2 = MemoryMeta::decode(&m2.encode()).unwrap();
        assert_eq!(dec2, m2);
    }

    #[test]
    fn meta_v2_roundtrip_preserves_version_id() {
        let m = sample_meta();
        assert_ne!(m.version_id, m.memory_id);
        let dec = MemoryMeta::decode(&m.encode()).unwrap();
        assert_eq!(dec, m);
        assert_eq!(dec.version_id, m.version_id);
    }

    #[test]
    fn meta_v1_decodes_with_slot_version_migration() {
        // v1 (pré-Phase 3): sem version_id — migração EXPLÍCITA p/ version_id
        // = memory_id (a 1ª versão de um slot é o próprio slot). O v1 não
        // é reinterpretado silenciosamente: o decode conhece os dois layouts.
        let mut enc = sample_meta().encode();
        // remove o campo v3 (last_reinforced u64) + v2 (vidlen u16 + vid) e
        // marca ver=1 — layout v1 genuíno (sem vid, sem lr)
        let vid = sample_meta().version_id;
        let cut = enc.len() - 8 - 2 - vid.len();
        enc.truncate(cut);
        enc[4] = 1;
        let dec = MemoryMeta::decode(&enc).unwrap();
        assert_eq!(dec.version_id, dec.memory_id);
        assert_eq!(dec.version_id, "aabbccddeeff00112233445566778899");
        assert_eq!(dec.last_reinforced, 0, "v1 nunca reforçada");
        // v2 (sem last_reinforced) também decodifica com lr=0
        let mut enc2 = sample_meta().encode();
        let cut2 = enc2.len() - 8;
        enc2.truncate(cut2);
        enc2[4] = 2;
        let dec2 = MemoryMeta::decode(&enc2).unwrap();
        assert_eq!(dec2.last_reinforced, 0);
        assert_eq!(dec2.version_id, sample_meta().version_id);
        // versão desconhecida → Err
        let mut bad = sample_meta().encode();
        bad[4] = 4;
        assert!(MemoryMeta::decode(&bad).is_err());
        // truncado no vid → Err, nunca panic
        let full = sample_meta().encode();
        for cut in 0..full.len() {
            let _ = MemoryMeta::decode(&full[..cut]);
        }
    }

    #[test]
    fn meta_never_panics_on_malformed() {
        let mut state = 0xDEAD_BEEF_CAFE_F00Du64;
        let bytes = |n: usize, s: &mut u64| -> Vec<u8> {
            (0..n)
                .map(|_| {
                    *s = s.wrapping_mul(1103515245).wrapping_add(12345);
                    ((*s >> 32) & 0xFF) as u8
                })
                .collect()
        };
        for len in 0..96usize {
            for _ in 0..16 {
                let _ = MemoryMeta::decode(&bytes(len, &mut state));
            }
        }
        // magic ok mas campos truncados — corte em todo ponto
        let enc = sample_meta().encode();
        for cut in 0..enc.len() {
            let _ = MemoryMeta::decode(&enc[..cut]);
        }
        // magic errado / versão desconhecida
        assert!(MemoryMeta::decode(b"XXXX").is_err());
        let mut bad = sample_meta().encode();
        bad[4] = 99;
        assert!(MemoryMeta::decode(&bad).is_err());
    }

    #[test]
    fn memory_id_deterministic_and_distinct() {
        let id1 = generate_memory_id(3, 7, MemoryLayer::L4Semantic, "k");
        let id2 = generate_memory_id(3, 7, MemoryLayer::L4Semantic, "k");
        assert_eq!(id1, id2); // determinístico
        assert_eq!(id1.len(), 32); // 128 bits hex
        assert!(id1.chars().all(|c| c.is_ascii_hexdigit()));
        // distinto por nó / tick / layer / key
        assert_ne!(id1, generate_memory_id(4, 7, MemoryLayer::L4Semantic, "k"));
        assert_ne!(id1, generate_memory_id(3, 8, MemoryLayer::L4Semantic, "k"));
        assert_ne!(id1, generate_memory_id(3, 7, MemoryLayer::L3EpisodicLong, "k"));
        assert_ne!(id1, generate_memory_id(3, 7, MemoryLayer::L4Semantic, "k2"));
    }

    #[test]
    fn doc_with_meta_encodes_byte_identical() {
        // o NMD1 NUNCA vê a meta — contrato byte-idêntico preservado
        let mut a = MemoryDoc::new(MemoryLayer::L4Semantic, "k", vec![1, 2, 3, 4]);
        let b = a.clone();
        a.meta = Some(sample_meta());
        assert_eq!(a.encode(), b.encode());
        // e decode deixa meta = None (o engine anexa via sys/meta/)
        let dec = MemoryDoc::decode(&a.encode()).unwrap();
        assert!(dec.meta.is_none());
    }

    #[test]
    fn layer_default_importance_sane() {
        assert_eq!(MemoryLayer::L4Semantic.default_importance(), 1.0);
        assert_eq!(MemoryLayer::L1Working.default_importance(), 0.0);
        assert!(MemoryLayer::L3EpisodicLong.default_importance() < 0.5);
        assert!(MemoryLayer::L5Procedural.default_importance() > 0.5);
    }

    // ── Merge associativo (propriedade CRDT — item 30) ─────────────────────

    #[test]
    fn vc_merge_associative() {
        // merge(merge(a,b),c) == merge(a, merge(b,c)) — a ordem de entrega
        // nunca muda o ponto-fixo.
        let a = vc(&[(1, 2), (2, 3)]);
        let b = vc(&[(2, 1), (3, 5)]);
        let c = vc(&[(1, 5), (4, 2)]);
        let mut ab_c = a.clone();
        ab_c.merge(&b);
        ab_c.merge(&c);
        let mut bc = b.clone();
        bc.merge(&c);
        let mut a_bc = a.clone();
        a_bc.merge(&bc);
        assert_eq!(ab_c, a_bc);
        // ponto-fixo: re-merge de `c` não muda nada
        let snapshot = ab_c.clone();
        ab_c.merge(&c);
        assert_eq!(ab_c, snapshot);
    }

    #[test]
    fn vc_entries_accessor() {
        let mut c = VectorClock::new();
        c.tick(3);
        c.tick(3);
        c.tick(9); // overflow (>8 nós)
        let mut e = c.entries();
        e.sort();
        assert_eq!(e, vec![(3, 2), (9, 1)]);
    }

    // ── MemoryRecord: unidade de replicação doc + estado + validade (P0-5) ─

    #[test]
    fn memory_record_roundtrip() {
        let mut doc = MemoryDoc::new(MemoryLayer::L4Semantic, "k", vec![1, 2, 3, 4]);
        doc.clock.tick(1);
        doc.meta = Some(sample_meta());
        let rec = MemoryRecord::new(doc.clone(), MemoryState::Superseded, Some((10, 200)));
        let dec = MemoryRecord::decode(&rec.encode()).unwrap();
        assert_eq!(dec, rec);
        // sem validade
        let rec2 = MemoryRecord::new(doc, MemoryState::Active, None);
        let dec2 = MemoryRecord::decode(&rec2.encode()).unwrap();
        assert_eq!(dec2, rec2);
    }

    #[test]
    fn memory_record_never_panics_on_malformed() {
        // fuzz determinístico LCG — nunca panics
        let mut state = 0xFEED_FACE_CAFE_BEEFu64;
        let bytes = |n: usize, s: &mut u64| -> Vec<u8> {
            (0..n)
                .map(|_| {
                    *s = s.wrapping_mul(1103515245).wrapping_add(12345);
                    ((*s >> 32) & 0xFF) as u8
                })
                .collect()
        };
        for len in 0..64usize {
            for _ in 0..16 {
                let _ = MemoryRecord::decode(&bytes(len, &mut state));
            }
        }
        // magic ok, truncado em todo ponto
        let rec = MemoryRecord::new(
            MemoryDoc::new(MemoryLayer::L4Semantic, "k", vec![1, 2, 3, 4]),
            MemoryState::Archived,
            Some((1, 2)),
        );
        let enc = rec.encode();
        for cut in 0..enc.len() {
            let _ = MemoryRecord::decode(&enc[..cut]);
        }
        // estado inválido / flag de validade inválida → Err, nunca panic
        let mut bad = rec.encode();
        bad[4] = 99;
        assert!(MemoryRecord::decode(&bad).is_err());
        let mut bad2 = rec.encode();
        bad2[5] = 2;
        assert!(MemoryRecord::decode(&bad2).is_err());
        assert!(MemoryRecord::decode(b"XXXX").is_err());
    }
}
