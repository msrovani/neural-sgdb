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
/// Forget cognitivo auditado (v1.2.2): elo anexado por `Sgdb::audit_forget`
/// quando o OS executa um esquecimento HITL — carrega a storage key canônica
/// apagada no campo `ts_note`-like do snapshot (1 item, sem meta).
pub const AUDIT_OP_FORGET: u8 = 2;
/// Resolução EXPLÍCITA de conflito auditada (triagem s413, ISSUE 7):
/// anexada por `Sgdb::audit_resolve` quando a camada superior (HITL)
/// decide o vencedor de um conflito CRDT. `digest` = FNV-1a do reason;
/// snapshot = 1 item marcando o alvo (`sk` = `sys/conflict/<id>`-
/// compatível: a key lógica do conflito, meta = winner_vid).
pub const AUDIT_OP_RESOLVE: u8 = 3;

// -- Seam de hash da chain (v1.4.4, triagem s413 ISSUE 6a) ----------------------
//
// ADR-0006: cripto e SEAM. O efeito pratico do FNV-1a sem chave e que a chain
// detecta corrupcao ACIDENTAL, nao adversario - quem escreve no storage
// recalcula o `prev_hash`. `Sha256Trunc` eleva a tamper-evidence sem mudar o
// wire (o `digest` continua `u64`). Honesto: 64 bits NAO e assinatura -
// autenticidade (Ed25519) continua sendo do host.
//
// Selecao por enum + `AtomicU8`: sem `dyn` global, sem `unsafe`, `no_std`.

/// SHA-256 (`no_std`, zero-dep). Existe para o seam: a chain deixa de
/// depender so do FNV-1a sem chave, sem wire change e sem `unsafe`. Nao e
/// assinatura - e resistencia a pre-imagem (~2^32 por forca bruta no trunc).
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    // padding: 0x80 + zeros + 64-bit BE do bit-length
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = Vec::with_capacity(data.len() + 72);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    let mut w = [0u32; 64];
    for chunk in msg.as_chunks::<64>().0 {
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d) = (h[0], h[1], h[2], h[3]);
        let (mut e, mut f, mut g, mut hh) = (h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

/// Hashers da chain (v1.4.4). O wire AUD1 nao muda: o seam troca a FUNCAO de
/// hash, nunca o formato.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hasher {
    /// FNV-1a 64 - default historico da chain.
    Fnv1a64,
    /// SHA-256 truncado aos 64 bits primeiros.
    Sha256Trunc,
}

impl Hasher {
    /// Rotulo estavel (telemetria honesta no health/contract).
    pub fn label(&self) -> &'static str {
        match self {
            Hasher::Fnv1a64 => "fnv1a64",
            Hasher::Sha256Trunc => "sha256-trunc",
        }
    }
}

/// Offset basis do FNV-1a 64 (o "hash vazio" da funcao).
pub const FNV64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a 64 COM SEED. O seed explicito cobre os DOIS formatos que a chain
/// usa: hash de bloco (`digest`, `prev_hash`) e digest RODANTE do estado.
/// Mesma polinomica de `crate::tickv::fnv1a64` - mudar aqui invalida toda
/// chain ja gravada (o teste de paridade impede).
#[inline]
fn fnv1a64_seeded(mut h: u64, data: &[u8]) -> u64 {
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

static AUDIT_HASHER: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// Hasher ativo da chain.
#[inline]
pub fn audit_hasher() -> Hasher {
    match AUDIT_HASHER.load(core::sync::atomic::Ordering::Relaxed) {
        1 => Hasher::Sha256Trunc,
        _ => Hasher::Fnv1a64,
    }
}

/// Troca o hasher da chain. Afeta elos NOVOS: a chain existente foi gravada
/// com o hasher antigo e re-auditar-la exigiria reescrever a storage - o host
/// escolhe o seam ANTES do primeiro checkpoint.
pub fn set_audit_hasher(h: Hasher) {
    use core::sync::atomic::Ordering;
    AUDIT_HASHER.store(
        match h {
            Hasher::Fnv1a64 => 0,
            Hasher::Sha256Trunc => 1,
        },
        Ordering::Relaxed,
    );
}

/// Hash de um bloco da chain (`digest`, `prev_hash`) com o hasher ativo.
#[inline]
pub fn audit_hash(data: &[u8]) -> u64 {
    hash_with(audit_hasher(), FNV64_OFFSET, data)
}

/// Hash de um bloco COM SEED (digest rodante do estado).
#[inline]
pub fn audit_hash_seeded(seed: u64, data: &[u8]) -> u64 {
    hash_with(audit_hasher(), seed, data)
}

/// Hash com hasher EXPLICITO - a funcao pura que os testes usam (nao toca no
/// global, entao nao corre em paralelo com os testes da chain).
#[inline]
pub fn hash_with(h: Hasher, seed: u64, data: &[u8]) -> u64 {
    match h {
        Hasher::Fnv1a64 => fnv1a64_seeded(seed, data),
        Hasher::Sha256Trunc => {
            // O seed entra como prefixo de 8 bytes: sem ele o modo seeded e o
            // modo de bloco dariam o MESMO hash.
            let mut buf = Vec::with_capacity(8 + data.len());
            buf.extend_from_slice(&seed.to_le_bytes());
            buf.extend_from_slice(data);
            let full = sha256(&buf);
            u64::from_le_bytes([full[0], full[1], full[2], full[3], full[4], full[5], full[6], full[7]])
        }
    }
}


/// Uma entrada do ledger (um elo da hash-chain).
#[derive(Clone, Debug, PartialEq)]
pub struct AuditEntry {
    /// Sequencial monotônico (chave `sys/audit/<seq:016x>`).
    pub seq: u64,
    /// FNV-1a do ENTRY anterior ENCODED (0 = primeiro elo).
    pub prev_hash: u64,
    /// Clock do caller no checkpoint/rollback.
    pub ts: u64,
    /// `AUDIT_OP_CHECKPOINT` | `AUDIT_OP_ROLLBACK` | `AUDIT_OP_FORGET` |
    /// `AUDIT_OP_RESOLVE`.
    pub op: u8,
    /// Semântica POR OP (triagem s413, ISSUE 6):
    /// - CHECKPOINT: FNV-1a do estado corrente (docs + side-tables);
    /// - ROLLBACK: digest do estado DEPOIS do restore;
    /// - FORGET: FNV-1a do `reason` (a evidência é o snapshot-item);
    /// - RESOLVE: FNV-1a do `reason`; o vencedor viaja no meta do item.
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
        if !(op == AUDIT_OP_CHECKPOINT
            || op == AUDIT_OP_ROLLBACK
            || op == AUDIT_OP_FORGET
            || op == AUDIT_OP_RESOLVE)
        {
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

#[cfg(test)]
mod v144_hasher_tests {
    use super::*;

    /// (a) SHA-256 bate com os vetores do NIST. A constante da prima do FNV
    /// tambem e coberta aqui: escrevi `0x1000_0000_01b3` (um zero a mais) e o
    /// hash "funcionava" sem nenhum erro visivel - so a PARIDADE com
    /// `tickv::fnv1a64` denunciou.
    #[test]
    fn sha256_matches_nist_vectors() {
        assert_eq!(
            sha256(b"abc"),
            [
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d,
                0xae, 0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10,
                0xff, 0x61, 0xf2, 0x00, 0x15, 0xad
            ]
        );
        assert_eq!(
            sha256(b""),
            [
                0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99,
                0x6f, 0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95,
                0x99, 0x1b, 0x78, 0x52, 0xb8, 0x55
            ]
        );
        // multiplos blocos + o padding de 56 mod 64
        let big = alloc::vec![b'a'; 1 << 20];
        assert_ne!(sha256(&big), sha256(b"a"));
    }

    /// (b) O default tem que ser BYTE-IDENTICO ao FNV-1a historico - o teste que
    /// impediu a prima errada de virar mudanca silenciosa de formato.
    #[test]
    fn fnv1a64_default_is_byte_identical_to_history() {
        let data = b"elo de auditoria";
        assert_eq!(
            hash_with(Hasher::Fnv1a64, FNV64_OFFSET, data),
            crate::tickv::fnv1a64(data),
            "default tem que bater com fnv1a64 historico"
        );
        // seeded: o estado rodante usa a mesma funcao com semente
        let seed = FNV64_OFFSET;
        let mut h = seed;
        for &b in data {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        assert_eq!(hash_with(Hasher::Fnv1a64, seed, data), h);
    }

    /// (c) Os dois hashers produzem digests DIFERENTES para o mesmo motivo, e
    /// o modo seeded nao degenera no modo de bloco.
    #[test]
    fn hashers_differ_and_seeded_is_not_the_block_hash() {
        let data = b"motivo";
        let a = hash_with(Hasher::Fnv1a64, FNV64_OFFSET, data);
        let b = hash_with(Hasher::Sha256Trunc, FNV64_OFFSET, data);
        assert_ne!(a, b, "sha256-trunc tem que diferir do fnv1a64");
        assert_ne!(
            hash_with(Hasher::Sha256Trunc, 7, data),
            hash_with(Hasher::Sha256Trunc, FNV64_OFFSET, data),
            "o seed tem que entrar no digest (ou o rolling nao herda nada)"
        );
        assert_eq!(Hasher::Fnv1a64.label(), "fnv1a64");
        assert_eq!(Hasher::Sha256Trunc.label(), "sha256-trunc");
    }
}

#[cfg(test)]
mod v144_golden_aud_tests {
    use super::*;

    /// v1.4.4 (Lote E, ISSUE 18): golden byte-exato da **AUD1**. A chain é o
    /// formato onde um off errado é mais caro: um `prev_hash` lido do lugar
    /// errado não dá panic — dá "chain intacta" sobre chain quebrada. Sem
    /// golden byte-a-byte, isso só apareceria num incidente real.
    #[test]
    fn golden_aud1_layout_pins_magic_version_and_field_order() {
        let e = AuditEntry {
            seq: 7,
            prev_hash: 0x1111_2222_3333_4444,
            ts: 99,
            op: AUDIT_OP_RESOLVE,
            digest: 0x5555_6666_7777_8888,
            snapshot: alloc::vec![AuditSnapshotItem {
                sk: String::from("md/L3/x"),
                state: crate::memory_doc::MemoryState::Active,
                validity: None,
                meta: alloc::vec![0xAA, 0xBB],
            }],
        };
        let enc = e.encode();
        assert_eq!(&enc[0..4], b"AUD1", "magic AUD1 byte-exato");
        assert_eq!(enc[4], AUDIT_VERSION, "byte de versao logo apos o magic");
        // campos de topo em ordem: seq | prev_hash | ts | op | digest
        // (o golden pegou um off-by-one REAL na 1a redacao deste teste: sem o
        // byte de versao no offset 4, todo o resto desliza 1 byte.)
        assert_eq!(&enc[5..13], &7u64.to_le_bytes(), "seq");
        assert_eq!(&enc[13..21], &0x1111_2222_3333_4444u64.to_le_bytes(), "prev_hash");
        assert_eq!(&enc[21..29], &99u64.to_le_bytes(), "ts");
        assert_eq!(enc[29], AUDIT_OP_RESOLVE, "op");
        assert_eq!(&enc[30..38], &0x5555_6666_7777_8888u64.to_le_bytes(), "digest");
        // roundtrip preserva o elo (tamper-evidence depende disso)
        let back = AuditEntry::decode(&enc).expect("decode do elo");
        assert_eq!(back.seq, e.seq);
        assert_eq!(back.prev_hash, e.prev_hash);
        assert_eq!(back.digest, e.digest);
        assert_eq!(back.op, e.op);
        assert_eq!(back.snapshot.len(), 1);
        assert_eq!(back.snapshot[0].sk, "md/L3/x");
        assert_eq!(back.snapshot[0].meta, alloc::vec![0xAA, 0xBB]);
    }
}
