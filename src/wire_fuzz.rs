//! P2-4 — fuzz dirigido central de TODOS os wire types.
//!
//! Harness LCG determinístico (zero deps) que exercita num único lugar todos
//! os codecs de rede/armazenamento do crate, fechando a promessa "fuzz-tested"
//! de `docs/api.md`:
//!
//! - **never-panic**: qualquer byte aleatório (vários tamanhos) → `decode` é
//!   chamado e o resultado é descartado; o teste só exige que NÃO panique.
//! - **roundtrip**: `decode(encode(x)) == x` para samples LCG válidos.
//! - **truncamento seguro**: todo prefixo de um encode válido decodifica sem
//!   panickar (e prefixos que são prefixos triviais → `Err`, nunca panic).
//! - **magic/versão corrompidos**: mutação do magic/version → `Err`, nunca
//!   panic.
//!
//! Wire types cobertos:
//! 1. NMD1 — `MemoryDoc` (doc)
//! 2. MDR1 — `MemoryRecord` (doc + estado + validade + meta)
//! 3. MDM1 — `MemoryMeta`
//! 4. CFL1 — `ConflictRecord`
//! 5. MDLT — `MemoryDelta` (p2p)
//! 6. MSNP — `MemorySnapshot` (p2p)
//! 7. `SignedEnvelope` (p2p)
//! 8. `CrdtState` (p2p)
//!
//! no_std-safe: só `alloc`. p2p-gated apenas nos wire types p2p.

#![cfg(test)]

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::conflict::{ConflictRecord, ConflictStatus};
use crate::memory_doc::{MemoryDoc, MemoryLayer, MemoryMeta, MemoryRecord, MemoryState};

#[cfg(feature = "p2p")]
use crate::crdt::{CrdtState, MemoryDelta, MemorySnapshot, MemoryVersion, SignedEnvelope};

/// LCG determinístico (mesmo gerador dos demais harnesses P1-4).
fn lcg(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    *state >> 32
}

fn bytes(state: &mut u64, n: usize) -> Vec<u8> {
    (0..n).map(|_| lcg(state) as u8).collect()
}

// ── never-panic em bytes aleatórios ─────────────────────────────────────────

#[test]
fn all_wire_decoders_never_panic_on_lcg_bytes() {
    let mut s = 0xC0FF_EE00_DEAD_BEEFu64;
    for len in 0..128usize {
        for _ in 0..8 {
            let b = bytes(&mut s, len);
            let _ = MemoryDoc::decode(&b);
            let _ = MemoryRecord::decode(&b);
            let _ = MemoryMeta::decode(&b);
            let _ = ConflictRecord::decode(&b);
            #[cfg(feature = "p2p")]
            {
                let _ = MemoryDelta::decode(&b);
                let _ = MemorySnapshot::decode(&b);
                let _ = SignedEnvelope::decode(&b);
                let _ = CrdtState::decode(&b);
            }
        }
    }
}

// ── roundtrip decode∘encode em samples LCG válidos ──────────────────────────

fn sample_doc(state: &mut u64) -> MemoryDoc {
    let layer = MemoryLayer::from_u8((lcg(state) % 8) as u8).unwrap();
    let plen = (lcg(state) % 64) as usize;
    let payload: Vec<u8> = bytes(state, plen);
    let mut d = MemoryDoc::new(layer, "k", payload);
    // node_id < 8 (slots fixos do VectorClock — overflow não viaja no NMD1)
    d.clock.tick((lcg(state) % 8) as u8);
    d.bitvec = Some(vec![lcg(state), lcg(state), 1]);
    d
}

fn sample_record(state: &mut u64) -> MemoryRecord {
    let doc = sample_doc(state);
    let st = MemoryState::from_u8((lcg(state) % 5) as u8).unwrap();
    let validity = if lcg(state).is_multiple_of(2) {
        Some((lcg(state), lcg(state)))
    } else {
        None
    };
    MemoryRecord::new(doc, st, validity)
}

fn sample_meta(state: &mut u64) -> MemoryMeta {
    let id: String = {
        let n = 1 + (lcg(state) % 16) as usize;
        (0..n).map(|_| format!("{:x}", lcg(state) % 16)).collect()
    };
    let nent = (lcg(state) % 4) as usize;
    let mut entities = Vec::with_capacity(nent);
    for _ in 0..nent {
        let elen = 1 + (lcg(state) % 8) as usize;
        entities.push((0..elen).map(|_| (b'a' + (lcg(state) % 26) as u8) as char).collect());
    }
    MemoryMeta {
        memory_id: id.clone(),
        version_id: id,
        source: lcg(state) as u8,
        confidence: (lcg(state) % 10_001) as f32 / 10_000.0,
        importance: (lcg(state) % 10_001) as f32 / 10_000.0,
        created_tick: lcg(state),
        parent_ids: Vec::new(),
        clock_overflow: Vec::new(),
        last_reinforced: 0,
        scope: String::new(),
        entities,
    }
}

fn sample_conflict(state: &mut u64) -> ConflictRecord {
    let ncand = (lcg(state) % 4) as usize;
    let mut candidates: Vec<String> = Vec::with_capacity(ncand);
    for i in 0..ncand {
        candidates.push(alloc::format!("vid-{i}-{:x}", lcg(state)));
    }
    let mut records: Vec<Vec<u8>> = Vec::with_capacity(ncand);
    for _ in 0..ncand {
        let n = (lcg(state) % 12) as usize;
        records.push(bytes(state, n));
    }
    let resolved = if lcg(state).is_multiple_of(2) {
        Some(candidates.first().cloned().unwrap_or_default())
    } else {
        None
    };
    ConflictRecord {
        conflict_id: alloc::format!("c-{:x}", lcg(state)),
        subject: alloc::format!("md/L4/theme-{:x}", lcg(state)),
        candidates,
        nodes: vec![1u8, 2u8],
        created_tick: lcg(state),
        status: if resolved.is_some() {
            ConflictStatus::Resolved
        } else {
            ConflictStatus::Open
        },
        resolved_winner: resolved,
        records,
    }
}

#[cfg(feature = "p2p")]
fn sample_delta(state: &mut u64) -> MemoryDelta {
    let mut base: Vec<MemoryVersion> = Vec::new();
    let nb = (lcg(state) % 4) as usize;
    for _ in 0..nb {
        base.push(MemoryVersion {
            node_id: (lcg(state) % 8) as u8,
            version: lcg(state),
        });
    }
    let mut records: Vec<MemoryRecord> = Vec::new();
    let nr = (lcg(state) % 4) as usize;
    for _ in 0..nr {
        records.push(sample_record(state));
    }
    MemoryDelta { base, records }
}

#[cfg(feature = "p2p")]
fn sample_snapshot(state: &mut u64) -> MemorySnapshot {
    let mut versions: Vec<MemoryVersion> = Vec::new();
    let nv = (lcg(state) % 4) as usize;
    for _ in 0..nv {
        versions.push(MemoryVersion {
            node_id: (lcg(state) % 8) as u8,
            version: lcg(state),
        });
    }
    let mut records: Vec<MemoryRecord> = Vec::new();
    let nr = (lcg(state) % 4) as usize;
    for _ in 0..nr {
        records.push(sample_record(state));
    }
    MemorySnapshot { versions, records }
}

#[cfg(feature = "p2p")]
fn sample_envelope(state: &mut u64) -> SignedEnvelope {
    let plen = (lcg(state) % 32) as usize;
    let payload = bytes(state, plen);
    let alen = (lcg(state) % 16) as usize;
    let auth = bytes(state, alen);
    SignedEnvelope::new((lcg(state) % 8) as u8, payload, auth)
}

#[cfg(feature = "p2p")]
fn sample_crdt_state(state: &mut u64) -> CrdtState {
    let mut node_versions: Vec<(u8, u64)> = Vec::new();
    let nv = (lcg(state) % 4) as usize;
    for _ in 0..nv {
        node_versions.push(((lcg(state) % 8) as u8, lcg(state)));
    }
    CrdtState {
        node_id: (lcg(state) % 8) as u8,
        local_version: lcg(state),
        own_writes: lcg(state),
        node_versions,
    }
}

#[test]
fn all_wire_types_roundtrip_lcg() {
    let mut s = 0xC0FF_EE00_5EED_0024u64;
    for _ in 0..500 {
        let d = sample_doc(&mut s);
        assert_eq!(MemoryDoc::decode(&d.encode()).unwrap(), d);
        let r = sample_record(&mut s);
        assert_eq!(MemoryRecord::decode(&r.encode()).unwrap(), r);
        let m = sample_meta(&mut s);
        assert_eq!(MemoryMeta::decode(&m.encode()).unwrap(), m);
        let c = sample_conflict(&mut s);
        assert_eq!(ConflictRecord::decode(&c.encode()).unwrap(), c);
    }
}

#[cfg(feature = "p2p")]
#[test]
fn p2p_wire_types_roundtrip_lcg() {
    let mut s = 0xC0FF_EE00_5EED_0025u64;
    for _ in 0..500 {
        let d = sample_delta(&mut s);
        assert_eq!(MemoryDelta::decode(&d.encode()).unwrap(), d);
        let sn = sample_snapshot(&mut s);
        assert_eq!(MemorySnapshot::decode(&sn.encode()).unwrap(), sn);
        let e = sample_envelope(&mut s);
        let elen = e.encode().len();
        assert_eq!(SignedEnvelope::decode(&e.encode()).unwrap(), (e, elen));
        let st = sample_crdt_state(&mut s);
        assert_eq!(CrdtState::decode(&st.encode()).unwrap(), st);
    }
}

// ── truncamento seguro: todo prefixo de um encode válido decodifica ─────────

#[test]
fn all_wire_decoders_safe_on_truncated_prefixes() {
    let mut s = 0xC0FF_EE00_5EED_0026u64;
    let encs: Vec<Vec<u8>> = vec![
        sample_doc(&mut s).encode(),
        sample_record(&mut s).encode(),
        sample_meta(&mut s).encode(),
        sample_conflict(&mut s).encode(),
    ];
    #[cfg(feature = "p2p")]
    let mut encs_p2p: Vec<Vec<u8>> = vec![
        sample_delta(&mut s).encode(),
        sample_snapshot(&mut s).encode(),
        sample_envelope(&mut s).encode(),
        sample_crdt_state(&mut s).encode(),
    ];

    for enc in &encs {
        for cut in 0..enc.len() {
            let _ = MemoryDoc::decode(&enc[..cut]);
            let _ = MemoryRecord::decode(&enc[..cut]);
            let _ = MemoryMeta::decode(&enc[..cut]);
            let _ = ConflictRecord::decode(&enc[..cut]);
        }
    }
    #[cfg(feature = "p2p")]
    {
        for enc in &mut encs_p2p {
            for cut in 0..enc.len() {
                let _ = MemoryDelta::decode(&enc[..cut]);
                let _ = MemorySnapshot::decode(&enc[..cut]);
                let _ = SignedEnvelope::decode(&enc[..cut]);
                let _ = CrdtState::decode(&enc[..cut]);
            }
        }
    }
}

// ── magic/versão corrompidos → Err, nunca panic ─────────────────────────────

#[test]
fn all_wire_decoders_reject_corrupt_magic_and_version() {
    let mut s = 0xC0FF_EE00_5EED_0027u64;
    let enc = sample_doc(&mut s).encode();
    let rec = sample_record(&mut s).encode();
    let meta = sample_meta(&mut s).encode();
    let cfl = sample_conflict(&mut s).encode();

    // primeiro byte corrompido
    let mut bad = enc.clone();
    bad[0] ^= 0xFF;
    assert!(MemoryDoc::decode(&bad).is_err());
    let mut bad = rec.clone();
    bad[0] ^= 0xFF;
    assert!(MemoryRecord::decode(&bad).is_err());
    let mut bad = meta.clone();
    bad[0] ^= 0xFF;
    assert!(MemoryMeta::decode(&bad).is_err());
    let mut bad = cfl.clone();
    bad[0] ^= 0xFF;
    assert!(ConflictRecord::decode(&bad).is_err());

    // versão desconhecida (byte 4)
    let mut bad = enc;
    bad[4] = 0xFE;
    assert!(MemoryDoc::decode(&bad).is_err());
    let mut bad = rec;
    bad[4] = 0xFE;
    assert!(MemoryRecord::decode(&bad).is_err());
    let mut bad = meta;
    bad[4] = 0xFE;
    assert!(MemoryMeta::decode(&bad).is_err());
    let mut bad = cfl;
    bad[4] = 0xFE;
    assert!(ConflictRecord::decode(&bad).is_err());

    #[cfg(feature = "p2p")]
    {
        let d = sample_delta(&mut s).encode();
        let mut bad = d.clone();
        bad[0] ^= 0xFF;
        assert!(MemoryDelta::decode(&bad).is_err());
        let mut bad = d;
        bad[4] = 0xFE;
        assert!(MemoryDelta::decode(&bad).is_err());

        let sn = sample_snapshot(&mut s).encode();
        let mut bad = sn.clone();
        bad[0] ^= 0xFF;
        assert!(MemorySnapshot::decode(&bad).is_err());
        let mut bad = sn;
        bad[4] = 0xFE;
        assert!(MemorySnapshot::decode(&bad).is_err());

        let e = sample_envelope(&mut s).encode();
        // SignedEnvelope não tem magic (byte 0 = node_id válido): corromper é
        // truncar os campos de tamanho (payload len além do buffer → None)
        let mut bad = e.clone();
        bad[1] = 0xFF; // plen u32 estourado
        bad[2] = 0xFF;
        bad[3] = 0xFF;
        bad[4] = 0xFF;
        assert!(SignedEnvelope::decode(&bad).is_none());
        // truncado antes do corpo
        assert!(SignedEnvelope::decode(&e[..5]).is_none());

        let st = sample_crdt_state(&mut s).encode();
        let mut bad = st.clone();
        bad[0] ^= 0xFF;
        assert!(CrdtState::decode(&bad).is_err());
        let mut bad = st;
        bad[4] = 0xFE;
        assert!(CrdtState::decode(&bad).is_err());
    }
}