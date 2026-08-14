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
//!   byte-exact TKLV records and a TKCK checkpoint (`checkpoint()` — the TKCK
//!   record is the LAST record); `open()` fast-mounts via `try_mount_from_ckpt`
//!   with a full `scan_volume` fallback. The OS mounts by full scan
//!   (`recover()` fallback).
//!
//! ⚠️ Checkpoint é escrito por `checkpoint()` (TKCK sempre por último). Se
//! GC/compaction estiver ativo, `compact()` reescreve o live set + ckpt e faz
//! rename atômico. Volumes de teste (sem `checkpoint()`) montam por scan.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::storage::{crc32, le32};

/// Magic TKLV — 4º byte 'V' (valid); tombstone in-place troca por 0x00.
pub const MAGIC: &[u8; 4] = b"TKLV";
/// Prefixo de 3 bytes usado para detectar "formato tickv" mesmo no tombstone.
pub const MAGIC_PREFIX: &[u8; 3] = b"TKL";
/// Tamanho do header fixo (magic + klen + vlen + crc).
pub const HEADER: usize = 16;
/// Limites do leitor (paridade com `recover()` do OS). Centralizados em
/// `crate::limits` (P1-3) — re-export preserva a API pública de `tickv`.
pub use crate::limits::{MAX_KLEN, MAX_VLEN};
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
    // Filtra ANTES de contar (bughunt #11): o campo `n` deve refletir o número
    // de entradas REALMENTE gravadas. Antes, o skip (chave >65535 ou
    // sys/tickv_ckpt) acontecia no loop mas `n` já estava escrito com
    // entries.len() — um decodificador que lê n entradas desalinhava.
    let writable: Vec<(String, u64)> = entries
        .iter()
        .filter(|(k, _)| k.len() <= 65535 && k != CKPT_KEY)
        .cloned()
        .collect();
    let mut body = Vec::with_capacity(24 + writable.len() * 16);
    body.extend_from_slice(b"TKCK");
    body.extend_from_slice(&append_off.to_le_bytes());
    let h = fnv1a64_entries(&writable);
    body.extend_from_slice(&h.to_le_bytes());
    body.extend_from_slice(&(writable.len() as u32).to_le_bytes());
    for (k, off) in &writable {
        let kb = k.as_bytes();
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
    /// Offset do record vigente (last-wins) de cada chave viva — alimenta o
    /// checkpoint TKCK e o fast-mount (paridade com o índice do OS).
    pub offsets: BTreeMap<String, u64>,
    /// `append_off`: fim do último record válido processado (máx off+total) —
    /// onde os próximos appends devem começar; cauda além disso é não-válida.
    pub append_off: u64,
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
    let mut eof = false;
    while off + HEADER as u64 <= size {
        let hdr = &data[off as usize..off as usize + HEADER];
        if !hdr_shaped(hdr) {
            if hdr.iter().all(|&b| b == 0 || b == 0xFF) {
                eof = true; // EOF (região apagada)
                break;
            }
            out.corrupt += 1;
            off = (off + 512) & !511;
            continue;
        }
        let klen = le32(&hdr[4..8]).unwrap_or(0) as usize;
        let vlen = le32(&hdr[8..12]).unwrap_or(0) as usize;
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
            out.append_off = out.append_off.max(off + total);
            off += total;
            continue;
        }
        let body = &data[off as usize + HEADER..off as usize + HEADER + klen + vlen];
        let want = le32(&hdr[12..16]).unwrap_or(0);
        if crc32(body) != want {
            out.corrupt += 1;
            off = (off + 512) & !511;
            continue;
        }
        // record confiável (vivo ou tombstone por append) — estende append_off
        out.append_off = out.append_off.max(off + total);
        if let Ok(key) = core::str::from_utf8(&body[..klen]) {
            if key == CKPT_KEY {
                // checkpoint TKCK = metadado, NÃO é memória — não indexar
                // (paridade com o recover() do OS; senão o ckpt vira chave
                // "sys/tickv_ckpt" no mapa do backend).
            } else {
                let val = body[klen..].to_vec();
                if vlen == 0 {
                    out.map.remove(key); // tombstone por append
                    out.offsets.remove(key);
                } else {
                    out.map.insert(String::from(key), val); // last-wins
                    out.offsets.insert(String::from(key), off);
                }
            }
        } else {
            out.corrupt += 1;
        }
        off += total;
    }
    // Cauda "rasgada" de 1..15 bytes (header parcial < HEADER, ex: crash no
    // meio do write) ficava invisível — truncated=false (bughunt #11). Uma
    // região pré-zeroada de EOF (eof=true) NÃO é truncamento: termina limpa.
    if !eof && off < size {
        out.truncated = true;
    }
    out
}

/// Tenta montar o índice via checkpoint TKCK (fast-mount, port do
/// `try_mount_from_ckpt` do OS). Semântica: varre SÓ headers, acha o ÚLTIMO
/// record `sys/tickv_ckpt`, valida magic `TKCK` + FNV do índice + integridade
/// de cada entrada (header `TKL V` não-stale, CRC, key bate) e que o ckpt é o
/// ÚLTIMO record do volume (nada além dele, senão está stale e o mount completo
/// é necessário). Qualquer anomalia → `None` → fallback `scan_volume`.
/// Resultado de um mount a partir do checkpoint (evita `type_complexity`).
#[cfg(feature = "file-storage")]
type MountResult = (
    BTreeMap<String, Vec<u8>>,
    BTreeMap<String, u64>,
    u64,
);

#[cfg(feature = "file-storage")]
fn try_mount_from_ckpt(data: &[u8]) -> Option<MountResult> {
    let size = data.len() as u64;
    let mut off = 0u64;
    let mut ckpt: Option<(u64, u64, Vec<u8>)> = None; // (off, end, body)
    while off + HEADER as u64 <= size {
        let hdr = &data[off as usize..off as usize + HEADER];
        if !hdr_shaped(hdr) {
            if hdr.iter().all(|&b| b == 0 || b == 0xFF) {
                break; // EOF
            }
            off = (off + 512) & !511;
            continue;
        }
        let klen = le32(&hdr[4..8]).unwrap_or(0) as usize;
        let vlen = le32(&hdr[8..12]).unwrap_or(0) as usize;
        if klen > MAX_KLEN || vlen > MAX_VLEN {
            off = (off + 512) & !511;
            continue;
        }
        let total = record_size(klen, vlen) as u64;
        if off + total > size {
            break;
        }
        if hdr[3] == 0 {
            off += total;
            continue;
        }
        let body = &data[off as usize + HEADER..off as usize + HEADER + klen + vlen];
        if crc32(body) != le32(&hdr[12..16]).unwrap_or(0) {
            off = (off + 512) & !511;
            continue;
        }
        if let Ok(key) = core::str::from_utf8(&body[..klen]) {
            if key == CKPT_KEY {
                ckpt = Some((off, off + total, body[klen..].to_vec()));
            }
        }
        off += total;
    }
    let (ckpt_off, ckpt_end, body) = ckpt?;
    // o ckpt só é válido se for o ÚLTIMO record do volume (nada após além de
    // zeros/0xFF de EOF) — senão há appends posteriores não indexados (stale)
    if !data[ckpt_end as usize..].iter().all(|&b| b == 0 || b == 0xFF) {
        return None;
    }
    if body.len() < 24 || &body[0..4] != b"TKCK" {
        return None;
    }
    let stored_hash = u64::from_le_bytes(body[12..20].try_into().ok()?);
    let n = u32::from_le_bytes(body[20..24].try_into().ok()?) as usize;
    let mut off2 = 24usize;
    let mut entries: Vec<(String, u64)> = Vec::with_capacity(n);
    for _ in 0..n {
        if off2 + 2 > body.len() {
            return None;
        }
        let klen = u16::from_le_bytes(body[off2..off2 + 2].try_into().ok()?) as usize;
        off2 += 2;
        if off2 + klen + 8 > body.len() {
            return None;
        }
        let key = core::str::from_utf8(&body[off2..off2 + klen]).ok()?;
        let rec_off = u64::from_le_bytes(body[off2 + klen..off2 + klen + 8].try_into().ok()?);
        off2 += klen + 8;
        entries.push((String::from(key), rec_off));
    }
    // FNV-1a do índice (bit-rot/tamper no índice detectado — não só no valor)
    if fnv1a64_entries(&entries) != stored_hash {
        return None;
    }
    let mut map = BTreeMap::new();
    let mut offsets = BTreeMap::new();
    // append_off = fim do record do ckpt (que é o último): os próximos appends
    // vão DEPOIS do ckpt. O `append_off` gravado no corpo é o pré-ckpt (onde o
    // ckpt começa); recomputar por ckpt_end mantém o ckpt intacto.
    let mut append_off = ckpt_end;
    for (key, rec_off) in &entries {
        let r = *rec_off as usize;
        if r + HEADER > data.len() || rec_off >= &ckpt_off {
            return None; // entrada aponta para além/antes do ckpt — suspeita
        }
        let hdr = &data[r..r + HEADER];
        // stale check: header do record referenciado ainda `TKL V` (não-TKL\0)
        if &hdr[0..3] != MAGIC_PREFIX || hdr[3] != b'V' {
            return None;
        }
        let klen = le32(&hdr[4..8])? as usize;
        let vlen = le32(&hdr[8..12])? as usize;
        let total = record_size(klen, vlen) as u64;
        if klen > MAX_KLEN || vlen > MAX_VLEN || vlen == 0 || rec_off + total > size {
            return None;
        }
        let body = &data[r + HEADER..r + HEADER + klen + vlen];
        if crc32(body) != le32(&hdr[12..16])? {
            return None;
        }
        if core::str::from_utf8(&body[..klen]).ok()? != key {
            return None;
        }
        map.insert(key.clone(), body[klen..].to_vec());
        offsets.insert(key.clone(), *rec_off);
        append_off = append_off.max(*rec_off + total);
    }
    Some((map, offsets, append_off))
}

/// Backend `Storage` com formato TKLV byte-exato (legível pelo OS).
///
/// - `open`: fast-mount via checkpoint `sys/tickv_ckpt` (TKCK) com fallback
///   para `scan_volume` (varredura completa) em qualquer anomalia.
/// - `put`: append de record TKLV (512-alinhado, CRC sobre key‖val).
/// - `delete`: append de record com `vlen = 0` (tombstone que o OS reconhece).
/// - `checkpoint()`: grava o record `sys/tickv_ckpt` (TKCK byte-idêntico ao
///   OS) como ÚLTIMO record — habilita fast-mount no próximo open.
#[cfg(feature = "file-storage")]
pub struct TickvFile {
    path: std::path::PathBuf,
    map: BTreeMap<String, Vec<u8>>,
    offsets: BTreeMap<String, u64>,
    append_off: u64,
}

#[cfg(feature = "file-storage")]
impl TickvFile {
    pub fn open(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        // HOT TEST v1.1 (2026-08-13): paridade com FileStorage — criar o
        // diretório pai para que o append lazy não falhe com volume ausente.
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let (map, offsets, append_off) = if path.exists() {
            let data = std::fs::read(&path)?;
            match try_mount_from_ckpt(&data) {
                Some(m) => m,
                None => {
                    let scan = scan_volume(&data);
                    let append_off = scan.append_off;
                    // cauda além do último record válido (zeros/torn/corrupt):
                    // trunca p/ manter o volume limpo e appends em append_off
                    if (data.len() as u64) > append_off {
                        use std::io::{Seek, SeekFrom};
                        let mut f = std::fs::OpenOptions::new()
                            .write(true)
                            .open(&path)?;
                        f.seek(SeekFrom::Start(append_off))?;
                        f.set_len(append_off)?;
                    }
                    (scan.map, scan.offsets, append_off)
                }
            }
        } else {
            (BTreeMap::new(), BTreeMap::new(), 0)
        };
        Ok(TickvFile { path, map, offsets, append_off })
    }

    /// Grava checkpoint TKCK (`sys/tickv_ckpt`) como último record do volume.
    /// Após chamar, o próximo `open()` monta por fast-mount em vez de varredura
    /// completa. Um put/delete posterior torna o ckpt stale (não é mais o
    /// último record) e o open volta ao fallback seguro.
    pub fn checkpoint(&mut self) -> Result<(), crate::storage::SgdbError> {
        let entries: Vec<(String, u64)> = self
            .offsets
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        let body = encode_ckpt(self.append_off, &entries);
        self.append(CKPT_KEY.as_bytes(), &body)?;
        Ok(())
    }

    /// GC/compaction (roadmap v0.2, parity com o `maybe_gc` do OS): reescreve
    /// o **live set** (mapa atual) como records TKLV frescos + checkpoint TKCK
    /// num arquivo temporário e troca atomicamente via rename. Remove
    /// tombstones e versões obsoletas; o ckpt final habilita fast-mount no
    /// próximo open. Crash-safe: temp órfão é sobrescrito na próxima compact.
    pub fn compact(&mut self) -> Result<(), crate::storage::SgdbError> {
        use std::io::Write;
        let tmp = self.path.with_extension("compact.tmp");
        let mut f = std::fs::File::create(&tmp)
            .map_err(|_| crate::storage::SgdbError::Storage("compact create"))?;
        let mut offsets = BTreeMap::new();
        let mut append_off = 0u64;
        for (key, val) in self.map.iter() {
            let rec = encode_record(key.as_bytes(), val);
            f.write_all(&rec)
                .map_err(|_| crate::storage::SgdbError::Storage("compact write"))?;
            offsets.insert(key.clone(), append_off);
            append_off += rec.len() as u64;
        }
        // ckpt como ÚLTIMO record → fast-mount no próximo open
        let entries: Vec<(String, u64)> = offsets.iter().map(|(k, v)| (k.clone(), *v)).collect();
        let ckpt_rec = encode_record(CKPT_KEY.as_bytes(), &encode_ckpt(append_off, &entries));
        f.write_all(&ckpt_rec)
            .map_err(|_| crate::storage::SgdbError::Storage("compact write"))?;
        f.flush().map_err(|_| crate::storage::SgdbError::Storage("compact flush"))?;
        drop(f);
        // troca atômica (Windows: MoveFileEx replace)
        std::fs::rename(&tmp, &self.path)
            .map_err(|_| crate::storage::SgdbError::Storage("compact rename"))?;
        self.offsets = offsets;
        self.append_off = append_off + ckpt_rec.len() as u64;
        Ok(())
    }

    /// Invalida um record antigo IN-PLACE (`magic[3] = 0` → `TKL\0`, parity
    /// com o overwrite do OS): o dead-space fica detectável por header, o
    /// `scan_volume` pula sem CRC e a contabilidade de espaço é honesta.
    /// #6 — o novo record (ou tombstone) é append depois.
    fn invalidate_in_place(&mut self, off: u64) -> Result<(), crate::storage::SgdbError> {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.path)
            .map_err(|_| crate::storage::SgdbError::Storage("open invalidate"))?;
        f.seek(SeekFrom::Start(off + 3))
            .map_err(|_| crate::storage::SgdbError::Storage("seek invalidate"))?;
        f.write_all(&[0u8])
            .map_err(|_| crate::storage::SgdbError::Storage("write invalidate"))?;
        f.flush().map_err(|_| crate::storage::SgdbError::Storage("flush invalidate"))
    }

    /// Append de record; retorna o offset onde o record começou (para o
    /// índice de offsets / ckpt). `append_off` = offset físico corrente.
    fn append(&mut self, key: &[u8], val: &[u8]) -> Result<u64, crate::storage::SgdbError> {
        use std::io::Write;
        let rec = encode_record(key, val);
        let off = self.append_off;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|_| crate::storage::SgdbError::Storage("open append"))?;
        f.write_all(&rec)
            .map_err(|_| crate::storage::SgdbError::Storage("write"))?;
        f.flush().map_err(|_| crate::storage::SgdbError::Storage("flush"))?;
        self.append_off += rec.len() as u64;
        Ok(off)
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
        let ks = String::from_utf8_lossy(key).into_owned();
        // #6: invalida o record anterior IN-PLACE (TKL\0) antes do append —
        // o valor antigo vira dead-space detectável, nunca ressuscita
        if let Some(&old_off) = self.offsets.get(&ks) {
            self.invalidate_in_place(old_off)?;
        }
        let off = self.append(key, val)?;
        // Valor vazio == tombstone no volume (vlen=0, idêntico ao delete) — o
        // mapa deve espelhar o mesmo (bughunt #11): put(k, &[]) não pode
        // devolver Some([]) em sessão e None após reopen.
        if val.is_empty() {
            self.map.remove(&ks);
            self.offsets.remove(&ks);
        } else {
            self.offsets.insert(ks.clone(), off);
            self.map.insert(ks, val.to_vec());
        }
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
        let ks = String::from_utf8_lossy(key).into_owned();
        // #6: invalida o record vigente in-place antes do tombstone por append
        if let Some(&old_off) = self.offsets.get(&ks) {
            self.invalidate_in_place(old_off)?;
        }
        self.append(key, &[])?; // tombstone vlen=0 (paridade OS)
        self.map.remove(&ks);
        self.offsets.remove(&ks);
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
    fn scan_partial_tail_flagged_truncated() {
        // bughunt #11: cauda "rasgada" de 1..15 bytes (header < 16B) era
        // silenciosamente ignorada (truncated = false) — mesma assimetria do
        // FileStorage, que marca truncation nesse caso. Deve ser flagged.
        let mut data = encode_record(b"k1", b"v1");
        data.extend_from_slice(b"\x00"); // 1-byte torn tail
        let scan = scan_volume(&data);
        assert!(scan.truncated, "cauda parcial < 16B deveria marcar truncated");
        assert_eq!(scan.map.get("k1").map(|v| v.as_slice()), Some(&b"v1"[..]));
        // 15 bytes de lixo não-tickv → corrupt + cauda parcial (não é EOF)
        let mut data2 = encode_record(b"k2", b"v2");
        data2.extend_from_slice(&[0xAAu8; 15]); // 15B não-checável como header
        let scan2 = scan_volume(&data2);
        assert!(scan2.truncated, "cauda parcial de 15B deveria marcar truncated");
        // os 15B nunca são lidos como header (faltam 1B p/ HEADER) → corrupt 0
        assert_eq!(scan2.corrupt, 0);
    }

    #[test]
    fn scan_trailing_zero_region_is_clean_eof() {
        // Região pré-zeroada do volume (convenção de EOF do TickvLite) NÃO é
        // cauda truncada — o scan deve parar limpo, sem flag truncated.
        let mut data = encode_record(b"k1", b"v1");
        data.extend_from_slice(&[0u8; 512]);
        let scan = scan_volume(&data);
        assert!(!scan.truncated);
        assert_eq!(scan.corrupt, 0);
        assert_eq!(scan.map.get("k1").map(|v| v.as_slice()), Some(&b"v1"[..]));
    }

    #[test]
    fn encode_ckpt_count_matches_written_entries() {
        // bughunt #11: entradas puladas no corpo (sys/tickv_ckpt, chave >
        // 65535) antes inflavam o campo `n` — um decodificador OS que lê n
        // entradas desalinhava no primeiro skip. `n` = entradas realmente
        // gravadas, e o hash cobre exatamente essas.
        let entries = vec![
            (String::from("a"), 512u64),
            (String::from(CKPT_KEY), 1024u64), // deve ser pulada
            (String::from("b"), 1536u64),
        ];
        let body = encode_ckpt(0, &entries);
        assert_eq!(&body[0..4], b"TKCK");
        // header: TKCK(4) + append_off(8) + fnv(8) + n(4) = 24B → n em [20..24]
        let n = u32::from_le_bytes([body[20], body[21], body[22], body[23]]) as usize;
        assert_eq!(n, 2, "campo n deve contar apenas entradas gravadas");
        // entradas contíguas após o header de 24B: a, b
        let mut off = 24usize;
        for want in ["a", "b"] {
            let klen = u16::from_le_bytes([body[off], body[off + 1]]) as usize;
            off += 2;
            assert_eq!(&body[off..off + klen], want.as_bytes());
            off += klen + 8;
        }
        assert_eq!(off, body.len());
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

    /// Fuzz determinístico (maturation P6): scan_volume NUNCA panics com
    /// entrada corrompida/truncada — falha segura (corrupt/truncated) em vez
    /// de panic.
    #[test]
    fn scan_never_panics_on_malformed() {
        let mut state = 0xDEAD_BEEF_CAFE_F00Du64;
        let bytes = |n: usize, s: &mut u64| -> Vec<u8> {
            (0..n)
                .map(|_| {
                    *s = s.wrapping_mul(1103515245).wrapping_add(12345);
                    ((*s >> 32) & 0xFF) as u8
                })
                .collect()
        };
        for len in 0..256usize {
            for _ in 0..8 {
                let data = bytes(len, &mut state);
                let _ = scan_volume(&data); // nunca panics
            }
        }
        // volume válido truncado em todo ponto
        let mut good = Vec::new();
        for i in 0..10 {
            good.extend_from_slice(&encode_record(
                alloc::format!("md/L3/{i:04}").as_bytes(),
                b"payload",
            ));
        }
        for cut in 0..good.len() {
            let _ = scan_volume(&good[..cut]); // truncado em todo ponto
        }
    }

    #[test]
    fn fnv1a64_known_vector() {
        // Vetor FNV-1a 64 conhecido: fnv1a64("a") = 0xaf63dc4c8601ec8c
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn open_creates_missing_parent_dir() {
        // HOT TEST v1.1: paridade com FileStorage — o append lazy falhava
        // quando o diretório pai não existia; `open` deve criá-lo.
        let dir = std::env::temp_dir().join("neural_sgdb_test").join("tickv_nested_hot");
        let path = dir.join("sub").join("vol.tk");
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = TickvFile::open(&path).unwrap();
        s.put(b"k", b"v").unwrap();
        assert_eq!(s.get(b"k").unwrap(), Some(b"v".to_vec()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn tickvfile_put_invalidates_old_record_in_place() {
        // #6: overwrite invalida o record anterior IN-PLACE (magic[3] 'V'→0),
        // tornando o dead-space detectável por header (TKL\0) sem ressuscitar.
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tickv_inplace.db");
        let _ = std::fs::remove_file(&path);
        {
            let mut s = TickvFile::open(&path).unwrap();
            s.put(b"k", b"v1").unwrap();
            let old_off = *s.offsets.get("k").unwrap();
            s.put(b"k", b"v2").unwrap();
            // o record antigo (em old_off) foi invalidado in-place
            let raw = std::fs::read(&path).unwrap();
            assert_eq!(
                &raw[old_off as usize..old_off as usize + 4],
                b"TKL\x00",
                "magic[3] do record antigo deveria ser 0 (TKL\\0)"
            );
            // delete também invalida o vigente (em 512) antes do tombstone
            let v2_off = *s.offsets.get("k").unwrap();
            s.delete(b"k").unwrap();
            let raw2 = std::fs::read(&path).unwrap();
            assert_eq!(
                &raw2[v2_off as usize..v2_off as usize + 4],
                b"TKL\x00",
                "delete deveria invalidar o vigente in-place"
            );
        }
        {
            // reopen: last-wins v2 não ressuscita v1 (TKL\0 pulado pelo scan)
            let mut s = TickvFile::open(&path).unwrap();
            assert!(s.get(b"k").unwrap().is_none()); // deletado
        }
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn tickvfile_put_empty_is_delete_consistent() {
        // bughunt #11: put(k, &[]) grava tombstone vlen=0 no volume (idêntico
        // ao delete) mas antes mantinha `k -> []` no mapa — get() Some([]) em
        // sessão, None após reopen. Empty == delete, nos dois pontos.
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tickv_emptyval.db");
        let _ = std::fs::remove_file(&path);
        {
            let mut s = TickvFile::open(&path).unwrap();
            s.put(b"k", b"v").unwrap();
            s.put(b"k", b"").unwrap();
            assert!(s.get(b"k").unwrap().is_none(), "em sessão deveria sumir");
        }
        {
            let mut s = TickvFile::open(&path).unwrap();
            assert!(s.get(b"k").unwrap().is_none(), "após reopen deveria sumir");
        }
        let _ = std::fs::remove_file(&path);
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
    fn tickvfile_checkpoint_fast_mount_roundtrip() {
        // checkpoint() grava TKCK como ÚLTIMO record → open() monta por
        // fast-mount (sem varredura completa) e preserva mapa+offsets.
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tickv_ckpt.db");
        let _ = std::fs::remove_file(&path);
        {
            let mut s = TickvFile::open(&path).unwrap();
            for i in 0..100 {
                s.put(format!("k/{i:03}").as_bytes(), format!("v{i}").as_bytes()).unwrap();
            }
            s.delete(b"k/000").unwrap();
            s.checkpoint().unwrap();
            let raw = std::fs::read(&path).unwrap();
            assert_eq!(raw.len() % 512, 0);
            // o ckpt é o último record → o fast-mount DEVE funcionar (Some)
            let (m, o, a) = try_mount_from_ckpt(&raw).expect("ckpt deveria permitir fast-mount");
            assert_eq!(o.len(), 99);
            assert_eq!(m.len(), 99);
            assert_eq!(a as usize, raw.len());
        }
        {
            let mut s = TickvFile::open(&path).unwrap();
            // fast-mount: offsets preenchidos e append_off = tamanho do arquivo
            assert_eq!(s.offsets.len(), 99);
            assert!(s.get(b"k/001").unwrap().is_some());
            assert!(s.get(b"k/000").unwrap().is_none()); // tombstone
            // append pós-fast-mount não corrompe nada
            s.put(b"k/999", b"v999").unwrap();
            s.checkpoint().unwrap();
        }
        {
            let mut s = TickvFile::open(&path).unwrap();
            assert_eq!(s.get(b"k/999").unwrap(), Some(b"v999".to_vec()));
            assert!(s.get(b"k/001").unwrap().is_some());
            assert_eq!(s.map.len(), 100);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn tickvfile_ckpt_stale_falls_back_to_full_scan() {
        // records gravados DEPOIS do ckpt tornam o ckpt stale (não é o último
        // record) → open() cai no scan completo e NÃO perde os appends novos.
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tickv_stale.db");
        let _ = std::fs::remove_file(&path);
        {
            let mut s = TickvFile::open(&path).unwrap();
            s.put(b"a", b"1").unwrap();
            s.checkpoint().unwrap();
            s.put(b"b", b"2").unwrap(); // apaga o "ckpt é último" — stale
        }
        {
            let mut s = TickvFile::open(&path).unwrap();
            assert_eq!(s.get(b"a").unwrap(), Some(b"1".to_vec()));
            assert_eq!(s.get(b"b").unwrap(), Some(b"2".to_vec()));
        }
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn tickvfile_torn_ckpt_falls_back_to_full_scan() {
        // ckpt corrompido (CRC/torn) → fast-mount falha → scan completo, dados
        // anteriores intactos (mesma semântica de crash-atomicidade do OS).
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tickv_torn.db");
        let _ = std::fs::remove_file(&path);
        {
            let mut s = TickvFile::open(&path).unwrap();
            s.put(b"a", b"1").unwrap();
            s.checkpoint().unwrap();
        }
        // corrompe o corpo do ckpt (último record)
        let mut raw = std::fs::read(&path).unwrap();
        let n = raw.len();
        raw[n - 10] ^= 0xFF;
        std::fs::write(&path, &raw).unwrap();
        {
            let mut s = TickvFile::open(&path).unwrap();
            assert_eq!(s.get(b"a").unwrap(), Some(b"1".to_vec()));
        }
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn tickvfile_compact_shrinks_and_preserves() {
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tickv_compact.db");
        let _ = std::fs::remove_file(&path);
        let before_len;
        {
            let mut s = TickvFile::open(&path).unwrap();
            for i in 0..100 {
                s.put(format!("k/{i:03}").as_bytes(), format!("v{i}").as_bytes()).unwrap();
            }
            for i in (0..100).step_by(2) {
                s.delete(format!("k/{i:03}").as_bytes()).unwrap(); // 50 tombstones
            }
            before_len = std::fs::metadata(&path).unwrap().len();
            s.compact().unwrap();
            assert!(std::fs::metadata(&path).unwrap().len() < before_len, "compact deveria encolher");
            // fast-mount no mesmo arquivo compactado
            let raw = std::fs::read(&path).unwrap();
            let (m, o, _) = try_mount_from_ckpt(&raw).expect("ckpt pós-compact deveria montar");
            assert_eq!(m.len(), 50);
            assert_eq!(o.len(), 50);
        }
        {
            let mut s = TickvFile::open(&path).unwrap();
            assert_eq!(s.map.len(), 50);
            assert!(s.get(b"k/001").unwrap().is_some());
            assert!(s.get(b"k/000").unwrap().is_none()); // tombstone não ressuscitou
            s.put(b"k/200", b"v200").unwrap(); // append pós-compact
        }
        {
            let mut s = TickvFile::open(&path).unwrap();
            assert_eq!(s.get(b"k/200").unwrap(), Some(b"v200".to_vec()));
            assert_eq!(s.map.len(), 51);
            assert!(s.get(b"k/001").unwrap().is_some());
        }
        assert!(!path.with_extension("compact.tmp").exists());
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn tickvfile_compact_crash_orphan_temp() {
        // temp órfão (crash antes do rename) é sobrescrito na próxima compact
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tickv_compact_orphan.db");
        let tmp = path.with_extension("compact.tmp");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&tmp, b"orphan-garbage").unwrap();
        {
            let mut s = TickvFile::open(&path).unwrap();
            s.put(b"a", b"1").unwrap();
            s.compact().unwrap();
        }
        let mut s = TickvFile::open(&path).unwrap();
        assert_eq!(s.get(b"a").unwrap(), Some(b"1".to_vec()));
        assert!(!tmp.exists());
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn fastmount_never_panics_on_malformed() {
        // Fuzz determinístico IN-MEMORY (paridade com os decoders adversarial):
        // truncar o volume em TODO offset e corromper bytes (foco no ckpt
        // final) nunca pode panicar no try_mount_from_ckpt — qualquer anomalia
        // → None → o open() cai no fallback seguro (scan completo).
        let mut entries = Vec::new();
        let mut data = Vec::new();
        let mut off = 0u64;
        for i in 0..50 {
            let rec = encode_record(format!("k/{i:02}").as_bytes(), format!("v{i}").as_bytes());
            entries.push((format!("k/{i:02}"), off));
            off += rec.len() as u64;
            data.extend_from_slice(&rec);
        }
        data.extend_from_slice(&encode_record(CKPT_KEY.as_bytes(), &encode_ckpt(off, &entries)));
        // mount válido funciona
        let (m, o, a) = try_mount_from_ckpt(&data).expect("ckpt válido deveria montar");
        assert_eq!(m.len(), 50);
        assert_eq!(o.len(), 50);
        assert_eq!(a as usize, data.len());
        // truncar em todo offset
        for cut in 0..data.len() {
            let _ = try_mount_from_ckpt(&data[..cut]); // nunca panics
        }
        // corromper cada byte uma vez + amostras LCG (foco no ckpt final)
        for pos in 0..data.len() {
            let mut c = data.clone();
            c[pos] ^= 0xFF;
            let _ = try_mount_from_ckpt(&c);
        }
        let mut state = 0xABCD_EF01u64;
        let n = data.len();
        for _ in 0..1000 {
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            let pos = ((state >> 32) as usize) % n;
            let bit = 1u8 << (state % 8);
            let mut c = data.clone();
            c[pos] ^= bit;
            let _ = try_mount_from_ckpt(&c);
        }
        // um mount Some() nunca pode devolver chaves além das 50 vivas
        assert!(m.len() <= 50);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn tickvfile_open_survives_torn_and_corrupt_ckpt() {
        // integração open(): cortes estratégicos + corrupção no ckpt → nunca
        // panics e dados anteriores sobrevivem (fallback para scan completo).
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tickv_fuzz_open.db");
        let _ = std::fs::remove_file(&path);
        {
            let mut s = TickvFile::open(&path).unwrap();
            for i in 0..20 {
                s.put(format!("k/{i:02}").as_bytes(), format!("v{i}").as_bytes()).unwrap();
            }
            s.checkpoint().unwrap();
        }
        let raw = std::fs::read(&path).unwrap();
        for cut in [0usize, raw.len() / 2, raw.len() - 512, raw.len() - 1] {
            let p2 = dir.join("tickv_fuzz_open_cut.db");
            std::fs::write(&p2, &raw[..cut]).unwrap();
            let mut s = TickvFile::open(&p2).unwrap(); // nunca panics
            for i in 0..20 {
                if let Some(v) = s.get(format!("k/{i:02}").as_bytes()).unwrap() {
                    assert_eq!(v, format!("v{i}").into_bytes(), "dado sobrevivente divergiu");
                }
            }
            let _ = std::fs::remove_file(&p2);
        }
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
            assert!(!db.scan_prefix("md/L1/").unwrap().is_empty());
            assert!(!db.scan_prefix("md/L3/").unwrap().is_empty());
        }
        {
            let mut db = crate::sgdb::Sgdb::open(TickvFile::open(&path).unwrap()).unwrap();
            assert!(!db.scan_prefix("md/L3/").unwrap().is_empty());
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
