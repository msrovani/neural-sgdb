//! TKLV/TKCK — codec byte-exato do TickvLite do OS (interop de storage, roadmap item 6).
//!
//! Port fiel do formato on-disk de `crates/k_nano/src/storage/tickv.rs` (691 LOC,
//! ADR-0063): um volume gravado pelo neural-os-core é lido por este crate e
//! vice-versa.
//!
//! ## Formato (cheat sheet)
//! - Stream de records múltiplos de 512 bytes.
//! - Header 16B: `54 4B 4C 56` (b"TKLV", 4º byte = 'V' = válido) | `klen u32le` |
//!   `vlen u32le` | `crc32le` — CRC32 (IEEE, poly 0xEDB88320, init 0xFFFFFFFF,
//!   XOR final) cobrindo **somente key‖val** (sem header, sem padding).
//! - Body: `key ‖ val` zero-padded a 16 bytes; tail: zero-padded a 512.
//! - Tombstone: `54 4B 4C 00` (in-place, mesmo offset) OU record com `vlen = 0`.
//! - Checkpoint: record TKLV com key `sys/tickv_ckpt`, val = `TKCK | append_off
//!   u64le | fnv1a64 u64le | n u32le | (klen u16le | key | off u64le)ⁿ`.
//! - EOF: janela de 16 bytes toda `0x00` ou toda `0xFF`.
//!
//! ## Interop directions
//! - **Read an OS volume:** `scan_volume` replicates the OS `recover()`
//!   semantics (512-aligned corrupt hunt, last-wins per key, `vlen=0`
//!   tombstone, in-place `TKL\0` tombstone skip).
//! - **Write an OS-readable volume:** `TickvFile` (a `Storage` backend) writes
//!   byte-exact TKLV records; the OS mounts by full scan (`recover()` fallback,
//!   no ckpt — `TickvFile` does not write a checkpoint in v0.1).
//!
//! ⚠️ No ckpt in v0.1: crate volumes mount in the OS via full scan (correct,
//! slower than fast-mount). GC/compaction (zero-fill + rewrite live set)
//! stays for v0.2 — append-only until then.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::storage::crc32;

/// Magic TKLV — 4º byte 'V' (valid); tombstone in-place troca por 0x00.
pub const MAGIC: &[u8; 4] = b"TKLV";
/// Prefixo de 3 bytes usado para detectar "formato tickv" mesmo no tombstone.
pub const MAGIC_PREFIX: &[u8; 3] = b"TKL";
/// Tamanho do header fixo (magic + klen + vlen + crc).
pub const HEADER: usize = 16;
/// Limites do leitor (paridade com `recover()` do OS).
pub const MAX_KLEN: usize = 4096;
pub const MAX_VLEN: usize = 1024 * 1024;
/// Canonical checkpoint key.
pub const CKPT_KEY: &str = "sys/tickv_ckpt";

/// Tamanho total de um record no volume (múltiplo de 512).
pub fn record_size(klen: usize, vlen: usize) -> usize {
    let body_padded = (klen + vlen + 15) & !15;
    ((HEADER + body_padded) + 511) & !511
}

/// Serializa um record TKLV completo (512-alinhado, body 16-alinhado).
/// CRC32 cobre key‖val apenas. `vlen = 0` ⇒ tombstone por append.
pub fn encode_record(key: &[u8], val: &[u8]) -> Vec<u8> {
    let body_len = key.len() + val.len();
    let body_padded = (body_len + 15) & !15;
    let mut body = vec![0u8; body_padded];
    body[..key.len()].copy_from_slice(key);
    body[key.len()..body_len].copy_from_slice(val);
    let crc = crc32(&body[..body_len]);
    let total = record_size(key.len(), val.len());
    let mut rec = vec![0u8; total];
    rec[0..4].copy_from_slice(MAGIC);
    rec[4..8].copy_from_slice(&(key.len() as u32).to_le_bytes());
    rec[8..12].copy_from_slice(&(val.len() as u32).to_le_bytes());
    rec[12..16].copy_from_slice(&crc.to_le_bytes());
    rec[HEADER..HEADER + body_padded].copy_from_slice(&body);
    rec
}

/// Serializa o body de checkpoint TKCK (val do record `sys/tickv_ckpt`).
/// `entries` em ordem lexicográfica de chave (BTreeMap); a própria chave
/// `sys/tickv_ckpt` NÃO entra no hash.
pub fn encode_ckpt(append_off: u64, entries: &[(String, u64)]) -> Vec<u8> {
    let mut body = Vec::with_capacity(24 + entries.len() * 16);
    body.extend_from_slice(b"TKCK");
    body.extend_from_slice(&append_off.to_le_bytes());
    let h = fnv1a64_entries(entries);
    body.extend_from_slice(&h.to_le_bytes());
    body.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (k, off) in entries {
        let kb = k.as_bytes();
        if kb.len() > 65535 || kb == CKPT_KEY.as_bytes() {
            continue;
        }
        body.extend_from_slice(&(kb.len() as u16).to_le_bytes());
        body.extend_from_slice(kb);
        body.extend_from_slice(&off.to_le_bytes());
    }
    body
}

/// FNV-1a 64 (offset basis `0xcbf29ce484222325`, prime `0x100000001b3`).
pub fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// FNV-1a 64 incremental sobre pares (key, off) em ordem de iteração
/// (paridade com `write_ckpt` do OS — `sys/tickv_ckpt` é pulado pelo caller).
fn fnv1a64_entries(entries: &[(String, u64)]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (k, off) in entries {
        if k.as_bytes() == CKPT_KEY.as_bytes() {
            continue;
        }
        for &b in k.as_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        for b in off.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}

/// Resultado de um scan de volume (semântica do `recover()` do OS).
#[derive(Debug, Default)]
pub struct ScanResult {
    /// Key → (value, offset). Last occurrence wins (index points to newest).
    pub map: BTreeMap<String, Vec<u8>>,
    /// Records corrompidos pulados (CRC falhou / header inválido).
    pub corrupt: u64,
    /// True se a varredura parou por cauda truncada (record fora dos limites).
    pub truncated: bool,
}

/// Verdadeiro se o header tem forma de tickv (`TKL` + 'V' | 1 | 0).
fn hdr_shaped(hdr: &[u8]) -> bool {
    hdr.len() >= 4
        && &hdr[0..3] == MAGIC_PREFIX
        && (hdr[3] == b'V' || hdr[3] == 1 || hdr[3] == 0)
}

/// Varre um volume TKLV (arquivo ou região) e reconstrói o índice — port do
/// `recover()` (tickv.rs:276-351). EOF = janela 16B toda 0x00/0xFF; corrupção
/// pula para o próximo boundary 512 e continua; cauda truncada para.
pub fn scan_volume(data: &[u8]) -> ScanResult {
    let mut out = ScanResult::default();
    let size = data.len() as u64;
    let mut off = 0u64;
    while off + HEADER as u64 <= size {
        let hdr = &data[off as usize..off as usize + HEADER];
        if !hdr_shaped(hdr) {
            if hdr.iter().all(|&b| b == 0 || b == 0xFF) {
                break; // EOF (região apagada)
            }
            out.corrupt += 1;
            off = (off + 512) & !511;
            continue;
        }
        let klen = u32::from_le_bytes(hdr[4..8].try_into().unwrap()) as usize;
        let vlen = u32::from_le_bytes(hdr[8..12].try_into().unwrap()) as usize;
        // Bounds first (paridade com recover() do OS — hdr absurdo NÃO para o
        // scan; pula para o próximo boundary 512 e continua)
        if klen > MAX_KLEN || vlen > MAX_VLEN {
            out.corrupt += 1;
            off = (off + 512) & !511;
            continue;
        }
        let total = record_size(klen, vlen) as u64;
        if off + total > size {
            out.truncated = true;
            break;
        }
        // Tombstone in-place (`TKL\0`): o OS invalida magic[3]=0 preservando
        // klen/vlen/crc/body — pula SEM indexar e SEM checar CRC (recover).
        if hdr[3] == 0 {
            off += total;
            continue;
        }
        let body = &data[off as usize + HEADER..off as usize + HEADER + klen + vlen];
        let want = u32::from_le_bytes(hdr[12..16].try_into().unwrap());
        if crc32(body) != want {
            out.corrupt += 1;
            off = (off + 512) & !511;
            continue;
        }
        if let Ok(key) = core::str::from_utf8(&body[..klen]) {
            let val = body[klen..].to_vec();
            if vlen == 0 {
                out.map.remove(key); // tombstone por append
            } else {
                out.map.insert(String::from(key), val); // last-wins
            }
        } else {
            out.corrupt += 1;
        }
        off += total;
    }
    out
}

/// Backend `Storage` com formato TKLV byte-exato (legível pelo OS).
///
/// - `open`: lê o arquivo e reconstrói o índice com `scan_volume`.
/// - `put`: append de record TKLV (512-alinhado, CRC sobre key‖val).
/// - `delete`: append de record com `vlen = 0` (tombstone que o OS reconhece).
/// - **Não escreve checkpoint** (v0.1): o OS monta por scan completo.
#[cfg(feature = "file-storage")]
pub struct TickvFile {
    path: std::path::PathBuf,
    map: BTreeMap<String, Vec<u8>>,
}

#[cfg(feature = "file-storage")]
impl TickvFile {
    pub fn open(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let map = if path.exists() {
            let data = std::fs::read(&path)?;
            scan_volume(&data).map
        } else {
            BTreeMap::new()
        };
        Ok(TickvFile { path, map })
    }

    fn append(&mut self, key: &[u8], val: &[u8]) -> Result<(), crate::storage::SgdbError> {
        use std::io::Write;
        let rec = encode_record(key, val);
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|_| crate::storage::SgdbError::Storage("open append"))?;
        f.write_all(&rec)
            .map_err(|_| crate::storage::SgdbError::Storage("write"))?;
        f.flush().map_err(|_| crate::storage::SgdbError::Storage("flush"))
    }
}

#[cfg(feature = "file-storage")]
impl crate::storage::Storage for TickvFile {
    fn name(&self) -> &'static str {
        "tickv"
    }
    fn put(&mut self, key: &[u8], val: &[u8]) -> Result<(), crate::storage::SgdbError> {
        if key.len() > MAX_KLEN || val.len() > MAX_VLEN {
            return Err(crate::storage::SgdbError::Storage("tickv limits"));
        }
        self.append(key, val)?;
        self.map
            .insert(String::from_utf8_lossy(key).into_owned(), val.to_vec());
        Ok(())
    }
    fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, crate::storage::SgdbError> {
        Ok(self
            .map
            .get(&String::from_utf8_lossy(key).into_owned())
            .cloned())
    }
    fn scan_prefix(&mut self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, crate::storage::SgdbError> {
        let p = String::from_utf8_lossy(prefix).into_owned();
        let mut out = Vec::new();
        for (k, v) in self.map.iter() {
            if k.starts_with(&p) {
                out.push((k.as_bytes().to_vec(), v.clone()));
            }
        }
        Ok(out)
    }
    fn delete(&mut self, key: &[u8]) -> Result<(), crate::storage::SgdbError> {
        self.append(key, &[])?; // tombstone vlen=0 (paridade OS)
        self.map.remove(&String::from_utf8_lossy(key).into_owned());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "file-storage")]
    use crate::storage::{InMemory, Storage};

    #[test]
    fn record_size_alignment() {
        assert_eq!(record_size(0, 0), 512);
        assert_eq!(record_size(1, 1), 512);
        // body 1000→1008 (pad 16); 16+1008 = 1024 → total 1024
        assert_eq!(record_size(500, 500), 1024);
        assert_eq!(record_size(1000, 0), 1024);
        // body 2000→2016; 16+2016 = 2032 → (2032+511)&!511 = 2048
        assert_eq!(record_size(1000, 1000), 2048);
    }

    #[test]
    fn golden_record_bytes() {
        // key="k" (1B), val="v" (1B) — layout byte-exato por spec
        let rec = encode_record(b"k", b"v");
        assert_eq!(rec.len(), 512);
        assert_eq!(&rec[0..4], b"TKLV");
        assert_eq!(&rec[4..8], &1u32.to_le_bytes()); // klen
        assert_eq!(&rec[8..12], &1u32.to_le_bytes()); // vlen
        let want_crc = crc32(b"kv");
        assert_eq!(&rec[12..16], &want_crc.to_le_bytes());
        assert_eq!(&rec[16..18], b"kv"); // body
        assert!(rec[18..512].iter().all(|&b| b == 0)); // padding zeros
    }

    #[test]
    fn scan_roundtrip_and_tombstone() {
        let mut data = Vec::new();
        let r1 = encode_record(b"md/L2/a", b"1");
        let r2 = encode_record(b"md/L2/b", b"2");
        let del = encode_record(b"md/L2/a", b""); // tombstone vlen=0
        data.extend_from_slice(&r1);
        data.extend_from_slice(&r2);
        data.extend_from_slice(&del);
        let scan = scan_volume(&data);
        assert_eq!(scan.corrupt, 0);
        assert!(!scan.truncated);
        assert_eq!(scan.map.get("md/L2/a"), None); // tombstoned
        assert_eq!(scan.map.get("md/L2/b").map(|v| v.as_slice()), Some(&b"2"[..]));
    }

    #[test]
    fn scan_corrupt_hunts_next_boundary() {
        let mut data = Vec::new();
        let r1 = encode_record(b"k1", b"v1");
        data.extend_from_slice(&r1);
        // lixo entre records (header não-tickv, não-zero)
        data.extend_from_slice(&[0xAAu8; 512]);
        let r2 = encode_record(b"k2", b"v2");
        data.extend_from_slice(&r2);
        let scan = scan_volume(&data);
        assert_eq!(scan.corrupt, 1);
        assert_eq!(scan.map.get("k2").map(|v| v.as_slice()), Some(&b"v2"[..]));
    }

    #[test]
    fn scan_torn_tail_truncated() {
        let mut data = Vec::new();
        data.extend_from_slice(&encode_record(b"k1", b"v1"));
        // header 16B completo EM BOUNDS (size=528), mas total (512) estoura →
        // truncated (cauda cortada por crash)
        data.extend_from_slice(b"TKLV");
        data.extend_from_slice(&0u32.to_le_bytes()); // klen
        data.extend_from_slice(&0u32.to_le_bytes()); // vlen
        data.extend_from_slice(&0u32.to_le_bytes()); // crc
        let scan = scan_volume(&data);
        assert!(scan.truncated);
        assert_eq!(scan.map.get("k1").map(|v| v.as_slice()), Some(&b"v1"[..]));
    }

    #[test]
    fn fnv1a64_known_vector() {
        // Vetor FNV-1a 64 conhecido: fnv1a64("a") = 0xaf63dc4c8601ec8c
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn tickvfile_roundtrip() {
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tickv_rt.db");
        let _ = std::fs::remove_file(&path);
        {
            let mut s = TickvFile::open(&path).unwrap();
            s.put(b"md/L2/x", b"10").unwrap();
            s.put(b"md/L2/y", b"20").unwrap();
            assert_eq!(s.get(b"md/L2/x").unwrap(), Some(b"10".to_vec()));
        }
        {
            let mut s = TickvFile::open(&path).unwrap();
            assert_eq!(s.get(b"md/L2/y").unwrap(), Some(b"20".to_vec()));
            s.delete(b"md/L2/x").unwrap();
        }
        {
            let mut s = TickvFile::open(&path).unwrap();
            assert!(s.get(b"md/L2/x").unwrap().is_none());
        }
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn tickvfile_bytes_match_os_format() {
        // O arquivo gravado pelo TickvFile deve ser um stream TKLV 512-alinhado:
        // re-parse com scan_volume (o mesmo que o OS faz no recover()) e conferir.
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tickv_osfmt.db");
        let _ = std::fs::remove_file(&path);
        {
            let mut s = TickvFile::open(&path).unwrap();
            s.put(b"a", b"1").unwrap();
            s.put(b"b", b"22").unwrap();
        }
        let raw = std::fs::read(&path).unwrap();
        assert_eq!(raw.len() % 512, 0);
        let scan = scan_volume(&raw);
        assert_eq!(scan.corrupt, 0);
        assert_eq!(scan.map.get("a").map(|v| v.as_slice()), Some(&b"1"[..]));
        assert_eq!(scan.map.get("b").map(|v| v.as_slice()), Some(&b"22"[..]));
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn sgdb_end_to_end_on_tickv() {
        // Sgdb inteiro sobre backend TKLV (paridade com InMemory/FileStorage)
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tickv_sgdb.db");
        let _ = std::fs::remove_file(&path);
        {
            let mut db = crate::sgdb::Sgdb::open(TickvFile::open(&path).unwrap()).unwrap();
            db.remember_exchange("oi", "ola").unwrap();
            db.remember_fact("fato 1", 42).unwrap();
            db.checkpoint().unwrap();
            assert!(db.scan_prefix("md/L1/").unwrap().len() >= 1);
            assert!(db.scan_prefix("md/L3/").unwrap().len() >= 1);
        }
        {
            let mut db = crate::sgdb::Sgdb::open(TickvFile::open(&path).unwrap()).unwrap();
            assert!(db.scan_prefix("md/L3/").unwrap().len() >= 1);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn parity_inmemory_vs_tickv() {
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tickv_parity.db");
        let _ = std::fs::remove_file(&path);
        let mut a = InMemory::new();
        let mut b = TickvFile::open(&path).unwrap();
        for i in 0..50 {
            let k = format!("md/L2/{:04}", i);
            let v = format!("v{i}");
            a.put(k.as_bytes(), v.as_bytes()).unwrap();
            b.put(k.as_bytes(), v.as_bytes()).unwrap();
        }
        assert_eq!(a.scan_prefix(b"md/L2/").unwrap().len(),
                   b.scan_prefix(b"md/L2/").unwrap().len());
        let _ = std::fs::remove_file(&path);
    }
}
