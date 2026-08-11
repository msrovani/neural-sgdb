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

/// Nível de durabilidade garantido por um backend (maturation P4).
///
/// Durabilidade ≠ persistência: "write retornou Ok" não significa "sobreviveu
/// a power loss". Os níveis são cumulativos:
///
/// - `Buffered`: dados no buffer do OS — sobrevive a crash do processo, NÃO a
///   power loss.
/// - `Flushed`: write + flush — sobrevive a crash do processo; NÃO garante
///   sobrevivência a power loss em hardware real.
/// - `Durable`: write + flush + sync (fsync/fdatasync) — sobrevive a power
///   loss.
///
/// O default do crate é `Flushed` (performance). `sync_durable()` é a
/// operação explícita para quem precisa de `Durable` (ex: checkpoint) — o
/// caller decide o custo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Durability {
    Buffered,
    Flushed,
    Durable,
}

/// Backend plugável. Semântica: append-log power-loss safe — `put` idempotente,
/// `delete` grava tombstone; o crate garante CRC + recuperação de crash sobre
/// qualquer impl que siga essa semântica.
pub trait Storage {
    fn name(&self) -> &'static str;
    fn put(&mut self, key: &[u8], val: &[u8]) -> Result<(), SgdbError>;
    fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, SgdbError>;
    fn scan_prefix(&mut self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, SgdbError>;
    fn delete(&mut self, key: &[u8]) -> Result<(), SgdbError>;

    /// Nível de durabilidade garantido pelo `put` deste backend.
    /// Default `Buffered` — backends que fazem flush por write reportam
    /// `Flushed`; quem faz sync reporta `Durable`.
    fn durability(&self) -> Durability {
        Durability::Buffered
    }

    /// Persistência durável explícita (fsync/fdatasync quando suportado).
    /// No-op por default; o backend reporta o nível real via `durability()`.
    /// Não é chamado automaticamente pelo crate — o caller decide (ex:
    /// checkpoint = Durable, turno = Flushed).
    fn sync_durable(&mut self) -> Result<(), SgdbError> {
        Ok(())
    }
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
    fn durability(&self) -> Durability {
        // RAM pura — não sobrevive a crash nem power loss
        Durability::Buffered
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

/// CRC32 (IEEE 802.3, polinômio 0xEDB88320).
///
/// Tabela de 256 (const fn, zero-dep, no_std-safe): 1 op/byte em vez de 8
/// shifts+braço por byte — medido ~384 MiB/s → ~3 GiB/s. Mesmo resultado da
/// versão bitwise (golden `crc32_known_vector` e TKLV pinam o layout).
const fn crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

static CRC32_TABLE: [u32; 256] = crc32_table();

fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    let mut crc = crc;
    for &b in data {
        crc = (crc >> 8) ^ CRC32_TABLE[((crc ^ b as u32) & 0xFF) as usize];
    }
    crc
}

pub fn crc32(data: &[u8]) -> u32 {
    !crc32_update(0xFFFF_FFFF, data)
}

/// CRC32 sobre key‖val SEM concatenar (CRC é streaming): evita a alocação de
/// um Vec temporário por record em todo append + recovery de FileStorage.
/// Gate `file-storage`: sem a feature é dead-code (deny(warnings) no no_std).
#[cfg(feature = "file-storage")]
pub(crate) fn crc32_parts(a: &[u8], b: &[u8]) -> u32 {
    !crc32_update(crc32_update(0xFFFF_FFFF, a), b)
}

/// Append-log em arquivo (feature `file-storage`): registros
/// `[klen u32][vlen u32][crc u32][key][val]`, tombstone = vlen `u32::MAX`.
/// Recovery determinístico: records válidos são aplicados em ordem (last-wins
/// por key); uma cauda truncada OU corrompida (CRC falho, klen/vlen fora dos
/// limites, header malformado) é **truncada** no open — registros anteriores
/// intactos. NUNCA aceita corrupção no meio do stream: o primeiro record
/// inválido encerra a leitura (e o arquivo é truncado aí).
#[cfg(feature = "file-storage")]
pub struct FileStorage {
    path: std::path::PathBuf,
    map: BTreeMap<Vec<u8>, Vec<u8>>,
    /// Handle de append persistente e LAZY (perf): evita abrir+fechar o
    /// arquivo em CADA put — medido ~422µs/op vs ~8µs InMemory antes. Abre no
    /// primeiro `append`; `None` antes do primeiro put e durante `compact()`
    /// (fechado antes do rename — não apontar para inode/objeto antigo).
    file: Option<std::fs::File>,
}

#[cfg(feature = "file-storage")]
const TOMBSTONE: u32 = u32::MAX;

/// Limites de sanidade do parser (paridade com TKLV: klen ≤ 4KiB, vlen ≤ 1MiB).
#[cfg(feature = "file-storage")]
const MAX_KLEN: usize = 4096;
#[cfg(feature = "file-storage")]
const MAX_VLEN: usize = 1024 * 1024;

/// Parse seguro de u32 LE: retorna `None` se o slice é curto — nunca panics
/// em dados externos (maturation P2: parsing safety). `pub(crate)` para o
/// codec TKLV (tickv.rs) reutilizar; puro e no_std-safe, sem gate.
pub(crate) fn le32(b: &[u8]) -> Option<u32> {
    if b.len() < 4 {
        return None;
    }
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

#[cfg(feature = "file-storage")]
impl FileStorage {
    pub fn open(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut map = BTreeMap::new();
        if path.exists() {
            let data = std::fs::read(&path)?;
            let mut off = 0usize;
            let mut valid_end = 0usize;
            // Recovery determinístico: para no PRIMEIRO record inválido.
            // (cauda truncada, CRC falho, limites estourados ou header
            // malformado = a partir daqui o stream é não-confiável)
            while off + 12 <= data.len() {
                let (Some(klen), Some(vlen), Some(crc)) = (
                    le32(&data[off..off + 4]),
                    le32(&data[off + 4..off + 8]),
                    le32(&data[off + 8..off + 12]),
                ) else {
                    break; // header malformado (nunca deve ocorrer — guard acima)
                };
                let klen = klen as usize;
                if klen > MAX_KLEN {
                    break; // klen fora do limite — não confiar em lengths externos
                }
                // TOMBSTONE PRIMEIRO: vlen = u32::MAX é um delete válido, não um
                // length absurdo — checar antes do bound de vlen (senão o
                // tombstone é tratado como corrupção e a chave ressuscita)
                if vlen == TOMBSTONE {
                    // HIGH #1 (review P6): bounds ANTES do slice — um tombstone
                    // truncado (klen prometido mas key cortada no crash) nunca
                    // deve panicar; encerra a leitura como cauda corrompida.
                    if off + 12 + klen > data.len() {
                        break;
                    }
                    // MED #3 (review P6): tombstone também é verificado por CRC
                    // (key bit-rot não deve deletar a chave errada silenciosamente)
                    let tomb_key = &data[off + 12..off + 12 + klen];
                    if crc32(tomb_key) != crc {
                        break;
                    }
                    map.remove(tomb_key);
                    let end = off + 12 + klen;
                    off = end;
                    valid_end = end;
                    continue;
                }
                let vlen = vlen as usize;
                if vlen > MAX_VLEN {
                    break; // vlen fora do limite — cauda corrompida
                }
                let end = match off.checked_add(12).and_then(|e| e.checked_add(klen)) {
                    Some(e) => match e.checked_add(vlen) {
                        Some(e) => e,
                        None => break, // overflow aritmético — cauda corrompida
                    },
                    None => break,
                };
                if end > data.len() {
                    break; // cauda cortada (crash) — truncar aqui
                }
                let key = &data[off + 12..off + 12 + klen];
                let val = &data[off + 12 + klen..end];
                // CRC cobre key‖val (integridade do valor — bit rot não passa);
                // streaming sem concat (evita 1 alocação por record no recovery)
                if crc32_parts(key, val) != crc {
                    break; // record corrompido no meio — encerra leitura
                }
                map.insert(key.to_vec(), val.to_vec());
                off = end;
                valid_end = end;
            }
            // Trunca cauda inválida (crash/corrupção) — determinístico
            if valid_end < data.len() {
                use std::io::{Seek, SeekFrom};
                let mut f = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&path)?;
                f.seek(SeekFrom::Start(valid_end as u64))?;
                f.set_len(valid_end as u64)?;
            }
        }
        // Handle de append é LAZY (abre no primeiro append): manter o open()
        // sem custo extra de syscall — open/close estressado não paga um
        // CreateFile que não usará (medido 404µs→585µs com open eager).
        Ok(FileStorage { path, map, file: None })
    }

    fn append(&mut self, key: &[u8], val: &[u8]) -> Result<(), SgdbError> {
        use std::io::Write;
        // Bounds check ANTES do append (bughunt #11): um valor/chave acima dos
        // limites (MAX_KLEN/MAX_VLEN) era aceito aqui mas REJEITADO no recovery
        // (open), que para e TRUNCA o arquivo — apagando silenciosamente todos
        // os registros posteriores. Rejeitar na escrita = paridade com TickvFile.
        if key.len() > MAX_KLEN || val.len() > MAX_VLEN {
            return Err(SgdbError::Storage("limits"));
        }
        let vlen = if val.is_empty() {
            TOMBSTONE // delete: marker vlen
        } else {
            val.len() as u32
        };
        let mut buf = Vec::with_capacity(12 + key.len() + val.len());
        buf.extend_from_slice(&(key.len() as u32).to_le_bytes());
        buf.extend_from_slice(&vlen.to_le_bytes());
        // CRC cobre key‖val — streaming sem concatenar (menos 1 alocação/put)
        buf.extend_from_slice(&crc32_parts(key, val).to_le_bytes());
        buf.extend_from_slice(key);
        buf.extend_from_slice(val);
        // append(true) => cada write_all vai ao fim do arquivo (O_APPEND /
        // FILE_APPEND_DATA); flush em File é no-op (dados já no OS).
        if self.file.is_none() {
            self.file = Some(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)
                    .map_err(|_| SgdbError::Storage("open append"))?,
            );
        }
        let f = self.file.as_mut().unwrap();
        f.write_all(&buf)
            .map_err(|_| SgdbError::Storage("write"))?;
        f.flush().map_err(|_| SgdbError::Storage("flush"))
    }

    /// Compactação (maturation P4): reescreve o **live set** (map atual) como
    /// records frescos num arquivo temporário e troca atomicamente via rename.
    ///
    /// - Preserva keys vivas; remove tombstones e versões obsoletas (map já é
    ///   last-wins, e keys deletadas não estão no map).
    /// - Crash-safe: enquanto o temp não é renomeado, o arquivo original
    ///   permanece íntegro (recovery normal). Um temp órfão é sobrescrito na
    ///   próxima compactação.
    /// - Explícito/manual — sem threads nem async (decisão da sprint §7).
    pub fn compact(&mut self) -> Result<(), SgdbError> {
        use std::io::Write;
        let tmp = self.path.with_extension("compact.tmp");
        let mut f = std::fs::File::create(&tmp)
            .map_err(|_| SgdbError::Storage("compact create"))?;
        for (key, val) in self.map.iter() {
            let mut buf = Vec::with_capacity(12 + key.len() + val.len());
            buf.extend_from_slice(&(key.len() as u32).to_le_bytes());
            // LOW #4 (review P6): espelha o append — valor vazio = TOMBSTONE,
            // senão a mesma chave mudaria de significado pós-compactação
            // (append grava u32::MAX p/ delete; compact gravaria vlen=0 = put)
            let vlen = if val.is_empty() { TOMBSTONE } else { val.len() as u32 };
            buf.extend_from_slice(&vlen.to_le_bytes());
            buf.extend_from_slice(&crc32_parts(key, val).to_le_bytes());
            buf.extend_from_slice(key);
            buf.extend_from_slice(val);
            f.write_all(&buf)
                .map_err(|_| SgdbError::Storage("compact write"))?;
        }
        f.flush().map_err(|_| SgdbError::Storage("compact flush"))?;
        drop(f);
        // Fecha o handle persistente ANTES do rename: com um handle aberto,
        // escrever no inode/objeto antigo pós-rename seria perda de dados
        // (Unix: inode órfão; Windows: objeto trocado). Reabre LAZY no próximo
        // append (agora no novo arquivo).
        self.file = None;
        // troca atômica: rename sobre o original (Windows: MoveFileEx replace)
        std::fs::rename(&tmp, &self.path)
            .map_err(|_| SgdbError::Storage("compact rename"))
    }
}

#[cfg(feature = "file-storage")]
impl Storage for FileStorage {
    fn name(&self) -> &'static str {
        "file"
    }
    fn durability(&self) -> Durability {
        // write + flush por append → sobrevive a crash de processo; NÃO
        // garante power loss (fsync é explícito via sync_durable)
        Durability::Flushed
    }
    fn sync_durable(&mut self) -> Result<(), SgdbError> {
        // fsync/fdatasync real: força dados do page cache para o dispositivo.
        // Handle read+write: no Windows, FlushFileBuffers (sync_all) exige
        // acesso de escrita — handle read-only falha.
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.path)
            .map_err(|_| SgdbError::Storage("open sync"))?;
        f.sync_all().map_err(|_| SgdbError::Storage("sync_all"))
    }
    fn put(&mut self, key: &[u8], val: &[u8]) -> Result<(), SgdbError> {
        self.append(key, val)?;
        // Valor vazio == tombstone no log (vlen u32::MAX) — o mapa deve
        // espelhar o mesmo (bughunt #11): senão put(k, &[]) devolveria
        // Some([]) em sessão mas None após reopen (last-wins inconsistente
        // entre os dois pontos de leitura).
        if val.is_empty() {
            self.map.remove(key);
        } else {
            self.map.insert(key.to_vec(), val.to_vec());
        }
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

    #[test]
    fn durability_levels_reported() {
        // Durability honesta (maturation P4): InMemory = Buffered (RAM pura)
        let im = InMemory::new();
        assert_eq!(im.durability(), Durability::Buffered);
        #[cfg(feature = "file-storage")]
        {
            // FileStorage = Flushed (write+flush, sem fsync automático)
            let dir = std::env::temp_dir().join("neural_sgdb_test");
            let _ = std::fs::create_dir_all(&dir);
            let p = dir.join("dur.db");
            let _ = std::fs::remove_file(&p);
            let mut fs = FileStorage::open(&p).unwrap();
            assert_eq!(fs.durability(), Durability::Flushed);
            fs.put(b"k", b"v").unwrap();
            // sync_durable explícito funciona (fsync real em arquivo)
            assert!(fs.sync_durable().is_ok());
            let mut fs2 = FileStorage::open(&p).unwrap();
            assert_eq!(fs2.get(b"k").unwrap(), Some(b"v".to_vec()));
            let _ = std::fs::remove_file(&p);
        }
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
        {            let mut s = FileStorage::open(&path).unwrap();
            s.put(b"x", b"1").unwrap();
            s.delete(b"x").unwrap();
        }
        {
            let mut s = FileStorage::open(&path).unwrap();
            assert!(s.get(b"x").unwrap().is_none());
        }
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn put_oversized_rejected_before_append() {
        // bughunt #11: um valor acima de MAX_VLEN era ACEITO no put mas
        // REJEITADO no recovery (open), que para e TRUNCA o arquivo — todos os
        // registros posteriores eram silenciosamente apagados. A escrita deve
        // falhar na origem (paridade com TickvFile), preservando o log.
        let p = tmp_path("fz_oversize.db");
        {
            let mut s = FileStorage::open(&p).unwrap();
            s.put(b"a", b"1").unwrap();
            let big = vec![0u8; MAX_VLEN + 1];
            assert!(s.put(b"big", &big).is_err(), "oversized put deveria falhar");
            let long_key = vec![b'k'; MAX_KLEN + 1];
            assert!(s.put(&long_key, b"v").is_err(), "oversized key deveria falhar");
            s.put(b"b", b"2").unwrap(); // log intacto após a rejeição
        }
        {
            let mut s = FileStorage::open(&p).unwrap();
            assert_eq!(s.get(b"a").unwrap(), Some(b"1".to_vec()));
            assert_eq!(s.get(b"b").unwrap(), Some(b"2".to_vec()));
            assert!(s.get(b"big").unwrap().is_none());
        }
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn put_empty_value_is_consistent_delete() {
        // bughunt #11: put(k, &[]) grava tombstone (vlen = u32::MAX) no log,
        // mas antes mantinha `k -> []` no mapa em memória → get() devolvia
        // Some([]) em sessão e None após reopen (last-wins inconsistente).
        // put com valor vazio == delete, nos DOIS pontos de leitura.
        let p = tmp_path("fz_emptyval.db");
        {
            let mut s = FileStorage::open(&p).unwrap();
            s.put(b"k", b"v").unwrap();
            s.put(b"k", b"").unwrap();
            assert!(s.get(b"k").unwrap().is_none(), "em sessão deveria sumir");
            s.put(b"k2", b"").unwrap(); // chave nunca existente também some
            assert!(s.get(b"k2").unwrap().is_none());
        }
        {
            let mut s = FileStorage::open(&p).unwrap();
            assert!(s.get(b"k").unwrap().is_none(), "após reopen deveria sumir");
            assert!(s.get(b"k2").unwrap().is_none());
        }
        let _ = std::fs::remove_file(&p);
    }

    // ── Fault-injection: recovery determinístico (maturation P2) ─────────────
    // Helper: escreve bytes crus no arquivo (simula crash/corrupção)
    #[cfg(feature = "file-storage")]
    fn write_raw(path: &std::path::Path, bytes: &[u8]) {
        std::fs::write(path, bytes).unwrap();
    }

    #[cfg(feature = "file-storage")]
    fn tmp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join(name);
        let _ = std::fs::remove_file(&p);
        p
    }

    #[cfg(feature = "file-storage")]
    fn rec(key: &[u8], val: &[u8]) -> Vec<u8> {
        // réplica do formato append do FileStorage (header + key‖val + CRC)
        let mut kv = Vec::new();
        kv.extend_from_slice(key);
        kv.extend_from_slice(val);
        let mut buf = Vec::with_capacity(12 + key.len() + val.len());
        buf.extend_from_slice(&(key.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(val.len() as u32).to_le_bytes());
        buf.extend_from_slice(&crc32(&kv).to_le_bytes());
        buf.extend_from_slice(key);
        buf.extend_from_slice(val);
        buf
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn recovery_empty_file() {
        let p = tmp_path("fz_empty.db");
        write_raw(&p, b"");
        let s = FileStorage::open(&p).unwrap();
        assert!(s.map.is_empty());
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn recovery_truncated_tail() {
        let p = tmp_path("fz_trunc.db");
        let mut data = rec(b"a", b"1");
        data.extend_from_slice(&rec(b"b", b"2"));
        data.truncate(data.len() - 5); // cauda cortada no meio do 2º record
        write_raw(&p, &data);
        let mut s = FileStorage::open(&p).unwrap();
        // 1º record íntegro sobrevive; cauda truncada descartada (e truncada no open)
        assert_eq!(s.get(b"a").unwrap(), Some(b"1".to_vec()));
        assert!(s.get(b"b").unwrap().is_none());
        // open truncou o arquivo (cauda removida)
        let raw = std::fs::read(&p).unwrap();
        assert_eq!(raw.len(), rec(b"a", b"1").len());
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn recovery_corrupt_mid_stream() {
        let p = tmp_path("fz_corrupt.db");
        let mut data = rec(b"a", b"1");
        data.extend_from_slice(&rec(b"b", b"2")); // corrompe o CRC do 2º
        let blen = data.len();
        data[rec(b"a", b"1").len() + 8] ^= 0xFF; // vira CRC do record b
        write_raw(&p, &data);
        let mut s = FileStorage::open(&p).unwrap();
        assert_eq!(s.get(b"a").unwrap(), Some(b"1".to_vec()));
        assert!(s.get(b"b").unwrap().is_none()); // CRC falho → para aí
        assert!(std::fs::metadata(&p).unwrap().len() < blen as u64); // truncado
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn recovery_truncated_tombstone_never_panics() {
        // HIGH #1 (review P6): tombstone com klen prometido mas key cortada no
        // crash — o open NUNCA deve panicar (slice fora de bounds); encerra a
        // leitura como cauda corrompida e preserva os records anteriores.
        let p = tmp_path("fz_tomb_trunc.db");
        let mut data = rec(b"a", b"1");
        // header de tombstone (klen=100, vlen=u32::MAX) mas SEM os 100 bytes
        // de key — cauda cortada exatamente no header
        data.extend_from_slice(&100u32.to_le_bytes()); // klen
        data.extend_from_slice(&TOMBSTONE.to_le_bytes()); // vlen
        data.extend_from_slice(&0u32.to_le_bytes()); // crc (irrelevante)
        write_raw(&p, &data);
        let mut s = FileStorage::open(&p).unwrap(); // não panics
        assert_eq!(s.get(b"a").unwrap(), Some(b"1".to_vec()));
        assert!(s.map.len() == 1);
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn recovery_invalid_lengths() {
        let p = tmp_path("fz_badlen.db");
        // klen absurdo (0xFFFFFFFF) → fora dos limites → para/trunca
        let mut data = rec(b"a", b"1");
        data.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0]);
        write_raw(&p, &data);
        let mut s = FileStorage::open(&p).unwrap();
        assert_eq!(s.get(b"a").unwrap(), Some(b"1".to_vec()));
        assert!(s.map.len() == 1);
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn recovery_malformed_header() {
        let p = tmp_path("fz_malformed.db");
        let mut data = rec(b"a", b"1");
        data.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0xDE, 0xAD, 0xBE, 0xEF]);
        write_raw(&p, &data);
        let mut s = FileStorage::open(&p).unwrap();
        assert_eq!(s.get(b"a").unwrap(), Some(b"1".to_vec()));
        assert!(s.map.len() == 1);
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn recovery_duplicate_keys_last_wins() {
        let p = tmp_path("fz_dup.db");
        let mut data = rec(b"k", b"v1");
        data.extend_from_slice(&rec(b"k", b"v2")); // overwrite → last-wins
        write_raw(&p, &data);
        let mut s = FileStorage::open(&p).unwrap();
        assert_eq!(s.get(b"k").unwrap(), Some(b"v2".to_vec()));
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn recovery_tombstone_removes() {
        let p = tmp_path("fz_tomb.db");
        let mut data = rec(b"x", b"1");
        // tombstone: vlen = u32::MAX, val vazio
        let mut kv = Vec::new();
        kv.extend_from_slice(b"x");
        let mut t = Vec::new();
        t.extend_from_slice(&1u32.to_le_bytes()); // klen
        t.extend_from_slice(&TOMBSTONE.to_le_bytes()); // vlen = tombstone
        t.extend_from_slice(&crc32(&kv).to_le_bytes());
        t.extend_from_slice(b"x");
        data.extend_from_slice(&t);
        write_raw(&p, &data);
        let mut s = FileStorage::open(&p).unwrap();
        assert!(s.get(b"x").unwrap().is_none()); // removido pelo tombstone
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn recovery_partial_final_record() {
        let p = tmp_path("fz_partial.db");
        let mut data = rec(b"a", b"1");
        // header diz vlen=100 mas só há 3 bytes → cauda parcial
        let mut kv = Vec::new();
        kv.extend_from_slice(b"p");
        let mut part = Vec::new();
        part.extend_from_slice(&1u32.to_le_bytes());
        part.extend_from_slice(&100u32.to_le_bytes());
        part.extend_from_slice(&crc32(&kv).to_le_bytes());
        part.extend_from_slice(b"p");
        part.extend_from_slice(b"xy"); // menos que os 100 prometidos
        data.extend_from_slice(&part);
        write_raw(&p, &data);
        let mut s = FileStorage::open(&p).unwrap();
        assert_eq!(s.get(b"a").unwrap(), Some(b"1".to_vec()));
        assert!(s.get(b"p").unwrap().is_none()); // record parcial descartado
        let _ = std::fs::remove_file(&p);
    }

    // ── Compaction: pre/post state, tombstones, atomicidade (maturation P4) ─

    #[cfg(feature = "file-storage")]
    #[test]
    fn compact_preserves_live_removes_dead() {
        let p = tmp_path("fz_compact.db");
        {
            let mut s = FileStorage::open(&p).unwrap();
            s.put(b"a", b"1").unwrap();
            s.put(b"b", b"2").unwrap();
            s.put(b"a", b"1v2").unwrap(); // overwrite → versão obsoleta no log
            s.put(b"c", b"3").unwrap();
            s.delete(b"c").unwrap(); // tombstone
            // pré-compactação: log contém a, b, a(v2), c, tombstone(c)
            assert_eq!(s.get(b"a").unwrap(), Some(b"1v2".to_vec()));
            assert_eq!(s.get(b"b").unwrap(), Some(b"2".to_vec()));
            assert!(s.get(b"c").unwrap().is_none());
            s.compact().unwrap();
            // pós: mapa in-memory intacto
            assert_eq!(s.get(b"a").unwrap(), Some(b"1v2".to_vec()));
            assert_eq!(s.get(b"b").unwrap(), Some(b"2".to_vec()));
        }
        // reopen do arquivo compactado: só o live set sobreviveu
        let mut s = FileStorage::open(&p).unwrap();
        assert_eq!(s.get(b"a").unwrap(), Some(b"1v2".to_vec()));
        assert_eq!(s.get(b"b").unwrap(), Some(b"2".to_vec()));
        assert!(s.get(b"c").unwrap().is_none()); // tombstone não ressuscitou
        // atomicidade: sem arquivo temp órfão
        assert!(!p.with_extension("compact.tmp").exists());
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn compact_shrinks_file() {
        let p = tmp_path("fz_shrink.db");
        let before_len;
        let after_len;
        {
            let mut s = FileStorage::open(&p).unwrap();
            for i in 0..100 {
                s.put(format!("k{i:03}").as_bytes(), b"value").unwrap();
            }
            for i in (0..100).step_by(2) {
                s.delete(format!("k{i:03}").as_bytes()).unwrap(); // 50 tombstones
            }
            before_len = std::fs::metadata(&p).unwrap().len();
            s.compact().unwrap();
            after_len = std::fs::metadata(&p).unwrap().len();
        }
        // 50 keys vivas, 50 tombstoned — compactação remove os tombstones
        assert!(after_len < before_len, "compact não encolheu: {before_len}→{after_len}");
        let s = FileStorage::open(&p).unwrap();
        assert_eq!(s.map.len(), 50);
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn compact_then_append_persists() {
        // Invariante de durabilidade do handle persistente (perf): após o
        // rename do compact, o put deve escrever no NOVO arquivo — nunca no
        // inode/objeto antigo (Unix: inode órfão; Windows: objeto trocado),
        // senão o registro pós-compactação se perde silenciosamente.
        let p = tmp_path("fz_compact_append.db");
        {
            let mut s = FileStorage::open(&p).unwrap();
            s.put(b"a", b"1").unwrap();
            s.put(b"b", b"2").unwrap();
            s.compact().unwrap();
            s.put(b"c", b"3").unwrap(); // escrita crítica pós-rename
            assert_eq!(s.get(b"c").unwrap(), Some(b"3".to_vec()));
        }
        {
            let mut s = FileStorage::open(&p).unwrap();
            assert_eq!(s.get(b"a").unwrap(), Some(b"1".to_vec()));
            assert_eq!(s.get(b"b").unwrap(), Some(b"2".to_vec()));
            assert_eq!(s.get(b"c").unwrap(), Some(b"3".to_vec()));
        }
        // arquivo == exatamente 3 records (sem cauda duplicada/stale)
        let raw = std::fs::read(&p).unwrap();
        assert_eq!(raw.len(), rec(b"a", b"1").len() * 3, "append pós-compact deve estar no novo arquivo");
        assert!(!p.with_extension("compact.tmp").exists());
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn lazy_handle_opens_on_first_put() {
        // Handle de append é LAZY: open() sem criar/abrir o arquivo (sem
        // syscall extra no open/close stress); o primeiro put materializa e
        // escreve. Durability "Flushed" preservada (write_all + flush por put).
        let p = tmp_path("fz_lazy.db");
        {
            let s = FileStorage::open(&p).unwrap();
            assert!(!p.exists(), "open lazy não deve criar o arquivo");
        }
        {
            let mut s = FileStorage::open(&p).unwrap();
            s.put(b"k", b"v").unwrap();
            assert!(p.exists(), "primeiro put deve criar o arquivo");
            assert_eq!(s.get(b"k").unwrap(), Some(b"v".to_vec()));
        }
        {
            let mut s = FileStorage::open(&p).unwrap();
            assert_eq!(s.get(b"k").unwrap(), Some(b"v".to_vec()));
        }
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn compact_recovery_after_crash_temp() {
        // Temp órfão (crash antes do rename) é sobrescrito na próxima compact
        let p = tmp_path("fz_crashcompact.db");
        let tmp = p.with_extension("compact.tmp");
        std::fs::write(&tmp, b"orphan-garbage").unwrap(); // crash simulado
        {
            let mut s = FileStorage::open(&p).unwrap();
            s.put(b"a", b"1").unwrap();
            s.compact().unwrap(); // sobrescreve o órfão
        }
        let mut s = FileStorage::open(&p).unwrap();
        assert_eq!(s.get(b"a").unwrap(), Some(b"1".to_vec()));
        assert!(!tmp.exists()); // órfão substituído
        let _ = std::fs::remove_file(&p);
    }
}
