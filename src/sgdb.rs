//! Sgdb — facade pública do neural-sgdb (instance-based).
//! Ponte Hermes/Cortex ↔ camadas MemoryDoc L0–L7 (port de `k_ai::sgdb::layers`).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::bq::{quantize_f32, BqFlatIndex};
use crate::engine::AiosDatabaseEngine;
use crate::memory_doc::{MemoryDoc, MemoryLayer, MemoryMeta, MemoryRecord, MemoryState};
use crate::storage::{Storage, SgdbError};

/// Proveniência de um hit (v0.6 — Phase 9 parcial): epistemologia exposta ao
/// caller — memórias com estados diferentes NÃO se parecem iguais no recall.
#[derive(Clone, Debug, PartialEq)]
pub struct HitProvenance {
    pub memory_id: String,
    pub layer: MemoryLayer,
    pub state: MemoryState,
    pub source: u8,
    pub confidence: f32,
    pub importance: f32,
    pub created_tick: u64,
    pub parent_ids: Vec<String>,
}

/// Resultado de recall semântico.
#[derive(Clone, Debug)]
pub struct Hit {
    pub key: String,
    pub text: String,
    /// distância 1−cos (0 = idêntico) em escala 0..1
    pub dist: f32,
    /// Proveniência (v0.6) — `None` para hits sem metadados (registros
    /// pré-v0.6 ainda não re-escritos) e para recalls puramente lexicais
    /// quando o doc não tem meta.
    pub provenance: Option<HitProvenance>,
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
    /// (Doc 04 §3). v0.6: registra a lineage em `new.parent_ids` (DAG causal
    /// começa aqui — Phase 3 incrementa).
    pub fn supersede(&mut self, old: &str, new: &str) -> Result<(), SgdbError> {
        let old_sk = self.resolve_storage_key(old);
        let new_sk = self.resolve_storage_key(new);
        self.engine.set_state(&old_sk, MemoryState::Superseded)?;
        self.engine.set_state(&new_sk, MemoryState::Active)?;
        if let Some(old_id) = self.engine.meta(&old_sk)?.map(|m| m.memory_id) {
            if let Some(mut nm) = self.engine.meta(&new_sk)? {
                if !nm.parent_ids.contains(&old_id) {
                    nm.parent_ids.push(old_id);
                }
                self.engine.write_meta(&new_sk, &nm)?;
            }
        }
        Ok(())
    }

    /// Identidade estável da memória (v0.6). `None` = sem doc na chave ou
    /// registro pré-v0.6 ainda sem meta (re-put/`ensure_meta` atribui).
    pub fn memory_id(&mut self, key: &str) -> Result<Option<String>, SgdbError> {
        let sk = self.resolve_storage_key(key);
        Ok(self.engine.meta(&sk)?.map(|m| m.memory_id))
    }

    /// Metadados completos (memory_id, source, confidence, importance,
    /// created_tick, parent_ids, clock_overflow).
    pub fn meta(&mut self, key: &str) -> Result<Option<MemoryMeta>, SgdbError> {
        let sk = self.resolve_storage_key(key);
        self.engine.meta(&sk)
    }

    /// Importância [0..1] (normalizada; fora do intervalo é clampada, não-
    /// finita é rejeitada). Persistente em `sys/meta/`; usada pelo lifecycle
    /// (fase posterior) e exposta no recall via `Hit.provenance`.
    pub fn set_importance(&mut self, key: &str, importance: f32) -> Result<(), SgdbError> {
        if !importance.is_finite() {
            return Err(SgdbError::Invalid("importance must be finite"));
        }
        let sk = self.resolve_storage_key(key);
        let mut m = self.engine.ensure_meta(&sk)?;
        m.importance = importance.clamp(0.0, 1.0);
        self.engine.write_meta(&sk, &m)
    }

    /// Confiança [0..1] (mesmo contrato da importância).
    pub fn set_confidence(&mut self, key: &str, confidence: f32) -> Result<(), SgdbError> {
        if !confidence.is_finite() {
            return Err(SgdbError::Invalid("confidence must be finite"));
        }
        let sk = self.resolve_storage_key(key);
        let mut m = self.engine.ensure_meta(&sk)?;
        m.confidence = confidence.clamp(0.0, 1.0);
        self.engine.write_meta(&sk, &m)
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
    /// Oversample é AUTO por dimensionalidade: BQ degrada com poucos words
    /// (#5, Qdrant: abaixo de ~768 dims o 1-bit colide) — poucos words ⇒
    /// pool maior. Para controle explícito use `recall_oversampled`.
    pub fn recall(&mut self, query: &[f32], k: usize) -> Result<Vec<Hit>, SgdbError> {
        let words = self.engine.bq.words_per_vec;
        let ov = match words {
            0 | 1 => 16,
            2..=4 => 8,
            _ => 4,
        };
        self.recall_oversampled(query, k, ov)
    }

    /// Recall com **oversampling** configurável (pesquisa upstream Qdrant/BQ):
    /// busca `oversample*k` candidatos Hamming no filtro grosseiro BQ e rescora
    /// FP32 — ~0.98–0.99 de recall com 2–4x oversample (vs `k*4` fixo). Com
    /// dims baixas (ex: 16) o filtro BQ colide em bits e o match exato escapa
    /// do top-k pequeno; aumentar o oversample recupera sem mudar o formato.
    /// `oversample >= 1`; `recall()` delega com oversample=4 (compatível).
    ///
    /// Estágios explícitos do pipeline (item 15/16):
    /// 1. **Candidate generation** — BQ binary (hamming SIMD) sobre os bitvecs
    ///    → pool de `oversample·k` candidatos. BQ é mecanismo de geração de
    ///    candidatos, NÃO representação semântica completa (item 4).
    /// 2. **Filtragem** — candidatos sem doc vivo (deletado/corrompido) são
    ///    pulados: índice derivado nunca ressuscita memória (item 17).
    /// 3. **Reranking** — cosseno FP32 sobre os f32 ORIGINAIS (payload NMD1);
    ///    sem payload f32, fallback = hamming normalizado (0..1).
    /// 4. **Finalização** — dedupe por storage key (overwrite), sort
    ///    determinístico por (score u32, storage key) → top-k.
    /// Filtros futuros (camada, metadado, temporal, proveniência) entram entre
    /// 2 e 3 sem mudar o contrato público.
    pub fn recall_oversampled(
        &mut self,
        query: &[f32],
        k: usize,
        oversample: usize,
    ) -> Result<Vec<Hit>, SgdbError> {
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let k = k.max(1);
        let cand = k.saturating_mul(oversample.max(1));
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
            let (score, dist, provenance) = match self.engine.get_by_storage_key(&sk) {
                Ok(Some(doc)) => {
                    let (score, dist) = match Self::fp32_dist_u32(query, &doc.payload) {
                        Some(d) => (d, d as f32 / 10_000.0),
                        None => (ham, (ham as f32 / ham_max).min(1.0)),
                    };
                    // v0.6 — provenance exposta (Phase 9 parcial): quem/quando/
                    // quão confiável/estado — memórias superseded não se fingem
                    // de ativas no resultado.
                    let prov = doc.meta.as_ref().map(|m| HitProvenance {
                        memory_id: m.memory_id.clone(),
                        layer: doc.layer,
                        state: self.engine.get_state(&sk),
                        source: m.source,
                        confidence: m.confidence,
                        importance: m.importance,
                        created_tick: m.created_tick,
                        parent_ids: m.parent_ids.clone(),
                    });
                    (score, dist, prov)
                }
                // doc sumiu (delete físico) ou corrompeu: o BQ é índice
                // DERIVADO — candidato sem doc vivo NUNCA vira hit (não
                // ressuscita memória deletada nem mostra lixo, item 17/21)
                Ok(None) | Err(_) => continue,
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
                    provenance,
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

    /// Recall com **scoring ponderado** (#3, padrão Mem0/MemGPT):
    /// `score = w_sem·dist + w_rec·recency_penalty + w_imp·importance_penalty`
    /// (menor = melhor). Recency vem do timestamp no storage key (`/ts/<hex>`);
    /// importance da camada (`md/LX/`). Busca um pool maior (`k·16`) para que
    /// recência/importância possam puxar candidatos fora do top-k semântico.
    pub fn recall_weighted(
        &mut self,
        query: &[f32],
        k: usize,
        w_sem: f32,
        w_rec: f32,
        w_imp: f32,
        now: u64,
    ) -> Result<Vec<Hit>, SgdbError> {
        let pool = self.recall_oversampled(query, k.max(1).saturating_mul(16), 1)?;
        let mut scored: Vec<(f64, Hit)> = Vec::with_capacity(pool.len());
        for h in pool {
            let rec = match ts_from_key(&h.key) {
                Some(t) => (now.saturating_sub(t) as f64 / 1000.0).clamp(0.0, 1.0),
                None => 0.5, // sem timestamp: neutro
            };
            let s = w_sem as f64 * h.dist as f64
                + w_rec as f64 * rec
                + w_imp as f64 * layer_importance(&h.key);
            scored.push((s, h));
        }
        scored.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(core::cmp::Ordering::Equal)
                .then_with(|| a.1.key.cmp(&b.1.key))
        });
        Ok(scored.into_iter().take(k).map(|(_, h)| h).collect())
    }

    /// Janela de validade (#9, Zep/Graphiti pattern): `from ≤ now < until`.
    /// `key` = storage key canônica (`md/Lx/...`). Side-table `sys/validity/`;
    /// o doc NUNCA é deletado — só marcado.
    pub fn set_validity(&mut self, key: &str, from: u64, until: u64) -> Result<(), SgdbError> {
        let sk = self.resolve_storage_key(key);
        self.engine.set_validity(&sk, from, until)
    }

    pub fn validity_at(&mut self, key: &str, now: u64) -> Result<bool, SgdbError> {
        let sk = self.resolve_storage_key(key);
        Ok(self.engine.validity_at(&sk, now))
    }

    pub fn invalidate(&mut self, key: &str, now: u64) -> Result<(), SgdbError> {
        let sk = self.resolve_storage_key(key);
        self.engine.invalidate(&sk, now)
    }

    /// Recall **lexical contextual** (#7, BM25-style sobre o índice invertido
    /// dos textos L2/L3): recupera casamentos de termos que o BQ perde.
    /// `dist` = 1 − score normalizado (0 = melhor hit lexical).
    pub fn recall_lexical(&mut self, query_text: &str, k: usize) -> Result<Vec<Hit>, SgdbError> {
        let scored = self.engine.lexical.search(query_text, k.max(1));
        let max = scored.first().map(|(_, s)| *s).unwrap_or(0.0).max(1e-6);
        let mut out = Vec::with_capacity(scored.len());
        for (sk, score) in scored {
            if let Ok(Some(doc)) = self.engine.get_by_storage_key(&sk) {
                let provenance = doc.meta.as_ref().map(|m| HitProvenance {
                    memory_id: m.memory_id.clone(),
                    layer: doc.layer,
                    state: self.engine.get_state(&sk),
                    source: m.source,
                    confidence: m.confidence,
                    importance: m.importance,
                    created_tick: m.created_tick,
                    parent_ids: m.parent_ids.clone(),
                });
                out.push(Hit {
                    key: sk,
                    text: String::from_utf8_lossy(&doc.payload).into_owned(),
                    dist: (1.0 - score / max).clamp(0.0, 1.0),
                    provenance,
                });
            }
        }
        Ok(out)
    }

    /// Recall **híbrido** semântico + lexical (Anthropic dual-path): semantic
    /// primeiro, depois os lexicais não duplicados — até `k`.
    pub fn recall_hybrid(
        &mut self,
        query_emb: &[f32],
        query_text: &str,
        k: usize,
    ) -> Result<Vec<Hit>, SgdbError> {
        let mut out = self.recall(query_emb, k)?;
        let mut seen: alloc::collections::BTreeSet<String> =
            out.iter().map(|h| h.key.clone()).collect();
        for h in self.recall_lexical(query_text, k.max(1).saturating_mul(4))? {
            if seen.insert(h.key.clone()) {
                out.push(h);
            }
        }
        out.truncate(k.max(1));
        Ok(out)
    }

    /// Recall que filtra memórias **invalidadas** em `now` (default:
    /// `recall` ignora validade). Recall-time only — não toca nos bitvecs.
    pub fn recall_at(&mut self, query: &[f32], k: usize, now: u64) -> Result<Vec<Hit>, SgdbError> {
        let hits = self.recall(query, k)?;
        Ok(hits
            .into_iter()
            .filter(|h| self.engine.validity_at(&h.key, now))
            .collect())
    }

    /// Conveniência RAG com oversample explícito.
    pub fn rag_context_oversampled(
        &mut self,
        query: &[f32],
        k: usize,
        oversample: usize,
    ) -> Result<String, SgdbError> {
        let hits = self.recall_oversampled(query, k, oversample)?;
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

    /// Importa/restaura um `MemoryDoc` completo (replicação p2p, restore,
    /// migração). Indexa nos índices derivados (ART/BQ/lexical) como qualquer
    /// `remember_*` — útil para o pull de memórias do peer no sync.
    pub fn put(&mut self, doc: MemoryDoc) -> Result<u64, SgdbError> {
        self.engine.put(doc)
    }

    /// Exporta uma memória como UNIDADE de replicação (P0-5): doc NMD1 (com
    /// meta anexada) + estado lógico + janela de validade. `None` = sem doc
    /// na chave. O lado remoto reimporta com `import_record`/`merge_remote`.
    ///
    /// Fecha a contradição #2: estado/validade são side-tables (`sys/state/`,
    /// `sys/validity/`) que o antigo diff/pull doc-a-doc descartava — agora
    /// viajam com o doc.
    pub fn export_record(&mut self, key: &str) -> Result<Option<MemoryRecord>, SgdbError> {
        let sk = self.resolve_storage_key(key);
        self.engine.export_record(&sk)
    }

    /// Importa uma `MemoryRecord` replicada (P0-5): grava o NMD1 **sem tick
    /// do relógio local** (o receptor não vira autor de memória alheia),
    /// preserva a identidade do criador (`meta`) e aplica estado + validade
    /// que viajam no record. Indexa ART/BQ/lexical como qualquer put.
    pub fn import_record(&mut self, rec: MemoryRecord) -> Result<u64, SgdbError> {
        self.engine.import_record(rec)
    }

    /// Merge de um record REMOTO sob a política da camada (P0-6). Veredicto:
    ///
    /// - `Rejected` — camada não aceita remoto (L0/L1 local-only, L6).
    /// - `Applied` — não existe local, ou o remoto domina causalmente (importa
    ///   com lineage em `parent_ids`), ou o side-metadata avançou (estado/
    ///   validade reaplicados sem tocar no NMD1).
    /// - `Stale` — o local domina causalmente: sem regressão, nada escrito.
    /// - `Duplicate` — mesmo conteúdo causal E mesmo estado/validade.
    /// - `Conflict` — clocks CONCORRENTES: NUNCA LWW cego — o local é
    ///   preservado, o conflito é reportado (camada superior resolve).
    ///
    /// O CRDT detecta/preserva; a camada cognitiva interpreta/decide
    /// (item 20/21 — nenhuma decisão semântica aqui).
    #[cfg(feature = "p2p")]
    pub fn merge_remote(&mut self, rec: MemoryRecord) -> Result<crate::crdt::MergeVerdict, SgdbError> {
        use crate::crdt::{MergePolicy, MergeVerdict};
        let policy = MergePolicy::for_layer(rec.doc.layer);
        if !policy.accepts_remote() {
            return Ok(MergeVerdict::Rejected);
        }
        let sk = rec.doc.storage_key();
        let Some(local) = self.engine.get_by_storage_key(&sk)? else {
            // sem local: adoção limpa (bootstrap / nó novo)
            self.engine.import_record(rec)?;
            return Ok(MergeVerdict::Applied);
        };
        let same_content = rec.doc.clock == local.clock
            && rec.doc.payload == local.payload
            && rec.doc.bitvec == local.bitvec;
        if same_content {
            // side-metadata evolui SEM tocar o NMD1 (supersede/invalidate):
            // conteúdo causal igual + estado/validade iguais → duplicata;
            // senão reimporta SÓ para propagar o side-metadata novo.
            let same_state = self.engine.get_state(&sk) == rec.state;
            let same_validity = self.engine.validity_window(&sk) == rec.validity;
            if same_state && same_validity {
                return Ok(MergeVerdict::Duplicate);
            }
            self.engine.import_record(rec)?;
            return Ok(MergeVerdict::Applied);
        }
        if rec.doc.clock.happens_before(&local.clock) {
            return Ok(MergeVerdict::Stale); // local domina — sem regressão
        }
        if local.clock.happens_before(&rec.doc.clock) {
            // remoto domina causalmente → importa: o conteúdo do slot é
            // atualizado, a IDENTIDADE permanece (v0.6: overwrite = mesma
            // memória). A lineage entre CHAVES distintas é registrada por
            // `supersede(old, new)` — aqui seria auto-parentesco (self-parent).
            self.engine.import_record(rec)?;
            return Ok(MergeVerdict::Applied);
        }
        // CONCORRENTE: nunca descartar memória — preserva o local e expõe o
        // conflito (L2/L3 multi-value: ambas retidas em seus nós de origem)
        Ok(MergeVerdict::Conflict)
    }

    /// Deleção **física** (tombstone + remoção dos índices derivados).
    ///
    /// Diferente do estado lógico (`set_state`/`supersede`/`invalidate`), que
    /// PRESERVA o doc na história (invalidar-não-deletar), aqui o doc some do
    /// storage, das side-tables (`sys/state/`, `sys/validity/`) e dos índices
    /// (ART, lexical, id→sk). Candidatos BQ órfãos são pulados no recall — o
    /// índice é derivado, não fonte da verdade (item 17/18).
    ///
    /// `key` = storage key canônica (`md/Lx/...`) ou `Lx/k`.
    /// Retorna `true` se o doc existia. Idempotente (segunda chamada = `false`).
    ///
    /// Nota: `remember_semantic` cria um PAR (L4 embedding + L2 companion
    /// text) — `delete` remove exatamente a chave resolvida; delete ambos para
    /// remoção completa da memória.
    pub fn delete(&mut self, key: &str) -> Result<bool, SgdbError> {
        let sk = self.resolve_storage_key(key);
        self.engine.delete(&sk)
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

    /// Acesso somente-leitura ao índice BQ (para `MihIndex::build`, estudo ou
    /// instrumentação). O índice é derivado do storage — não mutar.
    pub fn bq(&self) -> &BqFlatIndex {
        &self.engine.bq
    }

    pub fn ram_len(&self) -> usize {
        self.engine.ram_l0l1_len()
    }
}

/// Extrai o timestamp de um storage key `…/ts/<hex>` (facts L3 e semantic
/// timestamped de `remember_exchange_full`). `None` se a chave não é `ts/`.
fn ts_from_key(key: &str) -> Option<u64> {
    let hex = if let Some(p) = key.find("/ts/") {
        &key[p + 4..]
    } else if let Some(rest) = key.strip_prefix("ts/") {
        rest
    } else {
        return None;
    };
    let hex = hex.split('/').next()?;
    u64::from_str_radix(hex, 16).ok()
}

/// Penalty de importância por camada (0 = mais importante): L4/L7 alta,
/// L5/L3 média, L1/L2 baixa. Chaves fora de `md/LX/` → neutro (0.5).
fn layer_importance(key: &str) -> f64 {
    let b = key.as_bytes();
    let Some(i) = b.windows(4).position(|w| w == b"/L") else {
        return 0.5;
    };
    if i + 3 >= b.len() {
        return 0.5;
    }
    match b[i + 3].wrapping_sub(b'0') {
        4 | 7 => 0.0,
        5 => 0.2,
        3 => 0.6,
        2 => 0.8,
        _ => 1.0,
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

    use alloc::vec; // no_std test build: `vec!` não está no prelude

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
    fn recall_oversample_recovers_exact_on_low_dims() {
        // Com dims baixas (16) o filtro BQ colide em bits e o match exato pode
        // escapar do top-k pequeno (visto no stress: exact@1 ≈ 42% com 100k
        // docs). Oversampling maior amplia o pool de candidatos Hamming antes
        // do rescore FP32 — recupera o exato sem mudar o formato dos bitvecs.
        let emb16 = |seed: u64| -> Vec<f32> {
            let mut s = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let mut v = Vec::with_capacity(16);
            for _ in 0..16 {
                s = s.wrapping_mul(1103515245).wrapping_add(12345);
                v.push(((s >> 32) as i32 % 200) as f32 / 100.0 - 1.0);
            }
            v
        };
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        for i in 0..2000 {
            db.remember_semantic(&format!("d{i}"), &format!("doc {i}"), &emb16(i as u64))
                .unwrap();
        }
        let mut exact_small = 0usize;
        let mut exact_large = 0usize;
        let mut exact_auto = 0usize;
        for i in (0..2000).step_by(7) {
            let v = emb16(i as u64);
            if db.recall_oversampled(&v, 1, 4).unwrap().first().map(|h| h.dist == 0.0).unwrap_or(false) {
                exact_small += 1;
            }
            if db.recall_oversampled(&v, 1, 64).unwrap().first().map(|h| h.dist == 0.0).unwrap_or(false) {
                exact_large += 1;
            }
            // #5 auto-oversample: 16-dim = 1 word ⇒ ov=16 automático no recall()
            if db.recall(&v, 1).unwrap().first().map(|h| h.dist == 0.0).unwrap_or(false) {
                exact_auto += 1;
            }
        }
        // oversample=4 é lossy em dims baixas; oversample=64 recupera; o
        // recall() com auto-oversample (ov=16 em 1 word) também deve recuperar
        assert!(
            exact_large > exact_small,
            "oversample deveria melhorar exact: small={exact_small} large={exact_large}"
        );
        assert_eq!(exact_large, 286, "com 64x o match exato deve sempre vencer");
        // auto-oversample (1 word → ov=16) recupera ~tudo; só 1 query tem um
        // grupo de colisão bit-idêntico >16 (patológico, 2000 docs / 2^16)
        assert!(
            exact_auto >= 280 && exact_auto > exact_small,
            "auto-oversample deveria recuperar quase tudo: small={exact_small} auto={exact_auto}"
        );
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
    fn recall_weighted_prefers_recent_and_important() {
        use crate::memory_doc::MemoryDoc;
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let emb = vec![1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
        // duas memórias com MESMO embedding (empate semântico), timestamps
        // diferentes — keys ts/<hex> (padrão remember_exchange_full)
        let old = MemoryDoc::sortable_ts_key(100);
        let new = MemoryDoc::sortable_ts_key(900);
        db.remember_semantic(&old, "memoria antiga", &emb).unwrap();
        db.remember_semantic(&new, "memoria recente", &emb).unwrap();

        // w_rec alto: a recente (ts=900, mais perto de now=950) vem primeiro
        let r = db.recall_weighted(&emb, 2, 0.0, 1.0, 0.0, 950).unwrap();
        assert_eq!(r.len(), 2);
        assert!(r[0].text == "memoria recente", "recência deveria puxar a recente: {}", r[0].text);

        // w_rec = 0: empate semântico → tie-break por key (antiga primeiro)
        let r0 = db.recall_weighted(&emb, 2, 1.0, 0.0, 0.0, 950).unwrap();
        assert!(r0[0].text == "memoria antiga", "sem recência: key menor primeiro: {}", r0[0].text);

        // importância: L4 (semântica, penalty 0.0) deve vencer L5 (procedural,
        // penalty 0.2) com o MESMO embedding sob w_imp alto
        let mut l5 = MemoryDoc::new(crate::memory_doc::MemoryLayer::L5Procedural, "proc/1", vec![1, 2, 3, 4]);
        l5.bitvec = Some(crate::bq::quantize_f32(&emb));
        db.engine.put(l5).unwrap();
        let ri = db.recall_weighted(&emb, 1, 0.0, 0.0, 1.0, 0).unwrap();
        assert!(
            !ri[0].key.contains("proc/1"),
            "L4 (semântica) deveria vencer L5 por importância: {}",
            ri[0].key
        );
        // sem w_imp, os três empatam semanticamente → L5 pode aparecer no top
        let r0 = db.recall_weighted(&emb, 3, 1.0, 0.0, 0.0, 950).unwrap();
        assert!(r0.iter().any(|h| h.key.contains("proc/1")));
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn fact_validity_invalidate_not_delete() {
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sgdb_validity.db");
        let _ = std::fs::remove_file(&path);
        let key = "md/L3/ts/0000000000000064";
        {
            let mut db = Sgdb::open(crate::storage::FileStorage::open(&path).unwrap()).unwrap();
            db.remember_fact("fato antigo superado", 100).unwrap();
            // default: sempre válido
            assert!(db.validity_at(key, 50).unwrap());
            // janela explícita
            db.set_validity(key, 0, 1000).unwrap();
            assert!(db.validity_at(key, 500).unwrap());
            assert!(!db.validity_at(key, 1500).unwrap());
            // invalidar-não-deletar: doc permanece no storage
            db.invalidate(key, 600).unwrap();
            assert!(!db.validity_at(key, 600).unwrap());
            assert!(db.scan_prefix("md/L3/").unwrap().len() >= 1);

            // recall_at filtra inválidos; recall não
            let emb = [1.0, -1.0, 1.0, -1.0];
            db.remember_semantic("d1", "doc um", &emb).unwrap();
            db.remember_semantic("d2", "doc dois", &emb).unwrap();
            db.invalidate("md/L4/d1", 500).unwrap();
            let all = db.recall(&emb, 10).unwrap();
            let at = db.recall_at(&emb, 10, 600).unwrap();
            assert!(all.iter().any(|h| h.key.ends_with("/d1")));
            assert!(!at.iter().any(|h| h.key.ends_with("/d1")), "recall_at deveria filtrar o inválido");
            assert!(at.iter().any(|h| h.key.ends_with("/d2")));
        }
        // persistência no reopen
        {
            let mut db = Sgdb::open(crate::storage::FileStorage::open(&path).unwrap()).unwrap();
            assert!(!db.validity_at(key, 700).unwrap(), "validade deveria persistir");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn lexical_recall_finds_exact_term_semantic_misses() {
        // #7: termo raro que o BQ (16-dim, 1 word) perde por colisão de bits é
        // recuperado pelo path lexical BM25-style.
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let emb = |seed: u64| -> Vec<f32> {
            let mut s = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let mut v = Vec::with_capacity(16);
            for _ in 0..16 {
                s = s.wrapping_mul(1103515245).wrapping_add(12345);
                v.push(((s >> 32) as i32 % 200) as f32 / 100.0 - 1.0);
            }
            v
        };
        db.remember_semantic("doc/quicksort", "algoritmo de ordenacao quicksort", &emb(1)).unwrap();
        db.remember_semantic("doc/bebida", "quicksort e o nome de uma bebida", &emb(2)).unwrap();
        // o termo "ordenacao" existe só em doc/quicksort → lexical top-1 é ele
        let hits = db.recall_lexical("ordenacao", 1).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].key.ends_with("doc/quicksort"), "lexical deveria achar quicksort: {}", hits[0].key);
        // híbrido: semântico + lexical, sem duplicar
        let hy = db.recall_hybrid(&emb(1), "ordenacao", 10).unwrap();
        let keys: Vec<&str> = hy.iter().map(|h| h.key.as_str()).collect();
        assert!(keys.contains(&"md/L2/doc/quicksort") || keys.contains(&"md/L4/doc/quicksort"));
        assert_eq!(hy.len(), hy.iter().map(|h| &h.key).collect::<alloc::collections::BTreeSet<_>>().len(), "híbrido não deve duplicar chaves");
    }

    #[test]
    fn node_id_explicit_and_stable() {
        let mut db = Sgdb::open_with_node_id(7, InMemory::new()).unwrap();
        assert_eq!(db.node_id(), 7);
        db.remember_fact("f", 1).unwrap();
        assert_eq!(db.node_id(), 7); // estável
    }

    // ── Deleção física vs estado lógico (item 9/17) ─────────────────────────

    #[test]
    fn delete_removes_doc_and_keeps_indexes_consistent() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("k1", "texto um", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        db.remember_exchange("oi", "ola").unwrap(); // L1 RAM
        assert!(db.scan_prefix("md/L4/").unwrap().len() >= 1);

        // validade marcada antes do delete — side-table deve morrer com o doc
        db.set_validity("md/L4/k1", 0, 1000).unwrap();
        assert!(!db.validity_at("md/L4/k1", 1500).unwrap()); // fora da janela

        assert!(db.delete("md/L4/k1").unwrap());
        // sumiu do storage + índices + side-tables
        assert!(db.get(MemoryLayer::L4Semantic, "k1").unwrap().is_none());
        assert!(db.scan_prefix("md/L4/").unwrap().is_empty());
        assert!(db.validity_at("md/L4/k1", 1500).unwrap()); // side-table limpa → default
        // idempotente: segunda chamada = false (sem tombstone espúrio no mapa)
        assert!(!db.delete("md/L4/k1").unwrap());

        // recall NÃO ressuscita o deletado (candidato BQ stale é pulado)
        let hits = db.recall(&[1.0, -1.0, 1.0, -1.0], 10).unwrap();
        assert!(hits.iter().all(|h| !h.key.contains("/k1")), "deletado ressuscitou: {:?}", hits);

        // re-add após delete funciona (id novo, índices consistentes)
        db.remember_semantic("k1", "de novo", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        let hits = db.recall(&[1.0, -1.0, 1.0, -1.0], 10).unwrap();
        assert!(hits.iter().any(|h| h.key.ends_with("/k1") && h.text == "de novo"));

        // L1/L0 (RAM) também são deletáveis
        assert!(db.delete("md/L1/last_user").unwrap());
        assert!(db.get(MemoryLayer::L1Working, "last_user").unwrap().is_none());
        // chave nunca existida → false
        assert!(!db.delete("md/L3/ts/nao-existe").unwrap());
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn delete_persists_across_reopen() {
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sgdb_delete.db");
        let _ = std::fs::remove_file(&path);
        {
            let mut db = Sgdb::open(crate::storage::FileStorage::open(&path).unwrap()).unwrap();
            db.remember_semantic("k1", "texto um", &[1.0, -1.0, 1.0, -1.0]).unwrap();
            db.remember_semantic("k2", "texto dois", &[-1.0, 1.0, -1.0, 1.0]).unwrap();
            assert!(db.delete("md/L4/k1").unwrap());
            db.checkpoint().unwrap();
        }
        {
            // tombstone aplicado no recovery: k1 não ressuscita, k2 sobrevive
            let mut db = Sgdb::open(crate::storage::FileStorage::open(&path).unwrap()).unwrap();
            assert!(db.get(MemoryLayer::L4Semantic, "k1").unwrap().is_none());
            assert!(db.get(MemoryLayer::L4Semantic, "k2").unwrap().is_some());
            let hits = db.recall(&[1.0, -1.0, 1.0, -1.0], 10).unwrap();
            assert!(hits.iter().all(|h| !h.key.contains("/k1")));
        }
        let _ = std::fs::remove_file(&path);
    }

    // ── Estágios explícitos do retrieval (item 4/16) ───────────────────────

    #[test]
    fn retrieval_stages_candidate_generation_then_fp32_rerank() {
        // BQ (sinais) e cosseno FP32 (magnitudes) DISCORDAM neste caso: o
        // top-1 do filtro grosseiro NÃO é o top-1 do rerank — prova que o
        // recall é geração de candidatos + reranking, e que o rerank muda a
        // ordem final.
        //   q = [10,1,1,1,1,1,1,1]
        //   A = [10,-1,-1,-1,-1,-1,-1,-1] → hamming 7,  cos = 93/107 ≈ 0.869
        //   B = [1,1,1,1,1,1,1,10]        → hamming 0,  cos = 26/107 ≈ 0.243
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("A", "vetor A", &[10.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0])
            .unwrap();
        db.remember_semantic("B", "vetor B", &[1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 10.0])
            .unwrap();
        let q = [10.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];

        // (1) candidate generation: B (hamming 0) lidera A (hamming 7)
        let coarse = db.bq().top_k_f32(&q, 2);
        assert_eq!(coarse.len(), 2);
        assert_eq!(coarse[0].1, 0, "BQ deveria preferir B (hamming 0): {coarse:?}");
        assert_eq!(coarse[1].1, 7, "BQ deveria colocar A em segundo (hamming 7): {coarse:?}");

        // (2)+(3) rerank FP32 inverte a ordem: A (dist≈0.13) antes de B (≈0.76)
        let hits = db.recall(&q, 2).unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits[0].key.ends_with("/A"), "rerank deveria trazer A primeiro: {:?}", hits);
        assert!(hits[1].key.ends_with("/B"));
        assert!(hits[0].dist < hits[1].dist);

        // (4) determinístico: mesma DB + mesma query + mesmo k ⇒ mesma ordem
        let again = db.recall(&q, 2).unwrap();
        let keys: Vec<&str> = hits.iter().map(|h| h.key.as_str()).collect();
        let keys2: Vec<&str> = again.iter().map(|h| h.key.as_str()).collect();
        assert_eq!(keys, keys2);
        assert_eq!(hits[0].dist.to_bits(), again[0].dist.to_bits());
    }

    // ── v0.6: identidade estável + proveniência (Phase 1) ──────────────────

    #[test]
    fn put_assigns_stable_identity_across_overwrite() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("k1", "alpha", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        let id1 = db.memory_id("md/L4/k1").unwrap().expect("id criado");
        assert_eq!(id1.len(), 32);
        assert_ne!(id1, alloc::format!("{}", db.node_id())); // nunca confundido com node_id
        // overwrite = MESMA memória: identidade estável
        db.remember_semantic("k1", "alpha v2", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        let id2 = db.memory_id("md/L4/k1").unwrap().expect("id");
        assert_eq!(id1, id2);
        // chave diferente → id diferente
        db.remember_semantic("k2", "beta", &[0.0, 1.0, 0.0, 1.0])
            .unwrap();
        let id3 = db.memory_id("md/L4/k2").unwrap().expect("id");
        assert_ne!(id1, id3);
        // memória deletada e recriada: watermark garante id NOVO
        db.delete("md/L4/k1").unwrap();
        db.remember_semantic("k1", "renascida", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        let id4 = db.memory_id("md/L4/k1").unwrap().expect("id");
        assert_ne!(id1, id4, "re-criação após delete físico deve ter id novo");
    }

    #[test]
    fn meta_viaja_com_o_doc_na_replicacao() {
        let mut src = Sgdb::open_with_node_id(3, InMemory::new()).unwrap();
        src.remember_semantic("k", "texto", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        let id = src.memory_id("md/L4/k").unwrap().unwrap();
        let doc = src.get(MemoryLayer::L4Semantic, "k").unwrap().unwrap();
        assert!(doc.meta.is_some(), "get deveria anexar a meta");
        // réplica com node_id diferente: identidade do CRIADOR viaja
        let mut dst = Sgdb::open_with_node_id(9, InMemory::new()).unwrap();
        dst.put(doc).unwrap();
        assert_eq!(dst.memory_id("md/L4/k").unwrap().unwrap(), id);
        let m = dst.meta("md/L4/k").unwrap().unwrap();
        assert_eq!(m.source, 3, "origem preservada na replicação");
        assert_eq!(m.created_tick, src.meta("md/L4/k").unwrap().unwrap().created_tick);
    }

    #[test]
    fn set_importance_confidence_contract() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("k", "t", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        // clamp de valores fora de 0..1
        db.set_importance("md/L4/k", 5.0).unwrap();
        assert_eq!(db.meta("md/L4/k").unwrap().unwrap().importance, 1.0);
        db.set_importance("md/L4/k", -2.0).unwrap();
        assert_eq!(db.meta("md/L4/k").unwrap().unwrap().importance, 0.0);
        db.set_confidence("md/L4/k", 0.7).unwrap();
        assert_eq!(db.meta("md/L4/k").unwrap().unwrap().confidence, 0.7);
        // não-finita → rejeitada (contrato explícito, sem NaN no storage)
        assert!(matches!(
            db.set_importance("md/L4/k", f32::NAN),
            Err(SgdbError::Invalid(_))
        ));
        assert!(matches!(
            db.set_confidence("md/L4/k", f32::INFINITY),
            Err(SgdbError::Invalid(_))
        ));
        // chave sem doc → erro
        assert!(matches!(
            db.set_importance("md/L4/nao-existe", 0.5),
            Err(SgdbError::Invalid(_))
        ));
    }

    #[test]
    fn supersede_wires_parent_ids() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("old", "texto antigo", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        db.remember_semantic("new", "texto novo", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        let old_id = db.memory_id("md/L4/old").unwrap().unwrap();
        db.supersede("md/L4/old", "md/L4/new").unwrap();
        assert_eq!(
            db.get_state("md/L4/old").unwrap(),
            MemoryState::Superseded
        );
        // lineage: new.parent_ids = [old.memory_id] — DAG causal começa aqui
        let nm = db.meta("md/L4/new").unwrap().unwrap();
        assert_eq!(nm.parent_ids, vec![old_id.clone()]);
        // supersede repetido não duplica o pai
        db.supersede("md/L4/old", "md/L4/new").unwrap();
        assert_eq!(
            db.meta("md/L4/new").unwrap().unwrap().parent_ids,
            vec![old_id]
        );
    }

    #[test]
    fn recall_exposes_provenance() {
        let mut db = Sgdb::open_with_node_id(7, InMemory::new()).unwrap();
        db.remember_semantic("k", "texto", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        db.set_importance("md/L4/k", 0.9).unwrap();
        db.set_confidence("md/L4/k", 0.7).unwrap();
        let hits = db.recall(&[1.0, -1.0, 1.0, -1.0], 5).unwrap();
        let h = hits.iter().find(|h| h.key.ends_with("/k")).expect("hit");
        let p = h.provenance.as_ref().expect("provenance no hit");
        assert_eq!(p.source, 7);
        assert_eq!(p.layer, MemoryLayer::L4Semantic);
        assert_eq!(p.state, MemoryState::Active);
        assert_eq!(p.importance, 0.9);
        assert_eq!(p.confidence, 0.7);
        assert_eq!(p.memory_id, db.memory_id("md/L4/k").unwrap().unwrap());
        // recall_lexical também expõe provenance
        let lh = db.recall_lexical("texto", 5).unwrap();
        let l = lh.iter().find(|h| h.key.ends_with("/k")).expect("lexical hit");
        assert!(l.provenance.is_some());
    }

    #[test]
    fn decayed_state_roundtrip() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("k", "t", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        db.set_state("md/L4/k", MemoryState::Decayed).unwrap();
        assert_eq!(db.get_state("md/L4/k").unwrap(), MemoryState::Decayed);
        assert_eq!(MemoryState::from_u8(4), Some(MemoryState::Decayed));
        // estado é lógico: o doc continua recuperável (histórico preservado)
        assert!(db.get(MemoryLayer::L4Semantic, "k").unwrap().is_some());
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn identity_and_meta_persist_across_reopen() {
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sgdb_meta.db");
        let _ = std::fs::remove_file(&path);
        let id;
        {
            let mut db = Sgdb::open(crate::storage::FileStorage::open(&path).unwrap()).unwrap();
            db.remember_semantic("k", "texto", &[1.0, -1.0, 1.0, -1.0])
                .unwrap();
            db.set_importance("md/L4/k", 0.8).unwrap();
            db.set_confidence("md/L4/k", 0.6).unwrap();
            id = db.memory_id("md/L4/k").unwrap().unwrap();
            db.checkpoint().unwrap();
        }
        {
            let mut db = Sgdb::open(crate::storage::FileStorage::open(&path).unwrap()).unwrap();
            // identidade + meta persistem (side-table sys/meta/ via Storage cru)
            assert_eq!(db.memory_id("md/L4/k").unwrap().unwrap(), id);
            let m = db.meta("md/L4/k").unwrap().unwrap();
            assert_eq!(m.importance, 0.8);
            assert_eq!(m.confidence, 0.6);
            // overwrite pós-reopen preserva a identidade (watermark reconstruído)
            db.remember_semantic("k", "texto v2", &[1.0, -1.0, 1.0, -1.0])
                .unwrap();
            assert_eq!(db.memory_id("md/L4/k").unwrap().unwrap(), id);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn dynamic_clock_overflow_persists_across_reopen() {
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sgdb_clock9.db");
        let _ = std::fs::remove_file(&path);
        {
            let mut db =
                Sgdb::open_with_node_id(1, crate::storage::FileStorage::open(&path).unwrap())
                    .unwrap();
            let mut doc = MemoryDoc::new(MemoryLayer::L4Semantic, "k", vec![0xAA; 4]);
            for n in 2..=10u8 {
                doc.clock.tick(n); // 9 nós → 8 fixos + overflow
            }
            doc.clock.tick(1); // 10º nó → overflow (fixo cheio)
            db.put(doc).unwrap();
            db.checkpoint().unwrap();
        }
        {
            let mut db =
                Sgdb::open_with_node_id(1, crate::storage::FileStorage::open(&path).unwrap())
                    .unwrap();
            // overflow re-fundido da side-table sys/meta/ (NMD1 guarda só 72B)
            let d = db.get(MemoryLayer::L4Semantic, "k").unwrap().unwrap();
            assert_eq!(d.clock.counter_of(10), 1);
            assert_eq!(d.clock.counter_of(9), 1);
            assert!(d.clock.counter_of(1) >= 2, "self tick + put tick");
            // identidade estável no overwrite pós-reopen
            let id1 = db.memory_id("md/L4/k").unwrap().unwrap();
            db.remember_semantic("k", "v2", &[1.0, -1.0, 1.0, -1.0])
                .unwrap();
            let id2 = db.memory_id("md/L4/k").unwrap().unwrap();
            assert_eq!(id1, id2);
        }
        let _ = std::fs::remove_file(&path);
    }

    // ── P0-5: export/import de MemoryRecord (doc + estado + validade) ─────

    #[test]
    fn export_import_preserves_state_and_validity() {
        let mut a = Sgdb::open_with_node_id(1, InMemory::new()).unwrap();
        let mut b = Sgdb::open_with_node_id(2, InMemory::new()).unwrap();
        let emb = [1.0, -1.0, 1.0, -1.0];
        a.remember_semantic("k1", "doc 1", &emb).unwrap();
        a.remember_semantic("k2", "doc 2", &emb).unwrap();
        a.supersede("md/L4/k1", "md/L4/k2").unwrap();
        a.set_validity("md/L4/k1", 0, 1000).unwrap();
        // exportação carrega estado + validade + meta
        let rec = a.export_record("md/L4/k1").unwrap().expect("record");
        assert_eq!(rec.state, MemoryState::Superseded);
        assert_eq!(rec.validity, Some((0, 1000)));
        let mid = rec.doc.meta.as_ref().unwrap().memory_id.clone();
        // importação aplica o side-metadata (contradição #2 fechada)
        b.import_record(rec).unwrap();
        assert_eq!(b.get_state("md/L4/k1").unwrap(), MemoryState::Superseded);
        assert!(!b.validity_at("md/L4/k1", 1500).unwrap());
        assert!(b.validity_at("md/L4/k1", 500).unwrap());
        // identidade do criador preservada (source = nó 1)
        assert_eq!(b.memory_id("md/L4/k1").unwrap().unwrap(), mid);
        assert_eq!(b.meta("md/L4/k1").unwrap().unwrap().source, 1);
        // chave sem doc → None
        assert!(a.export_record("md/L4/nao-existe").unwrap().is_none());
    }

    #[test]
    fn import_does_not_inflate_receiver_clock() {
        let mut a = Sgdb::open_with_node_id(1, InMemory::new()).unwrap();
        let mut b = Sgdb::open_with_node_id(2, InMemory::new()).unwrap();
        a.remember_semantic("k", "doc", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        let rec = a.export_record("md/L4/k").unwrap().unwrap();
        let clock_before = rec.doc.clock.clone();
        b.import_record(rec).unwrap();
        let doc = b.get(MemoryLayer::L4Semantic, "k").unwrap().unwrap();
        assert_eq!(
            doc.clock, clock_before,
            "import não deve tickar o relógio do receptor"
        );
        assert_eq!(
            doc.clock.counter_of(2),
            0,
            "receptor não vira escritor de memória alheia"
        );
    }

    #[test]
    fn import_derives_meta_from_clock_author() {
        // registro pré-v0.6 (sem meta) replicado: identidade derivada do
        // AUTOR do relógio, nunca reivindicada pelo receptor
        let mut b = Sgdb::open_with_node_id(5, InMemory::new()).unwrap();
        let mut doc = MemoryDoc::new(MemoryLayer::L4Semantic, "old", vec![1, 2, 3, 4]);
        doc.clock.tick(3); // autor original = nó 3
        let rec = MemoryRecord::new(doc, MemoryState::Active, None);
        b.import_record(rec).unwrap();
        let m = b.meta("md/L4/old").unwrap().unwrap();
        assert_eq!(m.source, 3, "autor derivado do relógio");
        assert_eq!(m.created_tick, 1);
        assert_ne!(m.source, 5, "não reivindica autoria local");
    }

    // ── P0-6: merge de record remoto sob política da camada ───────────────

    #[cfg(feature = "p2p")]
    #[test]
    fn merge_remote_policies() {
        use crate::crdt::{MergePolicy, MergeVerdict};
        let mut db = Sgdb::open_with_node_id(1, InMemory::new()).unwrap();
        // L0/L1/L6 → Rejected, nada importado
        for layer in [
            MemoryLayer::L0Sensory,
            MemoryLayer::L1Working,
            MemoryLayer::L6Reserved,
        ] {
            let rec = MemoryRecord::new(
                MemoryDoc::new(layer, "k", vec![1]),
                MemoryState::Active,
                None,
            );
            assert_eq!(db.merge_remote(rec).unwrap(), MergeVerdict::Rejected);
            assert!(db.get(layer, "k").unwrap().is_none(), "{layer:?} não deve importar");
        }
        // L4 sem local → Applied
        let mut doc = MemoryDoc::new(MemoryLayer::L4Semantic, "k", vec![1, 2, 3, 4]);
        doc.clock.tick(1);
        let rec = MemoryRecord::new(doc, MemoryState::Active, None);
        assert_eq!(db.merge_remote(rec).unwrap(), MergeVerdict::Applied);
        // duplicata (mesmo conteúdo causal + mesmo side-metadata) → Duplicate
        let mut doc = MemoryDoc::new(MemoryLayer::L4Semantic, "k", vec![1, 2, 3, 4]);
        doc.clock.tick(1);
        let rec = MemoryRecord::new(doc, MemoryState::Active, None);
        assert_eq!(db.merge_remote(rec).unwrap(), MergeVerdict::Duplicate);
        // CONCORRENTE (outro nó) → Conflict, local preservado, nada sobrescrito
        let mut doc = MemoryDoc::new(MemoryLayer::L4Semantic, "k", vec![9, 9, 9, 9]);
        doc.clock.tick(2);
        let rec = MemoryRecord::new(doc, MemoryState::Active, None);
        assert_eq!(db.merge_remote(rec).unwrap(), MergeVerdict::Conflict);
        let cur = db.get(MemoryLayer::L4Semantic, "k").unwrap().unwrap();
        assert_eq!(cur.payload, vec![1, 2, 3, 4], "conflito nunca sobrescreve o local");
        // Stale: remoto causalmente mais antigo (relógio vazio) → sem regressão
        let stale = MemoryRecord::new(
            MemoryDoc::new(MemoryLayer::L4Semantic, "k", vec![7, 7, 7, 7]),
            MemoryState::Active,
            None,
        );
        assert_eq!(db.merge_remote(stale).unwrap(), MergeVerdict::Stale);
        // remoto causalmente DOMINANTE → Applied; identidade do slot preservada
        let id_before = db.memory_id("md/L4/k").unwrap().unwrap();
        let mut doc = MemoryDoc::new(MemoryLayer::L4Semantic, "k", vec![5, 5, 5, 5]);
        doc.clock.tick(1);
        doc.clock.tick(1); // (1,2) domina o local (1,1)
        let rec = MemoryRecord::new(doc, MemoryState::Active, None);
        assert_eq!(db.merge_remote(rec).unwrap(), MergeVerdict::Applied);
        assert_eq!(
            db.memory_id("md/L4/k").unwrap().unwrap(),
            id_before,
            "overwrite dominante não muda a identidade do slot"
        );
        // política por camada consultável (tabela explícita)
        assert_eq!(
            MergePolicy::for_layer(MemoryLayer::L2EpisodicShort),
            MergePolicy::MultiValueRegister
        );
    }
}
