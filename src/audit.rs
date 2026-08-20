//! Ledger de auditoria + checkpoints para rollback (v1.1.10 item 5).
//!
//! Modelo ChronoMem/MemTxn adaptado ao contrato ADD-only do núcleo:
//! - **Hash-chain** (`sys/audit/<seq:016x>` → `AuditEntry`): cada checkpoint
//!   guarda `prev_hash` (FNV-1a do ENTRY anterior) e `digest` (FNV-1a sobre o
//!   estado corrente ordenado de docs + side-tables). `audit_verify` caminha a
//!   cadeia e detecta (a) quebra de elo e (b) drift do estado vs último
//!   checkpoint — tamper-evidence sem cripto (ADR-0006: crypto é seam).
//! - **Checkpoint** guarda um SNAPSHOT das side-tables cognitivas
//!   (`sys/meta/` + `sys/state/` + `sys/validity/`) — a base do
//!   `Sgdb::rollback_to(seq)`: desfaz uma sequência ruim de
//!   `feedback`/`decay`/`forget`/`expire_old` restaurando o metadado.
//! - **Não faz parte da digest**: payloads de docs NÃO são revertidos —
//!   o undo de conteúdo é o DAG causal (`version_id`/`lineage`/`supersede`),
//!   e reverter payloads quebraria a causalidade. Documentado no `rollback_to`.
//!
//! `no_std`-safe (só `alloc`), zero deps, decode bounds-checked (nunca panics).

use alloc::format; // no_std test build: `format!` não está no prelude
use alloc::string::String;
use alloc::vec::Vec;
use crate::memory_doc::MemoryState;

pub const AUDIT_MAGIC: &[u8; 4] = b"AUD1";
pub const AUDIT_VERSION: u8 = 1;
/// Checkpoint com snapshot (base de `rollback_to`).
pub const AUDIT_OP_CHECKPOINT: u8 = 0;
/// Marcador de rollback aplicado (fecha o elo sem snapshot próprio).
pub const AUDIT_OP_ROLLBACK: u8 = 1;

/// Uma entrada do ledger (um elo da hash-chain).
#[derive(Clone, Debug, PartialEq)]
pub struct AuditEntry {
    /// Sequencial monotônico (chave `sys/audit/<seq:016x>`).
    pub seq: u64,
    /// FNV-1a do ENTRY anterior ENCODED (0 = primeiro elo).
    pub prev_hash: u64,
    /// Clock do caller no checkpoint/rollback.
    pub ts: u64,
    /// `AUDIT_OP_CHECKPOINT` | `AUDIT_OP_ROLLBACK`.
    pub op: u8,
    /// FNV-1a do estado corrente (docs + side-tables ordenados). Para um
    /// marcador de rollback, o digest do estado DEPOIS do restore.
    pub digest: u64,
    /// Snapshot das side-tables cognitivas (vazio p/ marcadores).
    pub snapshot: Vec<AuditSnapshotItem>,
}

/// Item do snapshot: estado cognitivo de uma memória num instante.
#[derive(Clone, Debug, PartialEq)]
pub struct AuditSnapshotItem {
    /// Storage key canônica (`md/Lx/...`).
    pub sk: String,
    /// Estado lógico (`sys/state/`).
    pub state: MemoryState,
    /// Janela bi-temporal (`sys/validity/`; `None` = sem janela).
    pub validity: Option<(u64, u64)>,
    /// Meta encodada (MDM1) — `sys/meta/`. Vazia = registro pré-v0.6.
    pub meta: Vec<u8>,
}

impl AuditEntry {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + self.snapshot.len() * 24);
        out.extend_from_slice(AUDIT_MAGIC);
        out.push(AUDIT_VERSION);
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&self.prev_hash.to_le_bytes());
        out.extend_from_slice(&self.ts.to_le_bytes());
        out.push(self.op);
        out.extend_from_slice(&self.digest.to_le_bytes());
        out.extend_from_slice(&(self.snapshot.len() as u32).to_le_bytes());
        for it in &self.snapshot {
            out.extend_from_slice(&(it.sk.len() as u16).to_le_bytes());
            out.extend_from_slice(it.sk.as_bytes());
            out.push(it.state as u8);
            match it.validity {
                Some((from, until)) => {
                    out.push(1);
                    out.extend_from_slice(&from.to_le_bytes());
                    out.extend_from_slice(&until.to_le_bytes());
                }
                None => out.push(0),
            }
            out.extend_from_slice(&(it.meta.len() as u32).to_le_bytes());
            out.extend_from_slice(&it.meta);
        }
        out
    }

    /// Decode bounds-checked (nunca panics em entrada malformada/truncada).
    pub fn decode(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 30 || &data[0..4] != AUDIT_MAGIC {
            return Err("bad audit magic");
        }
        if data[4] != AUDIT_VERSION {
            return Err("bad audit version");
        }
        let mut off = 5;
        let seq = rd_u64(data, off).ok_or("trunc seq")?;
        off += 8;
        let prev_hash = rd_u64(data, off).ok_or("trunc prev")?;
        off += 8;
        let ts = rd_u64(data, off).ok_or("trunc ts")?;
        off += 8;
        let op = *data.get(off).ok_or("trunc op")?;
        off += 1;
        if !(op == AUDIT_OP_CHECKPOINT || op == AUDIT_OP_ROLLBACK) {
            return Err("bad audit op");
        }
        let digest = rd_u64(data, off).ok_or("trunc digest")?;
        off += 8;
        let n = rd_u32(data, off).ok_or("trunc nsnap")? as usize;
        off += 4;
        let mut snapshot = Vec::with_capacity(n.min(4096));
        for _ in 0..n {
            let sklen = rd_u16(data, off).ok_or("trunc sklen")? as usize;
            off += 2;
            let sk_bytes = data
                .get(off..off.checked_add(sklen).ok_or("sklen overflow")?)
                .ok_or("trunc sk")?;
            off += sklen;
            let sk: String = core::str::from_utf8(sk_bytes).map_err(|_| "utf8 sk")?.into();
            let state = MemoryState::from_u8(*data.get(off).ok_or("trunc state")?).ok_or("bad state")?;
            off += 1;
            let vflag = *data.get(off).ok_or("trunc vflag")?;
            off += 1;
            let validity = match vflag {
                0 => None,
                1 => {
                    let from = rd_u64(data, off).ok_or("trunc vfrom")?;
                    off += 8;
                    let until = rd_u64(data, off).ok_or("trunc vuntil")?;
                    off += 8;
                    Some((from, until))
                }
                _ => return Err("bad vflag"),
            };
            let metalen = rd_u32(data, off).ok_or("trunc metalen")? as usize;
            off += 4;
            let meta = data
                .get(off..off.checked_add(metalen).ok_or("metalen overflow")?)
                .ok_or("trunc meta")?
                .to_vec();
            off += metalen;
            snapshot.push(AuditSnapshotItem { sk, state, validity, meta });
        }
        Ok(AuditEntry { seq, prev_hash, ts, op, digest, snapshot })
    }
}

fn rd_u16(data: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(off..off + 2)?.try_into().ok()?))
}
fn rd_u32(data: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(off..off + 4)?.try_into().ok()?))
}
fn rd_u64(data: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(data.get(off..off + 8)?.try_into().ok()?))
}

/// Chave de storage do elo (`sys/audit/<seq:016x>` — largura fixa, ordenável).
pub fn audit_key(seq: u64) -> Vec<u8> {
    let mut k = Vec::with_capacity(25);
    k.extend_from_slice(b"sys/audit/");
    k.extend_from_slice(format!("{seq:016x}").as_bytes());
    k
}

/// Extrai o seq de uma chave `sys/audit/<16 hex>` (usado pelo scan).
pub fn audit_seq_from_key(key: &[u8]) -> Option<u64> {
    let hex = key.strip_prefix(b"sys/audit/")?;
    if hex.len() != 16 {
        return None;
    }
    // hex, não decimal — `parse::<u64>()` leria "0000000000000100" como 100
    u64::from_str_radix(core::str::from_utf8(hex).ok()?, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec; // no_std test build: `vec!` não está no prelude

    #[test]
    fn audit_entry_roundtrip() {
        let e = AuditEntry {
            seq: 7,
            prev_hash: 0xdead_beef,
            ts: 1234,
            op: AUDIT_OP_CHECKPOINT,
            digest: 0xabcd,
            snapshot: vec![
                AuditSnapshotItem {
                    sk: "md/L3/ts/0000000000000042".into(),
                    state: MemoryState::Active,
                    validity: Some((10, 200)),
                    meta: b"MDM1...".to_vec(),
                },
                AuditSnapshotItem {
                    sk: "md/L4/k1".into(),
                    state: MemoryState::Decayed,
                    validity: None,
                    meta: Vec::new(),
                },
            ],
        };
        let enc = e.encode();
        let d = AuditEntry::decode(&enc).unwrap();
        assert_eq!(d, e);
    }

    #[test]
    fn audit_key_sortable_and_parseable() {
        let a = audit_key(0u64);
        let b = audit_key(0xffu64);
        let c = audit_key(0x100u64);
        assert!(a < b && b < c, "chaves ordenáveis por seq");
        assert_eq!(audit_seq_from_key(&a), Some(0));
        assert_eq!(audit_seq_from_key(&c), Some(0x100));
        assert_eq!(audit_seq_from_key(b"sys/audit/short"), None);
    }

    #[test]
    fn audit_decode_hostile_input_never_panics() {
        for t in [
            &b""[..],
            &b"AUD1"[..],
            &b"AUD1\x01"[..],
            &b"XXXXXXXX"[..],
            &b"AUD1\x01\xff\xff\xff\xff"[..],
        ] {
            let _ = AuditEntry::decode(t);
        }
        // truncation a partir de um encode válido
        let e = AuditEntry {
            seq: 1,
            prev_hash: 2,
            ts: 3,
            op: AUDIT_OP_ROLLBACK,
            digest: 4,
            snapshot: vec![AuditSnapshotItem {
                sk: "md/L3/k".into(),
                state: MemoryState::Superseded,
                validity: Some((1, 2)),
                meta: b"x".to_vec(),
            }],
        };
        let enc = e.encode();
        for cut in 0..enc.len() {
            let _ = AuditEntry::decode(&enc[..cut]);
        }
    }
}