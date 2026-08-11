//! Sgdb — facade pública do neural-sgdb (instance-based).
//! Ponte Hermes/Cortex ↔ camadas MemoryDoc L0–L7 (port de `k_ai::sgdb::layers`).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::bq::quantize_f32;
use crate::engine::AiosDatabaseEngine;
use crate::memory_doc::{MemoryDoc, MemoryLayer, MemoryState};
use crate::storage::{Storage, SgdbError};

/// Resultado de recall semântico.
#[derive(Clone, Debug)]
pub struct Hit {
    pub key: String,
    pub text: String,
    /// distância 1−cos (0 = idêntico) em escala 0..1
    pub dist: f32,
}

/// Cognitive memory database. `Sgdb::open(backend)` + remember/recall.
pub struct Sgdb {
    engine: AiosDatabaseEngine,
}

impl Sgdb {
    /// Opens with a storage backend (`InMemory` for tests, `FileStorage`
    /// for file persistence, or your own `Storage` impl).
    /// Rebuilds ART/BQ from persisted docs — **propagates rebuild errors**
    /// (P1: an unreadable storage does not open as a silent "ready").
    pub fn open(backend: impl Storage + 'static) -> Result<Self, SgdbError> {
        Self::open_with_node_id(1, backend)
    }

    /// Idem `open`, com node_id local explícito (maturation P5 — identidade de
    /// nó estável, nunca confundida com memory_id; self/remote distinto).
    pub fn open_with_node_id(
        node_id: u8,
        backend: impl Storage + 'static,
    ) -> Result<Self, SgdbError> {
        let mut engine = AiosDatabaseEngine::new(node_id, Box::new(backend));
        let recovered = engine.rebuild_indices_from_storage()?;
        crate::sgdb_log!("Sgdb open: {recovered} docs reindexados (ART/BQ)");
        Ok(Sgdb { engine })
    }

    /// Node_id local (vector clock / origem) — estável por instância.
    pub fn node_id(&self) -> u8 {
        self.engine.node_id
    }

    /// Estado lógico de uma memória (default `Active`). Estado ≠ deleção
    /// física: `superseded`/`archived`/`invalidated` continuam representáveis
    /// na história (maturation P5).
    pub fn get_state(&mut self, key: &str) -> Result<MemoryState, SgdbError> {
        let sk = self.resolve_storage_key(key);
        Ok(self.engine.get_state(&sk))
    }

    /// Seta estado lógico de uma memória (persiste em `sys/state/`).
    pub fn set_state(&mut self, key: &str, st: MemoryState) -> Result<(), SgdbError> {
        let sk = self.resolve_storage_key(key);
        self.engine.set_state(&sk, st)
    }

    /// Conveniência cognitiva: marca `old` como `Superseded` (histórico
    /// preservado, sem `delete` físico) — a cadeia causal sobrevive para CRDT
    /// (Doc 04 §3).
    pub fn supersede(&mut self, old: &str, new: &str) -> Result<(), SgdbError> {
        let old_sk = self.resolve_storage_key(old);
        let new_sk = self.resolve_storage_key(new);
        self.engine.set_state(&old_sk, MemoryState::Superseded)?;
        self.engine.set_state(&new_sk, MemoryState::Active)
    }

    /// Resolve uma chave lógica para storage key canônica `md/Lx/...`.
    /// Aceita tanto `md/Lx/k` quanto `Lx/k` ou `k` (heurística por camada
    /// ativa é do caller; aqui só normaliza prefixo).
    fn resolve_storage_key(&self, key: &str) -> String {
        if key.starts_with("md/") || key.starts_with("sys/") {
            String::from(key)
        } else {
            alloc::format!("md/{key}")
        }
    }

    /// Recovery observável (P1): docs reindexados no último open/rebuild.
    pub fn recovered_records(&self) -> usize {
        self.engine.art.len
    }

    /// Rebuild controlado dos índices derivados (ART/BQ) a partir do storage
    /// (maturation P4c). Storage = fonte da verdade; índices = estado derivado.
    /// Útil após escrita externa no backend ou para reconciliação pós-crash.
    /// Propaga erro de scan — nunca deixa índices "meio reconstruídos" sem
    /// reportar.
    pub fn rebuild_indices(&mut self) -> Result<usize, SgdbError> {
        let n = self.engine.rebuild_indices_from_storage()?;
        crate::sgdb_log!("Sgdb rebuild: {n} docs reindexados (ART/BQ)");
        Ok(n)
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
        let cand = k * 4; // k>=1 ⇒ k*4 ≥ k, o .max(k) era redundante
        let hits = self.engine.bq_top_k_f32(query, cand);
        // Distância Hamming máxima de um vetor indexado (normaliza o fallback
        // p/ escala 0..1 do contrato de `Hit.dist` — bughunt #11).
        let ham_max = (self.engine.bq.words_per_vec.max(1) * 64) as f32;
        let mut out: Vec<(u32, Hit)> = Vec::new();
        for (id, ham) in hits {
            let Some(sk) = self.engine.storage_key_of(id).map(String::from) else {
                continue;
            };
            // score bruto u32: fp32 rescore OU hamming (mesma escala de ordenação do OS)
            let (score, dist) = match self.engine.get_by_storage_key(&sk) {
                Ok(Some(doc)) => match Self::fp32_dist_u32(query, &doc.payload) {
                    Some(d) => (d, d as f32 / 10_000.0),
                    None => (ham, (ham as f32 / ham_max).min(1.0)),
                },
                _ => (ham, (ham as f32 / ham_max).min(1.0)),
            };
            // L2 companion text (direct storage key; only the 1st occurrence
            // of the /L4/ prefix — a key containing "/L4/" is not corrupted)
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
        // Dedupe por storage key mantendo o MELHOR score: um overwrite em L4
        // re-insere no BQ (append) sem remover o id antigo — sem dedupe, o
        // recall devolveria a mesma memória 2x (ids → mesma key → mesmo doc).
        let mut best: alloc::collections::BTreeMap<String, (u32, Hit)> =
            alloc::collections::BTreeMap::new();
        for (score, h) in out {
            match best.get(&h.key) {
                Some((s0, _)) if *s0 <= score => continue, // mantém o melhor
                _ => {
                    best.insert(h.key.clone(), (score, h));
                }
            }
        }
        // Sort determinístico: score u32 (paridade OS: fp32 0..10000 vs ham
        // 0..64 no mesmo espaço) + tie-break estável por storage key — mesma
        // DB + mesma query + mesmo k ⇒ mesmos resultados ordenados.
        let mut ranked: Vec<(u32, Hit)> = best.into_values().collect();
        ranked.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.key.cmp(&b.1.key)));
        Ok(ranked.into_iter().take(k).map(|(_, h)| h).collect())
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

    // ── Index/storage consistency: overwrite + determinism (maturation P2) ──

    #[test]
    fn overwrite_no_duplicate_recall() {
        // Overwrite em L4 re-insere no BQ (append) sem remover o id antigo —
        // o recall deve deduplicar por storage key mantendo o melhor score.
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("k1", "old", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        db.remember_semantic("k1", "new", &[-1.0, 1.0, -1.0, 1.0]).unwrap();
        let hits = db.recall(&[-1.0, 1.0, -1.0, 1.0], 10).unwrap();
        // apenas UMA entrada para k1 (dedupe por key), com o melhor score
        let k1: Vec<&Hit> = hits.iter().filter(|h| h.key.contains("k1")).collect();
        assert_eq!(k1.len(), 1, "overwrite duplicou no recall: {:?}", hits);
        assert_eq!(k1[0].text, "new");
    }

    #[test]
    fn recall_deterministic() {
        // Mesma DB + mesma query + mesmo k ⇒ mesmos resultados na mesma ordem
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        for i in 0..20 {
            let emb = [1.0, -1.0, (i as f32 - 10.0) / 10.0, -1.0];
            db.remember_semantic(&format!("d{i}"), &format!("doc {i}"), &emb).unwrap();
        }
        let q = [1.0, -1.0, 0.0, -1.0];
        let a = db.recall(&q, 10).unwrap();
        let b = db.recall(&q, 10).unwrap();
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.key, y.key, "ordem não determinística");
            assert_eq!(x.dist.to_bits(), y.dist.to_bits());
        }
    }

    #[test]
    fn recall_tie_break_by_key() {
        // Scores empatados: tie-break determinístico por storage key
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        // embeddings idênticos → mesmo score; keys diferentes → ordem por key
        db.remember_semantic("z", "z-doc", &[1.0, 1.0, 1.0, 1.0]).unwrap();
        db.remember_semantic("a", "a-doc", &[1.0, 1.0, 1.0, 1.0]).unwrap();
        db.remember_semantic("m", "m-doc", &[1.0, 1.0, 1.0, 1.0]).unwrap();
        let hits = db.recall(&[1.0, 1.0, 1.0, 1.0], 10).unwrap();
        let keys: Vec<&str> = hits.iter().map(|h| h.key.as_str()).collect();
        // 3 entradas, ordenadas por key (a, m, z) no empate
        assert_eq!(keys.len(), 3);
        assert!(keys[0].ends_with("/a"), "esperava /a primeiro: {keys:?}");
        assert!(keys[1].ends_with("/m"), "esperava /m segundo: {keys:?}");
        assert!(keys[2].ends_with("/z"), "esperava /z terceiro: {keys:?}");
    }

    // ── Index rebuild: storage = verdade, índices = derivado (maturation P4c) ─

    #[cfg(feature = "file-storage")]
    #[test]
    fn rebuild_from_storage_consistency() {
        // write → close → reopen → rebuild → recall: índices reconstruídos do
        // storage devem reproduzir o mesmo recall da sessão original
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("rebuild.db");
        let _ = std::fs::remove_file(&path);
        // sessão 1: escreve
        {
            let mut db = Sgdb::open(crate::storage::FileStorage::open(&path).unwrap()).unwrap();
            db.remember_exchange("oi", "ola").unwrap();
            db.remember_fact("fato a", 1).unwrap();
            db.remember_semantic("e1", "clima bom", &[1.0, -1.0, 1.0, -1.0]).unwrap();
            db.checkpoint().unwrap();
            let expected = db.recall(&[1.0, -1.0, 1.0, -1.0], 5).unwrap();
            assert_eq!(expected.len(), 1);
            // drop(db) → "close"
        }
        // sessão 2: reopen + rebuild explícito + recall
        {
            let mut db = Sgdb::open(crate::storage::FileStorage::open(&path).unwrap()).unwrap();
            // open já rebuilda; rebuild explícito deve ser idempotente
            let n = db.rebuild_indices().unwrap();
            assert!(n >= 1, "rebuild não reindexou nada");
            let after = db.recall(&[1.0, -1.0, 1.0, -1.0], 5).unwrap();
            assert_eq!(after.len(), 1, "recall pós-rebuild vazio");
            assert_eq!(after[0].text, "clima bom");
            // ART reconstruído: fatos e exchanges acessíveis por scan_prefix
            assert!(db.scan_prefix("md/L1/").unwrap().len() >= 1);
            assert!(db.scan_prefix("md/L3/").unwrap().len() >= 1);
        }
        let _ = std::fs::remove_file(&path);
    }

    // ── Memory state model (maturation P5) ─────────────────────────────────

    #[cfg(feature = "file-storage")]
    #[test]
    fn memory_state_default_active_and_persists() {
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("state.db");
        let _ = std::fs::remove_file(&path);
        {
            let mut db = Sgdb::open(crate::storage::FileStorage::open(&path).unwrap()).unwrap();
            db.remember_fact("fato", 1).unwrap();
            let key = "md/L3/ts/0000000000000001";
            // default é Active
            assert_eq!(db.get_state(key).unwrap(), MemoryState::Active);
            // supersede: old → Superseded, new → Active (histórico preservado)
            db.supersede(key, "md/L3/ts/0000000000000002").unwrap();
            assert_eq!(db.get_state(key).unwrap(), MemoryState::Superseded);
        }
        // estado persiste no reopen (side-table sys/state/ via Storage cru)
        {
            let mut db = Sgdb::open(crate::storage::FileStorage::open(&path).unwrap()).unwrap();
            assert_eq!(
                db.get_state("md/L3/ts/0000000000000001").unwrap(),
                MemoryState::Superseded
            );
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn node_id_explicit_and_stable() {
        let mut db = Sgdb::open_with_node_id(7, InMemory::new()).unwrap();
        assert_eq!(db.node_id(), 7);
        db.remember_fact("f", 1).unwrap();
        assert_eq!(db.node_id(), 7); // estável
    }
}
