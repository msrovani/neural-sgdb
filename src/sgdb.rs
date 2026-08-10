//! Sgdb — facade pública do neural-sgdb (instance-based).
//! Ponte Hermes/Cortex ↔ camadas MemoryDoc L0–L7 (port de `k_ai::sgdb::layers`).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::bq::quantize_f32;
use crate::engine::AiosDatabaseEngine;
use crate::memory_doc::{MemoryDoc, MemoryLayer};
use crate::storage::{Storage, SgdbError};

/// Resultado de recall semântico.
#[derive(Clone, Debug)]
pub struct Hit {
    pub key: String,
    pub text: String,
    /// distância 1−cos (0 = idêntico) em escala 0..1
    pub dist: f32,
}

/// Banco de memória cognitiva. `Sgdb::open(backend)` + remember/recall.
pub struct Sgdb {
    engine: AiosDatabaseEngine,
}

impl Sgdb {
    /// Abre com um backend de storage (`InMemory` para testes, `FileStorage`
    /// para persistência em arquivo, ou seu próprio impl de `Storage`).
    /// Reconstrói ART/BQ a partir de docs persistidos — **propaga erro de
    /// rebuild** (P1: storage ilegível não abre "ready" silencioso).
    pub fn open(backend: impl Storage + 'static) -> Result<Self, SgdbError> {
        let mut engine = AiosDatabaseEngine::new(1, Box::new(backend));
        let recovered = engine.rebuild_indices_from_storage()?;
        crate::sgdb_log!("Sgdb open: {recovered} docs reindexados (ART/BQ)");
        Ok(Sgdb { engine })
    }

    /// Recovery observável (P1): docs reindexados no último open/rebuild.
    pub fn recovered_records(&self) -> usize {
        self.engine.art.len
    }

    /// Pós-turno: L1 working (user) + L2 episódico curto (assistant).
    pub fn remember_exchange(&mut self, user: &str, response: &str) -> Result<(), SgdbError> {
        let u = MemoryDoc::new(
            MemoryLayer::L1Working,
            "last_user",
            user.as_bytes().to_vec(),
        );
        let _ = self.engine.put(u)?;
        let a = MemoryDoc::new(
            MemoryLayer::L2EpisodicShort,
            "last_asst",
            response.as_bytes().to_vec(),
        );
        let _ = self.engine.put(a)?;
        Ok(())
    }

    /// Pós-turno completo: texto L1/L2 constantes + L2 timestamped + L4 BQ temporal.
    pub fn remember_exchange_full(
        &mut self,
        user: &str,
        response: &str,
        emb_u: &[f32],
        emb_a: &[f32],
        now: u64,
    ) -> Result<(), SgdbError> {
        let ts = MemoryDoc::sortable_ts_key(now);
        let ts_u = format!("{}/u", ts);
        let ts_a = format!("{}/a", ts);

        // L1/L2 constant keys (prompt_slice compat)
        let u = MemoryDoc::new(MemoryLayer::L1Working, "last_user", user.as_bytes().to_vec());
        let _ = self.engine.put(u)?;
        let a = MemoryDoc::new(
            MemoryLayer::L2EpisodicShort,
            "last_asst",
            response.as_bytes().to_vec(),
        );
        let _ = self.engine.put(a)?;

        // L2 timestamped text (acumula para recall RAG)
        let _ = crate::engine::remember_text(
            &mut self.engine,
            MemoryLayer::L2EpisodicShort,
            &ts_u,
            user,
        )?;
        let _ = crate::engine::remember_text(
            &mut self.engine,
            MemoryLayer::L2EpisodicShort,
            &ts_a,
            response,
        )?;

        // L4 timestamped embeddings
        self.remember_semantic(&ts_u, user, emb_u)?;
        self.remember_semantic(&ts_a, response, emb_a)?;
        Ok(())
    }

    /// Indexa embedding L4 (BQ). `emb` vazio = no-op. O texto é armazenado em
    /// L2 (companion `md/L2/<key>`) para recall trazer texto legível.
    pub fn remember_semantic(
        &mut self,
        key: &str,
        text: &str,
        emb: &[f32],
    ) -> Result<(), SgdbError> {
        if emb.is_empty() {
            return Ok(());
        }
        let mut payload = Vec::with_capacity(emb.len() * 4);
        for x in emb {
            payload.extend_from_slice(&x.to_le_bytes());
        }
        let mut doc = MemoryDoc::new(MemoryLayer::L4Semantic, key, payload);
        doc.bitvec = Some(quantize_f32(emb));
        let _ = self.engine.put(doc)?;
        // companion texto (para Hit.text / rag_context)
        let tdoc = MemoryDoc::new(MemoryLayer::L2EpisodicShort, key, text.as_bytes().to_vec());
        let _ = self.engine.put(tdoc)?;
        Ok(())
    }

    /// Distância 1−cos em escala u32 (0 = idêntico). Sem floats no payload → None.
    fn fp32_dist_u32(query: &[f32], payload: &[u8]) -> Option<u32> {
        let n = payload.len() / 4;
        let mut doc = Vec::with_capacity(n);
        for i in 0..n {
            let o = i * 4;
            doc.push(f32::from_le_bytes([
                payload[o],
                payload[o + 1],
                payload[o + 2],
                payload[o + 3],
            ]));
        }
        if doc.is_empty() || query.is_empty() {
            return None;
        }
        let n = query.len().min(doc.len());
        let mut dot = 0.0f32;
        let mut nq = 0.0f32;
        let mut nd = 0.0f32;
        for i in 0..n {
            dot += query[i] * doc[i];
            nq += query[i] * query[i];
            nd += doc[i] * doc[i];
        }
        // ponytail: f32::sqrt não existe no core (x86_64-unknown-none);
        // Newton's method, zero deps (kernel usa libm::sqrtf).
        let denom = sqrt_f32(nq * nd) + 1e-8;
        let cos = (dot / denom).clamp(-1.0, 1.0);
        let dist = 1.0 - cos;
        Some((dist * 10_000.0) as u32)
    }

    /// Recall L4: BQ top-k, depois rescore FP32 nos candidatos (padrão Qdrant).
    /// Sort pelo score bruto u32 (paridade com o OS: fp32 ∈ 0..10000 vs ham ∈
    /// 0..64 convivem no mesmo espaço de ordenação — layers.rs:108-118).
    pub fn recall(&mut self, query: &[f32], k: usize) -> Result<Vec<Hit>, SgdbError> {
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let k = k.max(1);
        let cand = (k * 4).max(k);
        let hits = self.engine.bq_top_k_f32(query, cand);
        let mut out: Vec<(u32, Hit)> = Vec::new();
        for (id, ham) in hits {
            let Some(sk) = self.engine.storage_key_of(id).map(String::from) else {
                continue;
            };
            // score bruto u32: fp32 rescore OU hamming (mesma escala de ordenação do OS)
            let (score, dist) = match self.engine.get_by_storage_key(&sk) {
                Ok(Some(doc)) => match Self::fp32_dist_u32(query, &doc.payload) {
                    Some(d) => (d, d as f32 / 10_000.0),
                    None => (ham, ham as f32),
                },
                _ => (ham, ham as f32),
            };
            // texto companion L2 (storage key direta; só a 1ª ocorrência do
            // prefixo /L4/ — chave com "/L4/" no corpo não é corrompida)
            let text = self
                .engine
                .get_by_storage_key(&sk.replacen("/L4/", "/L2/", 1))
                .ok()
                .flatten()
                .map(|d| String::from_utf8_lossy(&d.payload).into_owned())
                .unwrap_or_default();
            out.push((
                score,
                Hit {
                    key: sk,
                    text,
                    dist,
                },
            ));
        }
        out.sort_by_key(|(score, _)| *score);
        Ok(out.into_iter().take(k).map(|(_, h)| h).collect())
    }

    /// RAG context: recall + fetch payload + formato string pro prompt.
    pub fn rag_context(&mut self, query: &[f32], k: usize) -> Result<String, SgdbError> {
        let hits = self.recall(query, k)?;
        if hits.is_empty() {
            return Ok(String::new());
        }
        let mut out = format!("[SGDB-RAG top-{}]\n", hits.len());
        for (i, h) in hits.iter().enumerate() {
            if h.text.is_empty() {
                continue;
            }
            out.push_str(&format!("  #{}) d={:.4} {}\n", i + 1, h.dist, clamp(&h.text, 200)));
        }
        if out.len() <= "[SGDB-RAG top-".len() {
            return Ok(String::new());
        }
        Ok(out)
    }

    /// Fato L3 (ART) com timestamp explícito (`now` — clock do caller).
    pub fn remember_fact(&mut self, fact: &str, now: u64) -> Result<(), SgdbError> {
        let key = MemoryDoc::sortable_ts_key(now);
        let doc = MemoryDoc::new(
            MemoryLayer::L3EpisodicLong,
            &key,
            fact.as_bytes().to_vec(),
        );
        let _ = self.engine.put(doc)?;
        Ok(())
    }

    /// Load por camada + chave (ex: `db.get(MemoryLayer::L1Working, "last_user")`).
    pub fn get(
        &mut self,
        layer: MemoryLayer,
        key: &str,
    ) -> Result<Option<MemoryDoc>, SgdbError> {
        self.engine.get(layer, key)
    }

    /// Lookup ART por prefixo de storage key (ex: "md/L1/").
    pub fn scan_prefix(&mut self, prefix: &str) -> Result<Vec<(String, u64)>, SgdbError> {
        Ok(self.engine.art.scan_prefix(prefix))
    }

    /// Flush L0/L1 RAM → Storage.
    pub fn checkpoint(&mut self) -> Result<usize, SgdbError> {
        self.engine.checkpoint_l0l1()
    }

    /// Drop arena RAM L0/L1 (pós-checkpoint).
    pub fn prune_working_ram(&mut self) -> Result<usize, SgdbError> {
        Ok(self.engine.prune_ram_l0l1())
    }

    pub fn backend(&self) -> &'static str {
        self.engine.backend_name()
    }

    pub fn ready(&self) -> bool {
        true // engine sempre tem storage
    }

    pub fn bq_len(&self) -> usize {
        self.engine.bq_len()
    }

    pub fn ram_len(&self) -> usize {
        self.engine.ram_l0l1_len()
    }
}

fn clamp(s: &str, max: usize) -> String {
    if s.len() <= max {
        String::from(s)
    } else {
        // corte em fronteira de char (evita panic em byte-200 no meio de
        // caractere multi-byte — bughunt #7)
        let cut = s
            .char_indices()
            .take_while(|(i, _)| *i <= max)
            .map(|(i, _)| i)
            .last()
            .unwrap_or(max.min(s.len()));
        let mut t = String::from(&s[..cut]);
        t.push('…');
        t
    }
}

/// sqrt para no_std (core não expõe `f32::sqrt` no target bare-metal).
/// Newton–Raphson, 10 iterações, convergência rápida para argumentos > 0.
fn sqrt_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut y = x;
    for _ in 0..10 {
        y = (y + x / y) * 0.5;
    }
    y
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::InMemory;

    #[test]
    fn exchange_and_prompt() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_exchange("oi", "ola!").unwrap();
        let l1 = db.scan_prefix("md/L1/").unwrap();
        let l2 = db.scan_prefix("md/L2/").unwrap();
        assert!(l1.len() >= 1);
        assert!(l2.len() >= 1);
    }

    #[test]
    fn semantic_recall_roundtrip() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("d1", "clima ensolarado", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        db.remember_semantic("d2", "tudo frio", &[-1.0, -1.0, -1.0, -1.0])
            .unwrap();
        let hits = db.recall(&[1.0, -1.0, 1.0, -1.0], 2).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].dist, 0.0); // idêntico
        assert_eq!(hits[0].text, "clima ensolarado");
        let ctx = db.rag_context(&[1.0, -1.0, 1.0, -1.0], 1).unwrap();
        assert!(ctx.contains("clima ensolarado"));
    }

    #[test]
    fn fact_timestamped() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_fact("prefere dark mode", 100).unwrap();
        let l3 = db.scan_prefix("md/L3/").unwrap();
        assert!(l3.len() >= 1);
    }

    #[test]
    fn checkpoint_preserves_l1() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_exchange("u", "r").unwrap();
        assert_eq!(db.ram_len(), 1); // L1 em RAM
        db.checkpoint().unwrap();
        let n = db.prune_working_ram().unwrap();
        assert!(n >= 1);
        assert_eq!(db.ram_len(), 0);
        // L1 sobrevive via Storage
        let l1 = db.scan_prefix("md/L1/").unwrap();
        assert!(l1.len() >= 1);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn file_backend_full_flow() {
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sgdb_flow.db");
        let _ = std::fs::remove_file(&path);
        {
            let mut db = Sgdb::open(crate::storage::FileStorage::open(&path).unwrap()).unwrap();
            db.remember_exchange("qual o clima?", "sol, 24 graus").unwrap();
            db.remember_semantic("turno:1", "clima ensolarado em sao paulo", &[1.0, -1.0, 1.0, -1.0])
                .unwrap();
            db.remember_fact("user gosta de cafe", 42).unwrap();
            db.checkpoint().unwrap();
            let hits = db.recall(&[1.0, -1.0, 1.0, -1.0], 1).unwrap();
            assert_eq!(hits.len(), 1);
        }
        // Reopen: índices reconstruídos do storage
        let mut db = Sgdb::open(crate::storage::FileStorage::open(&path).unwrap()).unwrap();
        let l1 = db.scan_prefix("md/L1/").unwrap();
        assert!(l1.len() >= 1);
        let l3 = db.scan_prefix("md/L3/").unwrap();
        assert!(l3.len() >= 1);
        let hits = db.recall(&[1.0, -1.0, 1.0, -1.0], 1).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "clima ensolarado em sao paulo");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn scan_prefix_art() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_exchange("a", "b").unwrap();
        db.remember_fact("f1", 1).unwrap();
        let l1 = db.scan_prefix("md/L1/").unwrap();
        let l2 = db.scan_prefix("md/L2/").unwrap();
        let l3 = db.scan_prefix("md/L3/").unwrap();
        assert_eq!(l1.len(), 1);
        assert_eq!(l2.len(), 1);
        assert_eq!(l3.len(), 1);
    }
}
