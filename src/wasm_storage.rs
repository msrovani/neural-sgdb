//! Seam WASM/OPFS (v1.1.15 §4 P1): o core é `no_std` + zero-dep e já compila
//! para `wasm32-unknown-unknown`; o que falta no browser não é índice — é
//! ONDE os bytes moram. Este módulo documenta o wiring e dá o backend sem DEP:
//!
//! - lib: `SnapshotStorage` (`src/storage.rs`) — `Storage` em RAM +
//!   `to_bytes`/`from_bytes` determinísticos (nunca panic).
//! - host (JS/Rust-wasm): lê/escreve o snapshot num arquivo OPFS
//!   (`opfs’artefato/nsgdb.snapshot`) ou IndexedDB; na abertura, `from_bytes`;
//!   a cada `checkpoint()`, `to_bytes` + escrita atômica.
//! - embedder no browser: `nsgdb-embed` baseline 384d já roda em WASM
//!   (puro Rust); ONNX via `tract` quando o host quiser.
//!
//! Nenhum `web-sys`/`wasm-bindgen` no lib (ADR-0001). O exemplo
//! `examples/wasm_backend.rs` demonstra o ciclo export → import sem browser.

use alloc::vec::Vec;
use crate::storage::SnapshotStorage;

/// Serializa o backend para persistir no OPFS/IndexedDB.
pub fn export_snapshot(s: &SnapshotStorage) -> Vec<u8> {
    s.to_bytes()
}

/// Restaura o backend a partir dos bytes do OPFS/IndexedDB.
pub fn import_snapshot(bytes: &[u8]) -> Result<SnapshotStorage, crate::storage::SgdbError> {
    SnapshotStorage::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Storage;

    #[test]
    fn snapshot_roundtrip_survives_opfs_cycle() {
        let mut s = SnapshotStorage::new();
        s.put(b"md/L3/a", b"ola").unwrap();
        s.put(b"sys/ttl/md/L3/a", &123u64.to_le_bytes()).unwrap();
        let bytes = export_snapshot(&s);
        let mut r = import_snapshot(&bytes).unwrap();
        assert_eq!(r.get(b"md/L3/a").unwrap().unwrap(), b"ola");
        assert_eq!(r.scan_prefix(b"md/").unwrap().len(), 1);
        // truncado → Err, nunca panic
        assert!(import_snapshot(&bytes[..bytes.len() / 2]).is_err());
        assert!(import_snapshot(b"").is_err());
    }
}
