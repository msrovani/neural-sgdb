//! Modelo de conflito de primeira classe (v0.9, roadmap Phase 14).
//!
//! O CRDT **detecta/preserva** o conflito (`merge_remote` → `Conflict`);
//! esta camada o torna um objeto persistido e consultável — a camada
//! cognitiva **interpreta/decide** (Phase 15: `resolve_conflict` /
//! `merge_memories`). Nenhuma decisão semântica vive aqui.
//!
//! Persistência: side-table `sys/conflict/<conflict_id>` (storage = fonte da
//! verdade — NMD1 intacto). O `conflict_id` é DETERMINÍSTICO sobre
//! (subject, candidatos ordenados): reentregar/re-mergear o mesmo par de
//! versões concorrentes faz upsert, nunca duplica.

use alloc::string::String;
use alloc::vec::Vec;

/// Status de um conflito. `Resolved` carrega o vencedor (version_id).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictStatus {
    Open = 0,
    Resolved = 1,
}

impl ConflictStatus {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Open),
            1 => Some(Self::Resolved),
            _ => None,
        }
    }
}

/// Conflito persistido: o que a camada superior precisa para decidir sem
/// esconder evidência (roadmap §19).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictRecord {
    /// Determinístico: FNV-1a 128 sobre (subject, candidatos ordenados).
    pub conflict_id: String,
    /// Sujeito lógico — storage key do slot onde as versões colidiram.
    pub subject: String,
    /// Candidatos concorrentes (VERSION ids), ordenados.
    pub candidates: Vec<String>,
    /// Nós fonte dos candidatos (ordenados, sem duplicata).
    pub nodes: Vec<u8>,
    /// Tick de criação (relógio local do nó que registrou o conflito).
    pub created_tick: u64,
    pub status: ConflictStatus,
    /// Vencedor (version_id) quando `Resolved`.
    pub resolved_winner: Option<String>,
    /// Evidência preservada: records MDR1 codificados, PARALELOS a
    /// `candidates` (mesmo índice). A resolução (`resolve_conflict`)
    /// re-importa o record do vencedor sem depender do nó remoto — nenhum
    /// candidato é perdido ou precisa ser re-buscado.
    pub records: Vec<Vec<u8>>,
}

const CFL_MAGIC: &[u8; 4] = b"CFL1";
const CFL_VERSION: u8 = 1;

fn rd_u16(data: &[u8], off: usize) -> Option<u16> {
    data.get(off..off.checked_add(2)?)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn rd_u64(data: &[u8], off: usize) -> Option<u64> {
    data.get(off..off.checked_add(8)?)
        .map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

fn rd_u32(data: &[u8], off: usize) -> Option<u32> {
    data.get(off..off.checked_add(4)?)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn push_str(out: &mut Vec<u8>, s: &str) -> Result<(), &'static str> {
    if s.len() > u16::MAX as usize {
        return Err("cfl str too long");
    }
    out.extend_from_slice(&(s.len() as u16).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

fn read_str(data: &[u8], off: &mut usize) -> Result<String, &'static str> {
    let len = rd_u16(data, *off).ok_or("trunc slen")? as usize;
    *off += 2;
    if *off + len > data.len() {
        return Err("trunc s");
    }
    let s = core::str::from_utf8(&data[*off..*off + len]).map_err(|_| "utf8 s")?;
    *off += len;
    Ok(String::from(s))
}

impl ConflictRecord {
    /// Encode truncating-seguro (P1-2): valida TODOS os campos de comprimento
    /// antes de qualquer cast (`u16`/`u8`/`u32`). Um campo que não cabe no
    /// wire retorna `Err` — nunca trunca silenciosamente (o decode leria um
    /// comprimento diferente e rejeitaria/desalinharia o stream).
    pub fn try_encode(&self) -> Result<Vec<u8>, &'static str> {
        if self.conflict_id.len() > u16::MAX as usize
            || self.subject.len() > u16::MAX as usize
            || self.candidates.len() > u16::MAX as usize
            || self.nodes.len() > u8::MAX as usize
            || self.records.len() > u8::MAX as usize
        {
            return Err("cfl field count overflow");
        }
        for c in &self.candidates {
            if c.len() > u16::MAX as usize {
                return Err("cfl candidate too long");
            }
        }
        if let Some(w) = &self.resolved_winner {
            if w.len() > u16::MAX as usize {
                return Err("cfl winner too long");
            }
        }
        for r in &self.records {
            if r.len() > u32::MAX as usize {
                return Err("cfl record too long");
            }
        }
        let mut out = Vec::with_capacity(24 + self.subject.len() + self.candidates.len() * 34);
        out.extend_from_slice(CFL_MAGIC);
        out.push(CFL_VERSION);
        out.push(self.status as u8);
        out.push(self.resolved_winner.is_some() as u8);
        out.extend_from_slice(&self.created_tick.to_le_bytes());
        push_str(&mut out, &self.conflict_id)?;
        push_str(&mut out, &self.subject)?;
        out.extend_from_slice(&(self.candidates.len() as u16).to_le_bytes());
        for c in &self.candidates {
            push_str(&mut out, c)?;
        }
        out.push(self.nodes.len() as u8);
        for n in &self.nodes {
            out.push(*n);
        }
        if let Some(w) = &self.resolved_winner {
            push_str(&mut out, w)?;
        }
        out.push(self.records.len() as u8);
        for r in &self.records {
            out.extend_from_slice(&(r.len() as u32).to_le_bytes());
            out.extend_from_slice(r);
        }
        Ok(out)
    }

    /// Encode para uso interno (panic em overflow — os call-sites de
    /// produção devem usar [`ConflictRecord::try_encode`] e propagar erro).
    pub fn encode(&self) -> Vec<u8> {
        self.try_encode().expect("conflict wire overflow")
    }

    /// Decode bounds-checked — nunca panics em entrada malformada/truncada.
    pub fn decode(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 15 || &data[0..4] != CFL_MAGIC {
            return Err("bad cfl magic");
        }
        if data[4] != CFL_VERSION {
            return Err("bad cfl version");
        }
        let status = ConflictStatus::from_u8(data[5]).ok_or("bad cfl status")?;
        let has_winner = data[6] != 0;
        let mut off = 7;
        let created_tick = rd_u64(data, off).ok_or("trunc cfl created")?;
        off += 8;
        let conflict_id = read_str(data, &mut off)?;
        let subject = read_str(data, &mut off)?;
        let n = rd_u16(data, off).ok_or("trunc cfl n")? as usize;
        off += 2;
        let mut candidates = Vec::with_capacity(n.min(64));
        for _ in 0..n {
            candidates.push(read_str(data, &mut off)?);
        }
        let nn = *data.get(off).ok_or("trunc cfl nn")? as usize;
        off += 1;
        let mut nodes = Vec::with_capacity(nn.min(256));
        for _ in 0..nn {
            nodes.push(*data.get(off).ok_or("trunc cfl node")?);
            off += 1;
        }
        let resolved_winner = if has_winner {
            Some(read_str(data, &mut off)?)
        } else {
            None
        };
        let nr = *data.get(off).ok_or("trunc cfl nr")? as usize;
        off += 1;
        let mut records = Vec::with_capacity(nr.min(8));
        for _ in 0..nr {
            let rl = rd_u32(data, off).ok_or("trunc cfl rl")? as usize;
            off += 4;
            let end = off.checked_add(rl).ok_or("overflow cfl rec")?;
            let rec = data.get(off..end).ok_or("trunc cfl rec")?;
            off = end;
            records.push(rec.to_vec());
        }
        Ok(ConflictRecord {
            conflict_id,
            subject,
            candidates,
            nodes,
            created_tick,
            status,
            resolved_winner,
            records,
        })
    }
}

/// ID determinístico do conflito: FNV-1a 128 sobre (subject + candidatos
/// ordenados). O MESMO par concorrente em re-merge gera o MESMO id (upsert).
pub fn generate_conflict_id(subject: &str, candidates: &mut Vec<String>) -> String {
    candidates.sort();
    candidates.dedup();
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut h0 = FNV_OFFSET;
    let mut h1 = FNV_OFFSET;
    let mut feed = |b: u8| {
        h0 ^= b as u64;
        h1 ^= b as u64;
        h0 = h0.wrapping_mul(FNV_PRIME);
        h1 = h1.wrapping_mul(FNV_PRIME).wrapping_add(h0);
    };
    for b in subject.as_bytes() {
        feed(*b);
    }
    for c in candidates.iter() {
        feed(0xFF); // separador entre campos
        for b in c.as_bytes() {
            feed(*b);
        }
    }
    let mut out = String::with_capacity(32);
    for i in 0..16 {
        let byte = if i < 8 { (h0 >> (i * 8)) as u8 } else { (h1 >> ((i - 8) * 8)) as u8 };
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xF) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::vec;
    use alloc::vec::Vec;

    fn sample() -> ConflictRecord {
        ConflictRecord {
            conflict_id: String::from("c1"),
            subject: String::from("md/L4/theme"),
            candidates: vec![String::from("vid-a"), String::from("vid-b")],
            nodes: vec![1u8, 2u8],
            created_tick: 7,
            status: ConflictStatus::Open,
            resolved_winner: None,
            records: vec![vec![1u8, 2, 3], vec![4, 5, 6, 7]],
        }
    }

    #[test]
    fn roundtrip_open() {
        let c = sample();
        let dec = ConflictRecord::decode(&c.encode()).unwrap();
        assert_eq!(dec, c);
        assert_eq!(dec.records.len(), 2);
    }

    #[test]
    fn roundtrip_resolved() {
        let mut c = sample();
        c.status = ConflictStatus::Resolved;
        c.resolved_winner = Some(String::from("vid-b"));
        let dec = ConflictRecord::decode(&c.encode()).unwrap();
        assert_eq!(dec.status, ConflictStatus::Resolved);
        assert_eq!(dec.resolved_winner.as_deref(), Some("vid-b"));
        assert_eq!(dec.records, c.records);
    }

    #[test]
    fn conflict_id_is_deterministic_and_order_independent() {
        let mut a = vec![String::from("vid-b"), String::from("vid-a")];
        let mut b = vec![String::from("vid-a"), String::from("vid-b")];
        let id1 = generate_conflict_id("md/L4/theme", &mut a);
        let id2 = generate_conflict_id("md/L4/theme", &mut b);
        assert_eq!(id1, id2);
        assert_eq!(id1.len(), 32);
        // outro sujeito → outro id
        let id3 = generate_conflict_id("md/L4/other", &mut a);
        assert_ne!(id1, id3);
        // candidatos deduplicados: [a, a] == [a] (mesmo id)
        let mut dup = vec![String::from("vid-a"), String::from("vid-a")];
        let id4 = generate_conflict_id("md/L4/theme", &mut dup);
        let mut single = vec![String::from("vid-a")];
        let id5 = generate_conflict_id("md/L4/theme", &mut single);
        assert_eq!(id4, id5);
    }

    #[test]
    fn decode_never_panics_on_garbage() {
        // fuzz LCG curto: qualquer entrada → Err, nunca panic
        let mut x = 0x1234_5678_9abc_def0u64;
        for _ in 0..500 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let n = (x % 80) as usize;
            let mut data = Vec::with_capacity(n);
            for _ in 0..n {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                data.push((x >> 32) as u8);
            }
            let _ = ConflictRecord::decode(&data); // ignora Ok/Err — só não pode panickar
        }
        // truncamento em cada prefixo do encode também é seguro
        let full = sample().encode();
        for cut in 0..full.len() {
            let _ = ConflictRecord::decode(&full[..cut]);
        }
    }

    #[test]
    fn try_encode_rejects_overflow_no_silent_truncation() {
        // P1-2: campo que não cabe no wire → Err (nunca cast silencioso)
        let base = sample();
        // subject > u16::MAX
        let mut big = base.clone();
        big.subject = "x".repeat(u16::MAX as usize + 1);
        assert!(big.try_encode().is_err());
        assert!(base.try_encode().is_ok());
        // candidates > u16::MAX
        let mut many = base.clone();
        many.candidates = (0..=u16::MAX as usize).map(|i| format!("c{i}")).collect();
        assert!(many.try_encode().is_err());
        // records > u8::MAX
        let mut many_recs = base.clone();
        many_recs.records = vec![vec![0u8; 4]; u8::MAX as usize + 1];
        assert!(many_recs.try_encode().is_err());
        // nodes > u8::MAX
        let mut many_nodes = base.clone();
        many_nodes.nodes = (0..=u8::MAX).collect();
        assert!(many_nodes.try_encode().is_err());
    }
}

// ── P1-4: property test decode∘encode (CFL1) ────────────────────────
// Harness LCG determinístico (zero deps; decisão P1-4). `records` preservam a
// ordem (decode∘encode é bijective) — tamanhos pequenos respeitam os limites
// de `try_encode`. O `conflict_id` é gerado pelo engine (`generate_conflict_id`)
// para não depender de strings arbitrariamente formatadas.
#[cfg(test)]
mod prop_tests {
    use super::*;
    use alloc::vec::Vec;

    fn rng(state: &mut u64) -> u64 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *state >> 32
    }

    fn record(state: &mut u64) -> ConflictRecord {
        let n_cand = (rng(state) % 5) as usize;
        let mut candidates: Vec<String> = (0..n_cand).map(|i| alloc::format!("vid-{i}")).collect();
        let nodes: Vec<u8> = (0..(rng(state) % 5) as usize).map(|i| i as u8 + 1).collect();
        let subject = alloc::format!("md/L4/subj{}", rng(state) % 4);
        let conflict_id = generate_conflict_id(&subject, &mut candidates);
        let resolved = rng(state).is_multiple_of(2);
        let winner = if resolved && !candidates.is_empty() {
            Some(candidates[0].clone())
        } else {
            None
        };
        let records: Vec<Vec<u8>> = (0..(rng(state) % 8) as usize)
            .map(|_| {
                let len = (rng(state) % 8) as usize;
                (0..len).map(|_| rng(state) as u8).collect()
            })
            .collect();
        ConflictRecord {
            conflict_id,
            subject,
            candidates,
            nodes,
            created_tick: rng(state),
            status: if resolved { ConflictStatus::Resolved } else { ConflictStatus::Open },
            resolved_winner: winner,
            records,
        }
    }

    #[test]
    fn prop_conflict_roundtrip_lcg() {
        let mut state = 0xC0FF_EE00_5EED_000Au64;
        for _ in 0..2000 {
            let rec = record(&mut state);
            let enc = rec.try_encode().unwrap();
            let dec = ConflictRecord::decode(&enc).unwrap();
            assert_eq!(dec, rec);
        }
    }
}
