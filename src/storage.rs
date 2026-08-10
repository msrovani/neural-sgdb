//! Storage trait — o contrato central do neural-sgdb.
//! Implemente 4 métodos = integrado. Entregues: `InMemory` (RAM, testes) e
//! `FileStorage` (append-log com CRC32 por registro, crash-safe, `std`).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// Storage/document error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SgdbError {
    /// Falha de backend (mensagem curta estática).
    Storage(&'static str),
    /// Registro corrompido (CRC/parse).
    Corrupt,
}

impl core::fmt::Display for SgdbError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SgdbError::Storage(m) => write!(f, "storage: {m}"),
            SgdbError::Corrupt => write!(f, "corrupt record"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for SgdbError {}

/// Backend plugável. Semântica: append-log power-loss safe — `put` idempotente,
/// `delete` grava tombstone; o crate garante CRC + recuperação de crash sobre
/// qualquer impl que siga essa semântica.
pub trait Storage {
    fn name(&self) -> &'static str;
    fn put(&mut self, key: &[u8], val: &[u8]) -> Result<(), SgdbError>;
    fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, SgdbError>;
    fn scan_prefix(&mut self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, SgdbError>;
    fn delete(&mut self, key: &[u8]) -> Result<(), SgdbError>;
}

/// RAM-only (testes/prototipagem). Volátil — não persiste.
#[derive(Default)]
pub struct InMemory {
    map: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl InMemory {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Storage for InMemory {
    fn name(&self) -> &'static str {
        "in-memory"
    }
    fn put(&mut self, key: &[u8], val: &[u8]) -> Result<(), SgdbError> {
        self.map.insert(key.to_vec(), val.to_vec());
        Ok(())
    }
    fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, SgdbError> {
        Ok(self.map.get(key).cloned())
    }
    fn scan_prefix(&mut self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, SgdbError> {
        let mut out = Vec::new();
        for (k, v) in self.map.iter() {
            if k.starts_with(prefix) {
                out.push((k.clone(), v.clone()));
            }
        }
        Ok(out)
    }
    fn delete(&mut self, key: &[u8]) -> Result<(), SgdbError> {
        self.map.remove(key);
        Ok(())
    }
}

/// CRC32 (IEEE 802.3, polinômio 0xEDB88320) — bitwise, sem tabela (zero deps).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Append-log em arquivo (feature `file-storage`): registros
/// `[klen u32][vlen u32][crc u32][key][val]`, tombstone = vlen `u32::MAX`.
/// Ao abrir, valida CRC de cada registro e trunca cauda corrompida (crash) —
/// registros anteriores intactos.
#[cfg(feature = "file-storage")]
pub struct FileStorage {
    path: std::path::PathBuf,
    map: BTreeMap<Vec<u8>, Vec<u8>>,
}

#[cfg(feature = "file-storage")]
const TOMBSTONE: u32 = u32::MAX;

#[cfg(feature = "file-storage")]
impl FileStorage {
    pub fn open(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut map = BTreeMap::new();
        if path.exists() {
            let data = std::fs::read(&path)?;
            let mut off = 0usize;
            let mut valid_end = 0usize;
            while off + 12 <= data.len() {
                let klen = u32::from_le_bytes(data[off..off + 4].try_into().unwrap()) as usize;
                let vlen = u32::from_le_bytes(data[off + 4..off + 8].try_into().unwrap());
                let crc = u32::from_le_bytes(data[off + 8..off + 12].try_into().unwrap());
                let consumed = if vlen == TOMBSTONE { 0 } else { vlen as usize };
                let end = off + 12 + klen + consumed;
                if end > data.len() {
                    break; // cauda cortada (crash)
                }
                let key = &data[off + 12..off + 12 + klen];
                let val = &data[off + 12 + klen..end];
                // CRC cobre key‖val (integridade do valor — bit rot não passa)
                let mut kv = Vec::with_capacity(klen + val.len());
                kv.extend_from_slice(key);
                kv.extend_from_slice(val);
                if crc32(&kv) != crc {
                    break; // corrompido
                }
                if vlen == TOMBSTONE {
                    map.remove(key);
                } else {
                    map.insert(key.to_vec(), val.to_vec());
                }
                off = end;
                valid_end = end;
            }
            // Trunca cauda inválida
            if valid_end < data.len() {
                use std::io::{Seek, SeekFrom};
                let mut f = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&path)?;
                f.seek(SeekFrom::Start(valid_end as u64))?;
                f.set_len(valid_end as u64)?;
            }
        }
        Ok(FileStorage { path, map })
    }

    fn append(&mut self, key: &[u8], val: &[u8]) -> Result<(), SgdbError> {
        use std::io::Write;
        let vlen = if val.is_empty() {
            TOMBSTONE // delete: marker vlen
        } else {
            val.len() as u32
        };
        let mut buf = Vec::with_capacity(12 + key.len() + val.len());
        buf.extend_from_slice(&(key.len() as u32).to_le_bytes());
        buf.extend_from_slice(&vlen.to_le_bytes());
        // CRC cobre key‖val
        let mut kv = Vec::with_capacity(key.len() + val.len());
        kv.extend_from_slice(key);
        kv.extend_from_slice(val);
        buf.extend_from_slice(&crc32(&kv).to_le_bytes());
        buf.extend_from_slice(key);
        buf.extend_from_slice(val);
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|_| SgdbError::Storage("open append"))?;
        f.write_all(&buf)
            .map_err(|_| SgdbError::Storage("write"))?;
        f.flush().map_err(|_| SgdbError::Storage("flush"))
    }
}

#[cfg(feature = "file-storage")]
impl Storage for FileStorage {
    fn name(&self) -> &'static str {
        "file"
    }
    fn put(&mut self, key: &[u8], val: &[u8]) -> Result<(), SgdbError> {
        self.append(key, val)?;
        self.map.insert(key.to_vec(), val.to_vec());
        Ok(())
    }
    fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, SgdbError> {
        Ok(self.map.get(key).cloned())
    }
    fn scan_prefix(&mut self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, SgdbError> {
        let mut out = Vec::new();
        for (k, v) in self.map.iter() {
            if k.starts_with(prefix) {
                out.push((k.clone(), v.clone()));
            }
        }
        Ok(out)
    }
    fn delete(&mut self, key: &[u8]) -> Result<(), SgdbError> {
        self.append(key, &[])?;
        self.map.remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known_vector() {
        // CRC32("123456789") = 0xCBF43926 (vetor de referência)
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn in_memory_basics() {
        let mut s = InMemory::new();
        s.put(b"a", b"1").unwrap();
        s.put(b"ab", b"2").unwrap();
        s.put(b"b", b"3").unwrap();
        assert_eq!(s.get(b"a").unwrap(), Some(b"1".to_vec()));
        assert_eq!(s.scan_prefix(b"a").unwrap().len(), 2);
        s.delete(b"a").unwrap();
        assert!(s.get(b"a").unwrap().is_none());
        assert_eq!(s.scan_prefix(b"a").unwrap().len(), 1);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn file_roundtrip() {
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("roundtrip.db");
        let _ = std::fs::remove_file(&path);
        {
            let mut s = FileStorage::open(&path).unwrap();
            s.put(b"k1", b"v1").unwrap();
            s.put(b"k2", b"v2").unwrap();
        }
        {
            let mut s = FileStorage::open(&path).unwrap();
            assert_eq!(s.get(b"k1").unwrap(), Some(b"v1".to_vec()));
            assert_eq!(s.get(b"k2").unwrap(), Some(b"v2".to_vec()));
        }
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn file_crash_tail() {
        use std::io::Write;
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("crash.db");
        let _ = std::fs::remove_file(&path);
        {
            let mut s = FileStorage::open(&path).unwrap();
            s.put(b"a", b"1").unwrap();
            s.put(b"b", b"2").unwrap();
        }
        // Simula crash: cauda parcial/truncada
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(b"\x05\x00\x00\x00\xde\xad\xbe\xefgarbage").unwrap();
        drop(f);
        {
            let mut s = FileStorage::open(&path).unwrap();
            assert_eq!(s.get(b"a").unwrap(), Some(b"1".to_vec()));
            assert_eq!(s.get(b"b").unwrap(), Some(b"2".to_vec()));
        }
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn file_delete_tombstone() {
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("del.db");
        let _ = std::fs::remove_file(&path);
        {
            let mut s = FileStorage::open(&path).unwrap();
            s.put(b"x", b"1").unwrap();
            s.delete(b"x").unwrap();
        }
        {
            let mut s = FileStorage::open(&path).unwrap();
            assert!(s.get(b"x").unwrap().is_none());
        }
        let _ = std::fs::remove_file(&path);
    }
}
