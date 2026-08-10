//! ADR-0063 F3/D2 — AiosDatabaseEngine: MemoryDoc ↔ Storage + ART + BQ.
//! L0/L1: RAM-only por default (checkpoint explícito). ART guarda id lógico;
//! key = `md/Lx/...`. Instance-based (sem global ENGINE) — port do OS mãe.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::art::ArtIndex;
use crate::bq::BqFlatIndex;
use crate::memory_doc::{MemoryDoc, MemoryDocView, MemoryLayer, MemoryState};
use crate::storage::{Storage, SgdbError};

/// Contador monotônico de handles internos (ART / BQ ids).
static NEXT_ID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

/// Namespace lateral de estado lógico (`sys/state/<storage_key>` → u8).
/// Persistido via Storage CRU (não NMD1) — o contrato byte-idêntico com o OS
/// fica intacto (maturation P5).
fn state_key(sk: &str) -> Vec<u8> {
    let mut k = Vec::with_capacity(10 + sk.len());
    k.extend_from_slice(b"sys/state/");
    k.extend_from_slice(sk.as_bytes());
    k
}

fn is_ram_layer(layer: MemoryLayer) -> bool {
    matches!(layer, MemoryLayer::L0Sensory | MemoryLayer::L1Working)
}

pub struct AiosDatabaseEngine {
    pub art: ArtIndex,
    pub bq: BqFlatIndex,
    /// node_id local para vector clock
    pub node_id: u8,
    pub puts: u64,
    pub gets: u64,
    /// Blobs L0/L1 encoded (storage_key → NMD1); não toca Storage até checkpoint.
    ram_l0l1: BTreeMap<String, Vec<u8>>,
    /// Puts L0/L1 que bypassaram Storage (métrica honesty).
    pub ram_puts: u64,
    /// id lógico → storage_key (recall BQ → doc).
    id_to_sk: BTreeMap<u64, String>,
    storage: Box<dyn Storage>,
}

impl AiosDatabaseEngine {
    pub fn new(node_id: u8, storage: Box<dyn Storage>) -> Self {
        AiosDatabaseEngine {
            art: ArtIndex::new(),
            bq: BqFlatIndex::new(),
            node_id,
            puts: 0,
            gets: 0,
            ram_l0l1: BTreeMap::new(),
            ram_puts: 0,
            id_to_sk: BTreeMap::new(),
            storage,
        }
    }

    pub fn backend_name(&self) -> &'static str {
        self.storage.name()
    }

    /// Persiste doc: L0/L1 → RAM; demais → Storage (`md/Lx/key`) + indexa.
    pub fn put(&mut self, mut doc: MemoryDoc) -> Result<u64, SgdbError> {
        doc.clock.tick(self.node_id);
        let id = NEXT_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let sk = doc.storage_key();
        let blob = doc.encode();

        if is_ram_layer(doc.layer) {
            self.ram_l0l1.insert(sk.clone(), blob);
            self.ram_puts = self.ram_puts.saturating_add(1);
        } else {
            self.storage.put(sk.as_bytes(), &blob)?;
        }

        self.index_doc(id, &doc, &sk);
        self.puts += 1;
        Ok(id)
    }

    /// Flush L0/L1 RAM → Storage. Honesty: sem isto, reboot perde L0/L1.
    pub fn checkpoint_l0l1(&mut self) -> Result<usize, SgdbError> {
        let mut n = 0usize;
        for (sk, blob) in self.ram_l0l1.iter() {
            self.storage.put(sk.as_bytes(), blob)?;
            n += 1;
        }
        Ok(n)
    }

    /// Pós-checkpoint: drop arena RAM (docs já no Storage sob `md/L0|L1/…`).
    pub fn prune_ram_l0l1(&mut self) -> usize {
        let n = self.ram_l0l1.len();
        self.ram_l0l1.clear();
        n
    }

    fn index_doc(&mut self, id: u64, doc: &MemoryDoc, sk: &str) {
        self.id_to_sk.insert(id, String::from(sk));
        match doc.layer {
            MemoryLayer::L0Sensory
            | MemoryLayer::L1Working
            | MemoryLayer::L2EpisodicShort
            | MemoryLayer::L3EpisodicLong => {
                self.art.insert(sk, id);
            }
            MemoryLayer::L4Semantic | MemoryLayer::L5Procedural => {
                if let Some(ref bv) = doc.bitvec {
                    self.bq.insert(id, bv.clone());
                } else if !doc.payload.is_empty() {
                    let n = doc.payload.len() / 4;
                    if n > 0 {
                        let mut f = Vec::with_capacity(n);
                        for i in 0..n {
                            let o = i * 4;
                            let w = f32::from_le_bytes([
                                doc.payload[o],
                                doc.payload[o + 1],
                                doc.payload[o + 2],
                                doc.payload[o + 3],
                            ]);
                            f.push(w);
                        }
                        self.bq.insert_f32(id, &f);
                    }
                }
                self.art.insert(sk, id);
            }
            MemoryLayer::L6Reserved | MemoryLayer::L7Identity => {
                self.art.insert(sk, id);
            }
        }
    }

    /// Reconstrói ART/BQ a partir de keys Storage `md/*` (pós-remount).
    /// Retorna o número de docs reindexados; propaga erro de scan (P1 —
    /// recovery observável: storage ilegível NÃO abre "ready" silencioso).
    pub fn rebuild_indices_from_storage(&mut self) -> Result<usize, SgdbError> {
        self.art.clear();
        self.bq.clear();
        self.id_to_sk.clear();
        // Reindex RAM L0/L1 first (logical ids fresh)
        let mut n = 0usize;
        let ram_keys: Vec<(String, Vec<u8>)> = self
            .ram_l0l1
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (sk, bytes) in ram_keys {
            if let Ok(doc) = MemoryDoc::decode(&bytes) {
                let id = NEXT_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                self.index_doc(id, &doc, &sk);
                n += 1;
            }
        }
        let keys = self.storage.scan_prefix(b"md/")?;
        for (sk_bytes, bytes) in keys {
            let sk = String::from_utf8_lossy(&sk_bytes).into_owned();
            if self.ram_l0l1.contains_key(&sk) {
                continue;
            }
            if let Ok(doc) = MemoryDoc::decode(&bytes) {
                let id = NEXT_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                self.index_doc(id, &doc, &sk);
                n += 1;
            }
        }
        Ok(n)
    }

    pub fn get(&mut self, layer: MemoryLayer, key: &str) -> Result<Option<MemoryDoc>, SgdbError> {
        let sk = alloc::format!("md/{}/{}", layer.as_str(), key);
        self.get_by_storage_key(&sk)
    }

    /// Load por storage key canônica `md/Lx/...` (RAM L0/L1 ou Storage).
    pub fn get_by_storage_key(&mut self, sk: &str) -> Result<Option<MemoryDoc>, SgdbError> {
        self.gets += 1;
        if let Some(bytes) = self.ram_l0l1.get(sk) {
            return Ok(Some(MemoryDoc::decode(bytes).map_err(|_| SgdbError::Corrupt)?));
        }
        match self.storage.get(sk.as_bytes()) {
            Ok(Some(bytes)) => Ok(Some(MemoryDoc::decode(&bytes).map_err(|_| SgdbError::Corrupt)?)),
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Load por storage key canônica `md/Lx/...` (RAM L0/L1 ou Storage).
    #[allow(dead_code)] // port-parity: usado internamente e em v0.2 (dumps)
    pub fn get_view_bytes(
        &mut self,
        layer: MemoryLayer,
        key: &str,
    ) -> Result<Option<Vec<u8>>, SgdbError> {
        self.gets += 1;
        let sk = alloc::format!("md/{}/{}", layer.as_str(), key);
        if let Some(bytes) = self.ram_l0l1.get(&sk) {
            let _ = MemoryDocView::parse(bytes).map_err(|_| SgdbError::Corrupt)?;
            return Ok(Some(bytes.clone()));
        }
        match self.storage.get(sk.as_bytes()) {
            Ok(Some(bytes)) => {
                let _ = MemoryDocView::parse(&bytes).map_err(|_| SgdbError::Corrupt)?;
                Ok(Some(bytes))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }

    #[allow(dead_code)] // port-parity: API de lookup direto (v0.2 benchmarks)
    pub fn art_lookup(&self, storage_key: &str) -> Option<u64> {
        self.art.get(storage_key)
    }

    pub fn bq_top_k_f32(&self, query: &[f32], k: usize) -> Vec<(u64, u32)> {
        self.bq.top_k_f32(query, k)
    }

    pub fn storage_key_of(&self, id: u64) -> Option<&str> {
        self.id_to_sk.get(&id).map(|s| s.as_str())
    }

    pub fn ram_l0l1_len(&self) -> usize {
        self.ram_l0l1.len()
    }

    pub fn bq_len(&self) -> usize {
        self.bq.len()
    }

    /// Estado lógico de um doc por storage key (default `Active`).
    /// Lê o side-table `sys/state/` (Storage cru) — não afeta o NMD1.
    pub fn get_state(&mut self, sk: &str) -> MemoryState {
        match self.storage.get(&state_key(sk)) {
            Ok(Some(b)) if b.len() == 1 => MemoryState::from_u8(b[0]).unwrap_or(MemoryState::Active),
            _ => MemoryState::Active,
        }
    }

    /// Seta estado lógico (persiste em `sys/state/`). Estado é metadado de
    /// memória — a deleção FÍSICA continua sendo via `Storage::delete`.
    pub fn set_state(&mut self, sk: &str, st: MemoryState) -> Result<(), SgdbError> {
        if st == MemoryState::Active {
            // Active = default: remove o registro lateral (economia)
            self.storage.delete(&state_key(sk))?;
        } else {
            self.storage.put(&state_key(sk), &[st as u8])?;
        }
        Ok(())
    }
}

/// Put conveniência com texto.
pub fn remember_text(
    engine: &mut AiosDatabaseEngine,
    layer: MemoryLayer,
    key: &str,
    text: &str,
) -> Result<u64, SgdbError> {
    let doc = MemoryDoc::new(layer, key, text.as_bytes().to_vec());
    engine.put(doc)
}
