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
}

impl MemoryState {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Active),
            1 => Some(Self::Superseded),
            2 => Some(Self::Archived),
            3 => Some(Self::Invalidated),
            _ => None,
        }
    }
}

/// Relógio vetorial simples (até 8 nós).
#[derive(Clone, Debug, Default)]
pub struct VectorClock {
    /// Pares (node_id, counter) densos; slots não usados = 0xFF / 0.
    pub nodes: [u8; 8],
    pub counts: [u64; 8],
}

/// Igualdade SEMÂNTICA: dois relógios são iguais sse o mapeamento
/// (nó → contador) é idêntico — **independente da ordem de inserção nos
/// slots**. O derive compararia slots por posição, o que faria dois relógios
/// com a mesma causalidade (inseridos em ordens diferentes) serem "desiguais".
impl PartialEq for VectorClock {
    fn eq(&self, other: &Self) -> bool {
        for i in 0..8 {
            let n = self.nodes[i];
            if n != 0xFF && other.counter_of(n) != self.counts[i] {
                return false;
            }
        }
        for j in 0..8 {
            let n = other.nodes[j];
            if n != 0xFF && self.counter_of(n) != other.counts[j] {
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
        }
    }

    pub fn tick(&mut self, node_id: u8) {
        for i in 0..8 {
            if self.nodes[i] == node_id {
                self.counts[i] = self.counts[i].saturating_add(1);
                return;
            }
            if self.nodes[i] == 0xFF {
                self.nodes[i] = node_id;
                self.counts[i] = 1;
                return;
            }
        }
    }

    /// Contador de um nó (0 se ausente) — semântica de relógio vetorial:
    /// nó ausente = contador 0 (v0.2 design: nó ausente nunca domina).
    pub fn counter_of(&self, node_id: u8) -> u64 {
        for i in 0..8 {
            if self.nodes[i] == node_id {
                return self.counts[i];
            }
        }
        0
    }

    /// Causal: `self` aconteceu-antes de `other` sse todo contador de `self`
    /// é ≤ o correspondente em `other` E pelo menos um é estritamente <.
    /// (Relógios vetoriais: `self ≺ other`.) Relógios iguais NÃO são
    /// happened-before.
    pub fn happens_before(&self, other: &Self) -> bool {
        if self == other {
            return false;
        }
        let mut strictly_less = false;
        // cada nó de `self` deve ser ≤ em `other`
        for i in 0..8 {
            let sn = self.nodes[i];
            if sn == 0xFF {
                continue;
            }
            let sc = self.counts[i];
            let oc = other.counter_of(sn);
            if sc > oc {
                return false;
            }
            if sc < oc {
                strictly_less = true;
            }
        }
        // cada nó de `other` presente e não em `self` é estritamente maior (0 < oc)
        for j in 0..8 {
            let on = other.nodes[j];
            if on == 0xFF {
                continue;
            }
            if self.counter_of(on) < other.counts[j] {
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

    /// Merge element-wise (max por nó, união de nós). Overflow: satura em
    /// u64::MAX (determinístico; nunca decresce).
    pub fn merge(&mut self, other: &Self) {
        for j in 0..8 {
            let on = other.nodes[j];
            if on == 0xFF {
                continue;
            }
            let oc = other.counts[j];
            match self.nodes.iter().position(|n| *n == on) {
                Some(slot) => self.counts[slot] = self.counts[slot].max(oc),
                None => {
                    // nó novo — ocupa slot livre; cheio → satura (v0.2: clock dinâmico)
                    if let Some(free) = self.nodes.iter().position(|n| *n == 0xFF) {
                        self.nodes[free] = on;
                        self.counts[free] = oc;
                    }
                }
            }
        }
    }

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

/// Documento de memória — encode length-prefixed (magic "NMD1").
#[derive(Clone, Debug)]
pub struct MemoryDoc {
    pub layer: MemoryLayer,
    pub key: String,
    pub clock: VectorClock,
    pub payload: Vec<u8>,
    /// Opcional: embedding binário (para L4/L5 BQ) — bits empacotados.
    pub bitvec: Option<Vec<u64>>,
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
        })
    }
}

/// Lê u32 LE em `off` sem unwrap — `None` se fora dos limites
/// (parsing safety, maturation P2).
fn rd_u32(data: &[u8], off: usize) -> Option<u32> {
    let b = data.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
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
}
