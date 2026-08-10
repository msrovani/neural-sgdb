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

/// Relógio vetorial simples (até 8 nós).
#[derive(Clone, Debug, Default)]
pub struct VectorClock {
    /// Pares (node_id, counter) densos; slots não usados = 0xFF / 0.
    pub nodes: [u8; 8],
    pub counts: [u64; 8],
}

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
        if off + 4 > data.len() {
            return Err("trunc keylen");
        }
        let klen = u32::from_le_bytes(data[off..off + 4].try_into().unwrap()) as usize;
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
        if off + 4 > data.len() {
            return Err("trunc plen");
        }
        let plen = u32::from_le_bytes(data[off..off + 4].try_into().unwrap()) as usize;
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
            if off + 4 > data.len() {
                return Err("trunc bvlen");
            }
            let n = u32::from_le_bytes(data[off..off + 4].try_into().unwrap()) as usize;
            off += 4;
            if off + n * 8 > data.len() {
                return Err("trunc bv");
            }
            let mut bv = Vec::with_capacity(n);
            for _ in 0..n {
                bv.push(u64::from_le_bytes(data[off..off + 8].try_into().unwrap()));
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
        if off + 4 > data.len() {
            return Err("trunc plen");
        }
        let plen = u32::from_le_bytes(data[off..off + 4].try_into().unwrap()) as usize;
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
}
