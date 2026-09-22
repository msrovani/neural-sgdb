//! ADR-0063 F3/D2 — AiosDatabaseEngine: MemoryDoc ↔ Storage + ART + BQ.
//! L0/L1: RAM-only por default (checkpoint explícito). ART guarda id lógico;
//! key = `md/Lx/...`. Instance-based (sem global ENGINE) — port do OS mãe.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::art::ArtIndex;
use crate::bq::BqFlatIndex;
use crate::lexical::LexicalIndex;
use crate::memory_doc::{
    generate_memory_id, MemoryDoc, MemoryDocView, MemoryLayer, MemoryMeta, MemoryRecord,
    MemoryState, RelationKind, VectorClock,
};
use crate::storage::{ScanResult, Storage, SgdbError};

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

/// Namespace lateral de validade temporal (`sys/validity/<storage_key>` →
/// 16B: `from u64le | until u64le`). #9 (padrão Zep/Graphiti): **invalidar-não-
/// deletar** — o doc permanece (histórico/NMD1 intacto), só marca a janela.
fn validity_key(sk: &str) -> Vec<u8> {
    let mut k = Vec::with_capacity(13 + sk.len());
    k.extend_from_slice(b"sys/validity/");
    k.extend_from_slice(sk.as_bytes());
    k
}

/// TTL per-key (v1.1.15 §4 P1): `sys/ttl/<sk>` → 8B `expires_at u64le`.
/// Expirado = candidato a GC físico (delete), distinto de invalidar-não-deletar.
fn ttl_key(sk: &str) -> Vec<u8> {
    let mut k = Vec::with_capacity(8 + sk.len());
    k.extend_from_slice(b"sys/ttl/");
    k.extend_from_slice(sk.as_bytes());
    k
}

/// Evento temporal (v1.1.15 §4 P2, mem0 jul/26): `sys/event/<sk>` →
/// `state_key u16len+bytes | event_end u64le`. Liga memórias do MESMO fato
/// evolutivo (timeline limpa, `recall_timeline`); `event_end=0` = aberto.
fn event_key(sk: &str) -> Vec<u8> {
    let mut k = Vec::with_capacity(10 + sk.len());
    k.extend_from_slice(b"sys/event/");
    k.extend_from_slice(sk.as_bytes());
    k
}

/// Namespace lateral de metadados de memória (`sys/meta/<storage_key>` →
/// MemoryMeta codec "MDM1", v0.6). Identidade + proveniência FORA do NMD1
/// (contrato byte-idêntico com o OS). Anexado no `get`; viaja com o doc na
/// replicação (`Sgdb::put` preserva `doc.meta`).
fn meta_key(sk: &str) -> Vec<u8> {
    let mut k = Vec::with_capacity(9 + sk.len());
    k.extend_from_slice(b"sys/meta/");
    k.extend_from_slice(sk.as_bytes());
    k
}

/// Índice reverso do DAG causal (`sys/version/<version_id>` → `[sklen u16 +
/// storage key | MemoryMeta da PRÓPRIA versão]`, v0.7 Phase 3). Permite
/// resolver um parent causal (version_id) de volta à meta DAQUELE VERSÃO
/// (não à meta corrente da chave — essencial para lineage em overwrites de
/// mesma chave) — base de `Sgdb::lineage` e do `explain`. Deriva da meta
/// (escrito em `persist_meta`/`ensure_meta`, reconstruído no rebuild) —
/// nunca fonte de verdade, sempre derivado.
fn version_key(vid: &str) -> Vec<u8> {
    let mut k = Vec::with_capacity(12 + vid.len());
    k.extend_from_slice(b"sys/version/");
    k.extend_from_slice(vid.as_bytes());
    k
}

/// Chave de side-table de um conflito (`sys/conflict/<id>` — v0.9).
fn conflict_key(id: &str) -> Vec<u8> {
    let mut k = Vec::with_capacity(14 + id.len());
    k.extend_from_slice(b"sys/conflict/");
    k.extend_from_slice(id.as_bytes());
    k
}

fn encode_version_entry(sk: &str, m: &MemoryMeta) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + sk.len() + m.encode().len());
    out.extend_from_slice(&(sk.len() as u16).to_le_bytes());
    out.extend_from_slice(sk.as_bytes());
    out.extend_from_slice(&m.encode());
    out
}

fn decode_version_entry(data: &[u8]) -> Option<(String, MemoryMeta)> {
    let sklen = u16::from_le_bytes(data.get(0..2)?.try_into().ok()?) as usize;
    let sk: String = core::str::from_utf8(data.get(2..2usize.checked_add(sklen)?)?)
        .ok()?
        .into();
    let m = MemoryMeta::decode(data.get(2usize.checked_add(sklen)?..)?).ok()?;
    Some((sk, m))
}

fn is_ram_layer(layer: MemoryLayer) -> bool {
    matches!(layer, MemoryLayer::L0Sensory | MemoryLayer::L1Working)
}

// ── L6 relations (v0.8): side-table persistente + índice ART derivado ──
//
// storage (fonte da verdade):  sys/rel/<kind>/<a>#<b>        → [fmt u8]
// ART forward (derivado):      rel/<kind>/<a>#<b>            → 0
// ART reverse (derivado):      rev/<kind>/<b>#<a>            → 0
//
// '#' é separador reservado — `associate` REJEITA chaves com '#', então o
// decode do rebuild nunca divide no lugar errado (e jamais panics).

fn rel_storage_key(kind: RelationKind, a: &str, b: &str) -> Vec<u8> {
    let mut k = Vec::with_capacity(16 + a.len() + b.len());
    k.extend_from_slice(b"sys/rel/");
    k.extend_from_slice(kind.as_str().as_bytes());
    k.push(b'/');
    k.extend_from_slice(a.as_bytes());
    k.push(b'#');
    k.extend_from_slice(b.as_bytes());
    k
}

fn rel_art_key(rev: bool, kind: RelationKind, x: &str, y: &str) -> String {
    let mut s = String::with_capacity(12 + x.len() + y.len());
    s.push_str(if rev { "rev/" } else { "rel/" });
    s.push_str(kind.as_str());
    s.push('/');
    s.push_str(x);
    s.push('#');
    s.push_str(y);
    s
}

fn parse_rel_storage_key(s: &str) -> Option<(RelationKind, String, String)> {
    let rest = s.strip_prefix("sys/rel/")?;
    let (kind_str, pair) = rest.split_once('/')?;
    let kind = RelationKind::from_str(kind_str)?;
    let (a, b) = pair.split_once('#')?;
    Some((kind, a.to_string(), b.to_string()))
}

pub struct AiosDatabaseEngine {
    pub art: ArtIndex,
    pub bq: BqFlatIndex,
    /// Índice lexical contextual (#7): textos L2/L3 → termos BM25-style.
    pub lexical: LexicalIndex,
    /// Índice de entidades nomeadas (v1.1.4 item 10, 1-hop): entidade →
    /// storage keys dos docs que a declaram (via `MemoryMeta.entities`).
    /// Derivado (`persist_meta`/`write_meta`/rebuild) — storage `sys/meta/`
    /// é a fonte da verdade. NUNCA extrai entidade do texto: quem fornece é
    /// a camada superior (mesmo contrato do `Embedder`).
    pub entity_index: BTreeMap<String, Vec<String>>,
    /// node_id local para vector clock
    pub node_id: u8,
    pub puts: u64,
    pub gets: u64,
    /// Dimensionalidades (número de f32) dos embeddings indexados no BQ
    /// (v1.1.3 S1): derivado de `payload.len()/4` dos docs L4/L5 — a fonte da
    /// verdade da dim. O recall avisa (em vez de silenciar) quando a query não
    /// casa com NENHUMA dim indexada: 4-dim ≠ 256-dim nunca casa por acidente.
    pub indexed_dims: BTreeSet<usize>,
    /// Soma acumulada dos embeddings por dim + contagem (v1.1.16 ADC-lite):
    /// `corpus_mean(dim)` = média dos vetores L4/L5 vivos, mantida por sessão
    /// (insert soma, delete/overwrite subtrai, rebuild reconstrói). A média
    /// re-expressa a QUERY (`sign(q - mean)`); os bitvecs ficam intactos.
    /// Aproximada por construção (nunca fonte de ranking final — o rescore
    /// FP32 decide); `f64` acumula sem deriva em `no_std`.
    corpus_sums: BTreeMap<usize, (Vec<f64>, u64)>,
    /// Blobs L0/L1 encoded (storage_key → NMD1); não toca Storage até checkpoint.
    ram_l0l1: BTreeMap<String, Vec<u8>>,
    /// Puts L0/L1 que bypassaram Storage (métrica honesty).
    pub ram_puts: u64,
    /// id lógico → storage_key (recall BQ → doc).
    id_to_sk: BTreeMap<u64, String>,
    /// (nó, contador do relógio) → storage keys: o vínculo versão CRDT ↔ doc
    /// (a versão N de um nó corresponde aos docs com counter_of(nó) == N).
    /// Base do pull DIRECIONADO por versões faltantes (anti-entropy, P0-7).
    /// Derivado (index_doc/rebuild) — storage = fonte da verdade.
    clock_index: BTreeMap<(u8, u64), Vec<String>>,
    /// Watermark do contador próprio (node_id): máximo contador deste nó em
    /// docs + overflow de metas. Garante `created_tick`/memory_id monotônicos
    /// através de restarts (o NMD1 72B perde o overflow >8 nós).
    own_clock_watermark: u64,
    storage: Box<dyn Storage>,
}

impl AiosDatabaseEngine {
    pub fn new(node_id: u8, storage: Box<dyn Storage>) -> Self {
        AiosDatabaseEngine {
            art: ArtIndex::new(),
            bq: BqFlatIndex::new(),
            lexical: LexicalIndex::new(),
            entity_index: BTreeMap::new(),
            node_id,
            puts: 0,
            gets: 0,
            ram_l0l1: BTreeMap::new(),
            ram_puts: 0,
            id_to_sk: BTreeMap::new(),
            clock_index: BTreeMap::new(),
            own_clock_watermark: 0,
            indexed_dims: BTreeSet::new(),
            corpus_sums: BTreeMap::new(),
            storage,
        }
    }

    pub fn backend_name(&self) -> &'static str {
        self.storage.name()
    }

    /// Scan CRU do storage por prefixo (P2-3, `Sgdb::health`/`validate`):
    /// a fonte da verdade é o storage — os índices (ART/BQ/lexical) são
    /// derivados. `no_std`-safe.
    pub fn scan_prefix_storage(&mut self, prefix: &[u8]) -> Result<ScanResult, SgdbError> {
        self.storage.scan_prefix(prefix)
    }

    /// Get CRU do storage por key (P2-3, `Sgdb::validate`): não passa pelo
    /// índice ART nem pelo decode NMD1 — integridade da side-table.
    pub fn storage_get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, SgdbError> {
        self.storage.get(key)
    }

    /// Put CRU do storage (P2-3, testes de integridade): bypassa índices —
    /// apenas para simular corrupção/injeção na fonte da verdade.
    #[cfg(test)]
    pub fn storage_put_raw(&mut self, key: &[u8], val: &[u8]) -> Result<(), SgdbError> {
        self.storage.put(key, val)
    }

    /// Put CRU do storage para o ledger de auditoria (v1.1.10 item 5):
    /// `sys/audit/<seq>` não passa por índices derivados (é fonte da verdade,
    /// não doc de memória).
    pub(crate) fn storage_put(&mut self, key: &[u8], val: &[u8]) -> Result<(), SgdbError> {
        self.storage.put(key, val)
    }

    /// Delete CRU do storage (v1.1.24): simétrico a `storage_put`, para
    /// side-tables que NÃO passam por índices derivados (`sys/negative/` do
    /// ledger — remover uma ausência não toca doc, ART, BQ nem lexical).
    pub(crate) fn storage_delete(&mut self, key: &[u8]) -> Result<(), SgdbError> {
        self.storage.delete(key)
    }

    /// Persiste doc: L0/L1 → RAM; demais → Storage (`md/Lx/key`) + indexa.
    /// v0.6: além do NMD1, escreve a side-table `sys/meta/` (identidade +
    /// proveniência) — identidade é ESTÁVEL: um doc já existente na chave
    /// mantém memory_id/source/created (overwrite = mesma memória); um doc
    /// que CHEGA com `meta` (replicação) preserva a identidade do criador.
    /// Put de AUTORIA local (tick do relógio próprio + watermark).
    pub fn put(&mut self, doc: MemoryDoc) -> Result<u64, SgdbError> {
        self.put_inner(doc, true)
    }

    /// Escreve um doc COMPANION do mesmo write lógico SEM tickar o relógio:
    /// reutiliza o contador próprio atual (watermark). Um `remember_semantic`
    /// grava L4+L2 sob a MESMA versão causal — um write lógico = uma versão
    /// (anti-entropy v0.7: `keys_for_clock(node, v)` retorna todos os docs da
    /// memória; sem isso o contador por-put e a versão do CRDT divergem e o
    /// pull direcionado perde docs).
    pub fn put_companion(&mut self, mut doc: MemoryDoc) -> Result<u64, SgdbError> {
        doc.clock
            .set_counter(self.node_id, self.own_clock_watermark);
        self.put_inner(doc, false)
    }

    /// Importação de REPLICAÇÃO (P0-5): `tick_local = false` NÃO incrementa o
    /// relógio próprio nem promove o watermark — o receptor nunca vira
    /// "escritor" de uma memória que não criou (sem inflação causal).
    fn put_inner(&mut self, mut doc: MemoryDoc, tick_local: bool) -> Result<u64, SgdbError> {
        // 11→1: central choke (inverso custo) — valida toda escrita antes de storage/índice,
        // fecha `merge_memories target`/`import`/`remember_episodic` bypass da facade.
        if doc.key.is_empty() || doc.key.len() > 4096 {
            return Err(SgdbError::Invalid("key length"));
        }
        if doc.key.contains('#') || doc.key.contains('\0') {
            return Err(SgdbError::Invalid("key contains # or NUL"));
        }
        if doc.key.split('/').any(|p| p == "." || p == "..") {
            return Err(SgdbError::Invalid("key contains . or .."));
        }
        if doc.key.bytes().any(|b| b < 0x20 || b == 0x7F) {
            return Err(SgdbError::Invalid("key contains control"));
        }
        if tick_local {
            doc.clock.tick(self.node_id);
            // Monotonia do contador próprio através de overwrites e restarts:
            // clocks frescos (ou com overflow perdido no NMD1) são promovidos
            // acima do watermark — `created_tick`/memory_id nunca regridem.
            let own = doc.clock.counter_of(self.node_id);
            if own <= self.own_clock_watermark {
                doc.clock
                    .set_counter(self.node_id, self.own_clock_watermark.saturating_add(1));
            }
            self.own_clock_watermark = doc.clock.counter_of(self.node_id);
        }

        let id = NEXT_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let sk = doc.storage_key();
        // Regra 4: ART não suporta prefix-key (chave nova que é prefixo de uma
        // existente, ou vice-versa). Guarda na borda: rejeita ANTES de gravar
        // storage/índice — caso contrário o insert_rec grava silenciosamente
        // errado e a chave mais curta fica inacessível.
        if self.art.has_prefix_conflict(&sk) {
            return Err(SgdbError::Invalid(
                "storage key is a prefix of an existing key (ART requires non-prefix keys)",
            ));
        }
        let blob = doc.encode();

        // ADC-lite: overwrite de L4/L5 subtrai o vetor ANTIGO da média antes
        // de somar o novo (sem isso o overwrite contaria 2x). Um get a mais
        // no write (não-hot) mantém a média exata sem scan.
        if matches!(
            doc.layer,
            MemoryLayer::L4Semantic | MemoryLayer::L5Procedural
        ) {
            if let Ok(Some(old)) = self.storage.get(sk.as_bytes()) {
                if let Ok(old_doc) = MemoryDoc::decode(&old) {
                    if let Some(v) = Self::payload_floats(&old_doc.payload) {
                        self.unnote_vec(&v);
                    }
                }
            }
        }

        if is_ram_layer(doc.layer) {
            self.ram_l0l1.insert(sk.clone(), blob);
            self.ram_puts = self.ram_puts.saturating_add(1);
        } else {
            self.storage.put(sk.as_bytes(), &blob)?;
        }

        self.persist_meta(&sk, &doc, tick_local)?;
        self.index_doc(id, &doc, &sk);
        self.puts += 1;
        Ok(id)
    }

    /// Escreve/lê `sys/meta/<sk>`. Regra de identidade:
    /// - `(None, None)` — criação: gera memory_id determinístico (slot).
    /// - `(None, Some(d))` — replicado: identidade do remetente viaja.
    /// - `(Some(e), _)` — overwrite: identidade LOCAL vence (memory_id é
    ///   estável por chave); confidence/importance/parents vêm do doc se
    ///   presentes, senão do existente.
    /// - `bump_version` (put local, Phase 3): cada escrita local que muda o
    ///   slot avança `version_id` e registra a versão anterior em
    ///   `parent_ids` — o DAG causal. Import/replicação NÃO bumpa (a versão
    ///   é do criador e viaja na meta).
    fn persist_meta(
        &mut self,
        sk: &str,
        doc: &MemoryDoc,
        bump_version: bool,
    ) -> Result<(), SgdbError> {
        let existing = match self.storage.get(&meta_key(sk)) {
            Ok(Some(b)) => MemoryMeta::decode(&b).ok(),
            _ => None,
        };
        let tick = doc.clock.counter_of(self.node_id);
        let mut m = match (existing, doc.meta.clone()) {
            (Some(e), d) if bump_version => {
                let mut mm = e;
                let new_vid = generate_memory_id(self.node_id, tick, doc.layer, &doc.key);
                if new_vid != mm.version_id {
                    // a versão anterior vira parent (linhagem causal)
                    if !mm.parent_ids.contains(&mm.version_id) {
                        mm.parent_ids.push(mm.version_id.clone());
                        // Janela de ancestralidade (fix AI-user audit): a meta é
                        // re-encoda e re-grava A CADA put — sem teto, o custo
                        // cresce O(turns²) no caminho overwrite (last_user/
                        // last_asst). Mantém os ancestrais MAIS RECENTES; o
                        // histórico completo permanece em sys/version/.
                        while mm.parent_ids.len() > crate::limits::MAX_PARENT_IDS {
                            mm.parent_ids.remove(0);
                        }
                    }
                    mm.version_id = new_vid;
                }
                if let Some(dd) = d {
                    mm.confidence = dd.confidence;
                    mm.importance = dd.importance;
                    for p in dd.parent_ids {
                        if !mm.parent_ids.contains(&p) {
                            mm.parent_ids.push(p);
                        }
                    }
                }
                mm
            }
            (Some(e), Some(d)) => {
                let mut mm = e;
                mm.confidence = d.confidence;
                mm.importance = d.importance;
                for p in d.parent_ids {
                    if !mm.parent_ids.contains(&p) {
                        mm.parent_ids.push(p);
                    }
                }
                mm
            }
            (Some(e), None) => e,
            (None, Some(d)) => d,
            (None, None) => {
                let mid = generate_memory_id(self.node_id, tick, doc.layer, &doc.key);
                MemoryMeta {
                    memory_id: mid.clone(),
                    version_id: mid, // 1ª versão do slot: versão == slot
                    source: self.node_id,
                    confidence: 1.0,
                    importance: doc.layer.default_importance(),
                    created_tick: tick,
                    parent_ids: Vec::new(),
                    clock_overflow: Vec::new(),
                    last_reinforced: 0,
                    scope: String::new(),
                    entities: Vec::new(),
                    content_type: None,
                    scope_dims: crate::memory_doc::ScopeDims::new(),
                    model_id: String::new(),
                }
            }
        };
        // overflow do relógio dinâmico persiste com a meta (o NMD1 não guarda)
        m.clock_overflow = doc.clock.overflow.clone();
        // índice reverso: version_id → (chave + meta da PRÓPRIA versão)
        let vid = m.version_id.clone();
        self.storage
            .put(&version_key(&vid), &encode_version_entry(sk, &m))?;
        self.storage.put(&meta_key(sk), &m.encode())?;
        self.reindex_entities(sk, &m);
        Ok(())
    }

    /// Re-indexa as entidades de `sk` no `entity_index` (derivado da meta —
    /// storage `sys/meta/` é a fonte da verdade). Remoção idempotente das
    /// entradas antigas antes de re-adicionar as atuais.
    pub fn reindex_entities(&mut self, sk: &str, m: &MemoryMeta) {
        for keys in self.entity_index.values_mut() {
            keys.retain(|k| k != sk);
        }
        self.entity_index.retain(|_, keys| !keys.is_empty());
        for e in &m.entities {
            if e.is_empty() {
                continue;
            }
            let entry = self.entity_index.entry(e.clone()).or_default();
            if !entry.iter().any(|k| k == sk) {
                entry.push(String::from(sk));
            }
        }
    }

    /// Remove `sk` de todas as listas do `entity_index` (delete de memória).
    pub fn remove_entities(&mut self, sk: &str) {
        for keys in self.entity_index.values_mut() {
            keys.retain(|k| k != sk);
        }
        self.entity_index.retain(|_, keys| !keys.is_empty());
    }

    /// Lê `sys/meta/<sk>` (None = sem metadados: registro pré-v0.6).
    pub fn read_meta(&mut self, sk: &str) -> Option<MemoryMeta> {
        match self.storage.get(&meta_key(sk)) {
            Ok(Some(b)) => MemoryMeta::decode(&b).ok(),
            _ => None,
        }
    }

    pub fn write_meta(&mut self, sk: &str, m: &MemoryMeta) -> Result<(), SgdbError> {
        self.storage.put(&meta_key(sk), &m.encode())?;
        self.reindex_entities(sk, m);
        Ok(())
    }

    /// Meta da memória em `sk` (None = sem doc OU registro pré-v0.6).
    pub fn meta(&mut self, sk: &str) -> Result<Option<MemoryMeta>, SgdbError> {
        Ok(self.read_meta(sk))
    }

    /// Deleta uma side-table cognitiva de `sk` do storage cru
    /// (`sys/meta/` | `sys/state/` | `sys/validity/`). Usado pelo rollback de
    /// auditoria (v1.1.10 item 5) para remover metadados de memórias criadas
    /// DEPOIS do checkpoint — payloads permanecem (ADD-only).
    pub(crate) fn delete_side_key(&mut self, kind: &[u8], sk: &str) -> Result<(), SgdbError> {
        let mut k = Vec::with_capacity(kind.len() + sk.len());
        k.extend_from_slice(kind);
        k.extend_from_slice(sk.as_bytes());
        self.storage.delete(&k)
    }

    /// Métricas de auditoria: último seq do ledger (`None` = ledger vazio).
    pub(crate) fn audit_last_seq(&mut self) -> Result<Option<u64>, SgdbError> {
        let rows = self.storage.scan_prefix(b"sys/audit/")?;
        let mut last = None;
        for (k, _) in rows {
            if let Some(s) = crate::audit::audit_seq_from_key(&k) {
                last = Some(last.map(|l: u64| l.max(s)).unwrap_or(s));
            }
        }
        Ok(last)
    }

    /// Escopo EFETIVO de uma storage key (v1.1.4 item 8): lê a meta própria;
    /// se vazia e a key é um companion `/L2/`, resolve o scope do primário
    /// `/L4/`/`/L5/`/`/L3/` do mesmo id (a meta do companion não carrega
    /// scope — quem o carrega é o doc dono do conteúdo). `""` = global.
    pub fn effective_scope(&mut self, sk: &str) -> String {
        if let Some(m) = self.read_meta(sk) {
            if !m.scope.is_empty() {
                return m.scope;
            }
        }
        if let Some(rest) = sk.strip_prefix("md/L2/") {
            for prim in ["md/L4/", "md/L5/", "md/L3/"] {
                if let Some(m) = self.read_meta(&format!("{prim}{rest}")) {
                    if !m.scope.is_empty() {
                        return m.scope;
                    }
                }
            }
        }
        String::new()
    }

    /// Escopo multi-dim EFETIVO (v1.1.14): mesma regra do companion que
    /// `effective_scope`, mas sobre `ScopeDims`. Legado `scope!=""` com dims
    /// vazias já foi mapeado para `user` no decode — aqui basta herdar.
    pub fn effective_scope_dims(&mut self, sk: &str) -> crate::memory_doc::ScopeDims {
        if let Some(m) = self.read_meta(sk) {
            if !m.scope_dims.is_global() {
                return m.scope_dims;
            }
            if !m.scope.is_empty() {
                return crate::memory_doc::ScopeDims {
                    user: m.scope,
                    agent: String::new(),
                    app: String::new(),
                    run: String::new(),
                };
            }
        }
        if let Some(rest) = sk.strip_prefix("md/L2/") {
            for prim in ["md/L4/", "md/L5/", "md/L3/"] {
                if let Some(m) = self.read_meta(&format!("{prim}{rest}")) {
                    if !m.scope_dims.is_global() {
                        return m.scope_dims;
                    }
                    if !m.scope.is_empty() {
                        return crate::memory_doc::ScopeDims {
                            user: m.scope,
                            agent: String::new(),
                            app: String::new(),
                            run: String::new(),
                        };
                    }
                }
            }
        }
        crate::memory_doc::ScopeDims::new()
    }

    /// Resolve um `version_id` à (storage key, meta DAQUELA VERSÃO) — DAG
    /// causal, base de `Sgdb::lineage`. Derivado de `sys/version/` (escrito
    /// no persist_meta, reconstruído no rebuild). `None` = versão não
    /// indexada (ex: antepassado cujo índice não sobreviveu a rebuild).
    pub fn version_record(
        &mut self,
        vid: &str,
    ) -> Result<Option<(String, MemoryMeta)>, SgdbError> {
        match self.storage.get(&version_key(vid))? {
            Some(b) => Ok(decode_version_entry(&b)),
            None => Ok(None),
        }
    }

    /// Varre TODAS as versões indexadas `sys/version/` (v0.9 — filhos no
    /// `explain`). Derivado; determinístico (ordenado por vid).
    pub fn scan_versions(&mut self) -> Result<Vec<(String, String, MemoryMeta)>, SgdbError> {
        let mut out = Vec::new();
        let rows = self.storage.scan_prefix(b"sys/version/")?;
        for (k, bytes) in rows {
            let vid = String::from_utf8_lossy(&k[12..]).into_owned(); // strip sys/version/
            if let Some((sk, m)) = decode_version_entry(&bytes) {
                out.push((vid, sk, m));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    /// Anexa `parent_ids` à meta de `sk` (linhagem causal, idempotente).
    /// Garante meta se o registro for pré-v0.6.
    pub fn add_parents(&mut self, sk: &str, parents: &[String]) -> Result<(), SgdbError> {
        let mut m = self.ensure_meta(sk)?;
        for p in parents {
            if !m.parent_ids.contains(p) {
                m.parent_ids.push(p.clone());
            }
        }
        self.write_meta(sk, &m)
    }

    /// Garante meta para `sk` (migração de registros pré-v0.6: cria
    /// identidade determinística a partir do doc). Err se o doc não existe.
    pub fn ensure_meta(&mut self, sk: &str) -> Result<MemoryMeta, SgdbError> {
        if let Some(m) = self.read_meta(sk) {
            return Ok(m);
        }
        let exists = self.ram_l0l1.contains_key(sk) || self.storage.get(sk.as_bytes())?.is_some();
        if !exists {
            return Err(SgdbError::Invalid(
                "no memory at key (use the full canonical storage key, e.g. md/L4/<key> — remember returns it)",
            ));
        }
        let doc = self
            .get_by_storage_key(sk)?
            .ok_or(SgdbError::Invalid(
                "no memory at key (use the full canonical storage key, e.g. md/L4/<key> — remember returns it)",
            ))?;
        let tick = doc.clock.counter_of(self.node_id);
        let mid = generate_memory_id(self.node_id, tick, doc.layer, &doc.key);
        let m = MemoryMeta {
            memory_id: mid.clone(),
            version_id: mid, // migração: 1ª versão = slot
            source: self.node_id,
            confidence: 1.0,
            importance: doc.layer.default_importance(),
            created_tick: tick,
            parent_ids: Vec::new(),
            clock_overflow: doc.clock.overflow.clone(),
            last_reinforced: 0,
            scope: String::new(),
            entities: Vec::new(),
            content_type: None,
            scope_dims: crate::memory_doc::ScopeDims::new(),
            model_id: String::new(),
        };
        // índice reverso também é derivado na migração (DAG consultável)
        self.storage
            .put(&version_key(&m.version_id), &encode_version_entry(sk, &m))?;
        self.write_meta(sk, &m)?;
        Ok(m)
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
        // vínculo versão CRDT ↔ doc: para cada (nó, contador) do relógio
        for (n, c) in doc.clock.entries() {
            let e = self.clock_index.entry((n, c)).or_default();
            if !e.iter().any(|k| k == sk) {
                e.push(String::from(sk));
            }
        }
        // path lexical (#7): textos L2/L3 alimentam o índice BM25-style
        match doc.layer {
            MemoryLayer::L2EpisodicShort | MemoryLayer::L3EpisodicLong => {
                if let Ok(text) = core::str::from_utf8(&doc.payload) {
                    self.lexical.add(sk, text);
                }
            }
            _ => {}
        }
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
                    // S1: só embeddings DECLARADOS (bitvec) alimentam a detecção
                    // de dim. payload.len()/4 = dim f32 (remember_semantic grava
                    // o embedding no payload). Docs com payload de TEXTO (sem
                    // bitvec) NÃO contam — texto re-interpretado como f32 é
                    // ruído, não dimensionalidade.
                    if doc.payload.len() >= 4 {
                        self.indexed_dims.insert(doc.payload.len() / 4);
                        // ADC-lite: a média do corpus alimenta o segundo path
                        // de candidatos (`sign(q - mean)`); bitvecs intactos.
                        if let Some(v) = Self::payload_floats(&doc.payload) {
                            self.note_vec(&v);
                        }
                    }
                } else if let Some(f) = Self::payload_floats_truncated(&doc.payload) {
                    self.bq.insert_f32(id, &f);
                    self.note_vec(&f);
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
        self.lexical = LexicalIndex::new();
        self.entity_index.clear();
        self.id_to_sk.clear();
        self.clock_index.clear();
        self.indexed_dims.clear();
        self.corpus_sums.clear();
        // watermark reconstruído do storage (docs = fonte da verdade)
        self.own_clock_watermark = 0;
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
                let own = doc.clock.counter_of(self.node_id);
                if own > self.own_clock_watermark {
                    self.own_clock_watermark = own;
                }
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
                // watermark: contador próprio nos docs (72B fixos)
                let own = doc.clock.counter_of(self.node_id);
                if own > self.own_clock_watermark {
                    self.own_clock_watermark = own;
                }
            }
        }
        // watermark também cobre o overflow (metas `sys/meta/`): um contador
        // próprio além do 8º nó não sobrevive no NMD1, mas persiste na meta
        let meta_keys = self.storage.scan_prefix(b"sys/meta/")?;
        for (mk, bytes) in meta_keys {
            if let Ok(m) = MemoryMeta::decode(&bytes) {
                for &(n, c) in &m.clock_overflow {
                    if n == self.node_id && c > self.own_clock_watermark {
                        self.own_clock_watermark = c;
                    }
                }
                // re-escrita idempotente do índice reverso (derivado da meta;
                // reconstruível — storage = fonte da verdade)
                let sk = String::from_utf8_lossy(&mk[9..]).into_owned(); // strip sys/meta/
                self.storage
                    .put(&version_key(&m.version_id), &encode_version_entry(&sk, &m))?;
                // índice de entidades (derivado, reconstruível)
                self.reindex_entities(&sk, &m);
            }
        }
        // relações L6 (v0.8): side-table → índice ART forward+reverse
        let rel_keys = self.storage.scan_prefix(b"sys/rel/")?;
        for (rk, _) in rel_keys {
            let s = String::from_utf8_lossy(&rk).into_owned();
            if let Some((kind, a, b)) = parse_rel_storage_key(&s) {
                self.art.insert(&rel_art_key(false, kind, &a, &b), 0);
                self.art.insert(&rel_art_key(true, kind, &b, &a), 0);
            }
        }
        Ok(n)
    }

    pub fn get(&mut self, layer: MemoryLayer, key: &str) -> Result<Option<MemoryDoc>, SgdbError> {
        let sk = alloc::format!("md/{}/{}", layer.as_str(), key);
        self.get_by_storage_key(&sk)
    }

    /// Load por storage key canônica `md/Lx/...` (RAM L0/L1 ou Storage).
    /// v0.6: anexa a meta (`sys/meta/`) e re-funde o overflow do relógio
    /// dinâmico que o NMD1 72B não carrega.
    pub fn get_by_storage_key(&mut self, sk: &str) -> Result<Option<MemoryDoc>, SgdbError> {
        self.gets += 1;
        if let Some(bytes) = self.ram_l0l1.get(sk) {
            let mut doc = MemoryDoc::decode(bytes).map_err(|_| SgdbError::Corrupt)?;
            self.attach_meta(sk, &mut doc);
            return Ok(Some(doc));
        }
        match self.storage.get(sk.as_bytes()) {
            Ok(Some(bytes)) => {
                let mut doc = MemoryDoc::decode(&bytes).map_err(|_| SgdbError::Corrupt)?;
                self.attach_meta(sk, &mut doc);
                Ok(Some(doc))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Batch-get de payloads de TEXTOS (v1.1.3 S3): uma passada por N keys,
    /// decodifica só o NMD1 e devolve o payload UTF-8 lossy. SEM `attach_meta`
    /// (cada meta é um `storage.get` extra — texto não precisa de meta) e sem
    /// RAM L0/L1 (companions L2/L3 vivem no storage). O `recall` usava um
    /// `get_by_storage_key` por hit para buscar o texto companion — com N hits
    /// isso era N×(doc + meta) reads. Aqui: N reads, deduplicados.
    pub fn get_texts_batch(&mut self, keys: &[String]) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for sk in keys {
            if out.contains_key(sk) {
                continue;
            }
            self.gets += 1;
            let text = match self.storage.get(sk.as_bytes()) {
                Ok(Some(bytes)) => MemoryDoc::decode(&bytes)
                    .ok()
                    .map(|d| String::from_utf8_lossy(&d.payload).into_owned())
                    .unwrap_or_default(),
                _ => String::new(),
            };
            out.insert(sk.clone(), text);
        }
        out
    }

    /// Anexa meta + overflow do relógio ao doc recém-decodificado.
    fn attach_meta(&mut self, sk: &str, doc: &mut MemoryDoc) {
        if let Some(m) = self.read_meta(sk) {
            let mut oc = VectorClock::new();
            for &(n, c) in &m.clock_overflow {
                oc.set_counter(n, c);
            }
            doc.clock.merge(&oc);
            doc.meta = Some(m);
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

    /// Soma um vetor à média do corpus (insert L4/L5 com payload f32).
    /// Vetores com qualquer não-finito são ignorados por inteiro (texto
    /// reinterpretado como f32 gera NaN/Inf — não contamina a média);
    /// dims além de `MAX_EMBEDDING_DIM` nunca chegam aqui (guard P1-1).
    fn note_vec(&mut self, v: &[f32]) {
        corpus_add(&mut self.corpus_sums, v);
    }

    /// Subtrai um vetor da média do corpus (delete/overwrite L4/L5).
    /// Mesma regra de `note_vec`: não-finitos ignorados por inteiro.
    /// Contador satura em 0; entrada zerada é removida (mapa limpo).
    fn unnote_vec(&mut self, v: &[f32]) {
        if v.is_empty() || v.iter().any(|x| !x.is_finite()) {
            return;
        }
        let dim = v.len();
        let remove = match self.corpus_sums.get_mut(&dim) {
            Some((sum, count)) => {
                for (i, s) in sum.iter_mut().enumerate() {
                    let x = v[i];
                    if x.is_finite() {
                        *s -= x as f64;
                    }
                }
                *count = count.saturating_sub(1);
                *count == 0
            }
            None => false,
        };
        if remove {
            self.corpus_sums.remove(&dim);
        }
    }

    /// Recomputa as somas do corpus DO ZERO a partir dos docs vivos no storage
    /// (v1.1.21, ADR-0011 §float) — a referência contra a qual o estado mantido
    /// incrementalmente é comparado no `validate`.
    ///
    /// Espelha os DOIS ramos de `index_doc` que chamam `note_vec` (bitvec +
    /// payload, e payload reinterpretado sem bitvec) e reusa as mesmas funções
    /// de leitura: se o critério de acumulação fosse reescrito aqui, a
    /// comparação divergiria por construção e o check seria inútil.
    pub fn recompute_corpus_sums(
        &mut self,
    ) -> Result<BTreeMap<usize, (Vec<f64>, u64)>, SgdbError> {
        let mut acc: BTreeMap<usize, (Vec<f64>, u64)> = BTreeMap::new();
        let rows = self.storage.scan_prefix(b"md/")?;
        for (_k, bytes) in rows {
            let doc = match MemoryDoc::decode(&bytes) {
                Ok(d) => d,
                Err(_) => continue,
            };
            if !matches!(doc.layer, MemoryLayer::L4Semantic | MemoryLayer::L5Procedural) {
                continue;
            }
            if doc.bitvec.is_some() {
                if doc.payload.len() >= 4 {
                    if let Some(v) = Self::payload_floats(&doc.payload) {
                        corpus_add(&mut acc, &v);
                    }
                }
            } else if let Some(f) = Self::payload_floats_truncated(&doc.payload) {
                corpus_add(&mut acc, &f);
            }
        }
        Ok(acc)
    }

    /// Divergências entre o `corpus_sums` MANTIDO (incremental) e o recomputado
    /// do storage (referência) — v1.1.21, ADR-0011 §float.
    ///
    /// `counts` comparam-se por **igualdade exata**: divergência de contagem é
    /// bug duro (um caminho de delete/overwrite deixou de subtrair). As `somas`
    /// são `f64` e a adição **não é associativa** — o rebuild acumula numa ordem
    /// e a escrita incremental noutra, então os últimos bits divergem
    /// LEGITIMAMENTE. A tolerância relativa aceita isso e rejeita deriva real;
    /// sem ela o check daria falso positivo em produção.
    ///
    /// Devolve `(dim, kind)` com `kind` em {`"count"`, `"sum"`}.
    pub fn corpus_sums_drift(&mut self) -> Result<Vec<(usize, &'static str)>, SgdbError> {
        let reference = self.recompute_corpus_sums()?;
        let mut out: Vec<(usize, &'static str)> = Vec::new();
        for (dim, (sum, count)) in &reference {
            match self.corpus_sums.get(dim) {
                None => out.push((*dim, "count")),
                Some((msum, mcount)) => {
                    if mcount != count {
                        out.push((*dim, "count"));
                    } else if !sums_within_tolerance(sum, msum) {
                        out.push((*dim, "sum"));
                    }
                }
            }
        }
        // dim presente no mantido e AUSENTE na referência: sobrou entrada órfã
        // (o storage é a fonte da verdade, então isto é a única direção que
        // pode acusar dado fantasmal).
        for dim in self.corpus_sums.keys() {
            if !reference.contains_key(dim) {
                out.push((*dim, "count"));
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    /// Test-only: adultera o `corpus_sums` MANTIDO para exercitar o caminho
    /// POSITIVO do detector de drift do `validate` (v1.1.21, ADR-0011).
    ///
    /// Existe só em build de teste: não se cria API pública que "mente" apenas
    /// para ser testável.
    #[cfg(test)]
    pub(crate) fn corrupt_corpus_sums_for_test(
        &mut self,
        dim: usize,
        sum_delta: f64,
        count_delta: i64,
    ) {
        if let Some((sum, count)) = self.corpus_sums.get_mut(&dim) {
            if let Some(first) = sum.first_mut() {
                *first += sum_delta;
            }
            *count = ((*count as i64) + count_delta).max(0) as u64;
        }
    }

    /// Payload reinterpretado como f32 com **truncamento** em múltiplo de 4
    /// (`len / 4` floats). Distinto de `payload_floats`, que exige
    /// `len % 4 == 0`. Compartilhado por `index_doc` (insert no BQ) e por
    /// `recompute_corpus_sums` — a mesma leitura, senão a comparação do
    /// `validate` divergiria por construção.
    fn payload_floats_truncated(payload: &[u8]) -> Option<Vec<f32>> {
        let n = payload.len() / 4;
        if n == 0 {
            return None;
        }
        let mut f = Vec::with_capacity(n);
        for i in 0..n {
            let o = i * 4;
            f.push(f32::from_le_bytes([
                payload[o],
                payload[o + 1],
                payload[o + 2],
                payload[o + 3],
            ]));
        }
        Some(f)
    }

    /// Decodifica o payload como vetor f32 (`None` se não for embedding:
    /// vazio, `len % 4 != 0`). Mesma leitura do rescore FP32 — sem alloc
    /// além do vetor de saída.
    fn payload_floats(payload: &[u8]) -> Option<Vec<f32>> {
        if payload.len() < 4 || !payload.len().is_multiple_of(4) {
            return None;
        }
        let n = payload.len() / 4;
        if n > crate::limits::MAX_EMBEDDING_DIM {
            return None;
        }
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let o = i * 4;
            out.push(f32::from_le_bytes([
                payload[o],
                payload[o + 1],
                payload[o + 2],
                payload[o + 3],
            ]));
        }
        Some(out)
    }

    /// Média dos embeddings vivos de `dim` (`None` = sem vetores nessa dim).
    /// Observabilidade do path ADC-lite (`Sgdb::corpus_mean`); o recall usa
    /// direto (sem cópia extra além desta).
    pub fn corpus_mean(&self, dim: usize) -> Option<Vec<f32>> {
        let (sum, count) = self.corpus_sums.get(&dim)?;
        if *count == 0 {
            return None;
        }
        Some(sum.iter().map(|s| (*s / *count as f64) as f32).collect())
    }

    /// Fingerprint canônico do estado DERIVADO (v1.1.21, ADR-0011) — o oráculo
    /// de equivalência de reconstrução (`fp(open) == fp(rebuild_indices())`).
    ///
    /// Regras em [`crate::fingerprint`]: ordem canônica; ids do ART, órfãos do
    /// BQ e floats FICAM FORA. Custo O(n log n) (as chaves do ART não iteram em
    /// ordem lexicográfica, então são ordenadas) — NÃO é chamado pelo
    /// `health()` default; use `health(view=index)` ou `Sgdb::index_fingerprint`.
    pub fn index_fingerprint(&self) -> u64 {
        use crate::fingerprint::{fp_mix_str, fp_mix_u64, FP_SEED};
        let mut h = FP_SEED;

        // 1. Chaves do ART ordenadas. O ID fica FORA: `NEXT_ID` é um contador
        //    global de processo, logo o mesmo corpus em dois opens produz ids
        //    diferentes — hash de id não é estável entre processos.
        let mut keys: Vec<String> = self
            .art
            .scan_prefix("")
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        keys.sort();
        h = fp_mix_u64(h, keys.len() as u64);
        for k in &keys {
            h = fp_mix_str(h, k);
        }

        // 2. BQ: geometria + vetores EFETIVOS (resolvidos id → storage key).
        //    Órfãos do BQ ficam FORA: são inertes por design e a recompacção
        //    (`reclaim_bq_orphans`) não pode mover o fingerprint.
        //    `bq.len()` NÃO entra: ele conta os órfãos, que ficam fora por
        //    design (ver abaixo). O que entra é a contagem de vetores VIVOS.
        h = fp_mix_u64(h, self.bq.words_per_vec as u64);
        let w = self.bq.words_per_vec;
        let mut live: Vec<(&str, &[u64])> = Vec::new();
        for (i, id) in self.bq.ids.iter().enumerate() {
            if let Some(sk) = self.id_to_sk.get(id) {
                let off = (i * w).min(self.bq.flat.len());
                let end = (off + w).min(self.bq.flat.len());
                live.push((sk.as_str(), &self.bq.flat[off..end]));
            }
        }
        live.sort_by(|a, b| a.0.cmp(b.0));
        h = fp_mix_u64(h, live.len() as u64);
        for (sk, words) in live {
            h = fp_mix_str(h, sk);
            for wa in words {
                h = fp_mix_u64(h, *wa);
            }
        }

        // 3. Dimensões indexadas (`BTreeSet`: ordem natural).
        h = fp_mix_u64(h, self.indexed_dims.len() as u64);
        for d in &self.indexed_dims {
            h = fp_mix_u64(h, *d as u64);
        }

        // 4. Lexical (postings em ordem canônica por construção).
        h = self.lexical.fp_mix_into(h);

        // 5. Entidades — as listas de keys são ordenadas (ordem de inserção
        //    do `set_entities` não pode mover o hash).
        h = fp_mix_u64(h, self.entity_index.len() as u64);
        for (ent, skeys) in &self.entity_index {
            h = fp_mix_str(h, ent);
            let mut sorted: Vec<&String> = skeys.iter().collect();
            sorted.sort();
            h = fp_mix_u64(h, sorted.len() as u64);
            for sk in sorted {
                h = fp_mix_str(h, sk);
            }
        }

        // 6. `corpus_sums`: SÓ os counts. As somas f64 NÃO são associativas —
        //    o rebuild acumula numa ordem e a escrita incremental noutra, então
        //    os últimos bits podem divergir LEGITIMAMENTE. A média é verificada
        //    à parte, com tolerância relativa (`Sgdb::validate`).
        h = fp_mix_u64(h, self.corpus_sums.len() as u64);
        for (dim, (_sum, count)) in &self.corpus_sums {
            h = fp_mix_u64(h, *dim as u64);
            h = fp_mix_u64(h, *count);
        }

        h
    }

    /// Dual-path grosseiro (v1.1.16 ADC-lite): `top_k` legado ∪ `top_k`
    /// com query re-expressa (`sign(q - mean)`), fundidos por id (menor
    /// hamming vence) e ordenados por `(dist, id)`. Retorna até `2*k`;
    /// o chamador trunca ao orçamento `cand` (rescore FP32 decide o ranking
    /// final). Sem média para a dim → só o path legado (zero regressão).
    pub fn bq_top_k_f32_dual(&self, query: &[f32], k: usize) -> Vec<(u64, u32)> {
        let plain = self.bq_top_k_f32(query, k);
        let Some(mean) = self.corpus_mean(query.len()) else {
            return plain;
        };
        let centered = self.bq.top_k_f32_minus_mean(query, &mean, k);
        if centered.is_empty() {
            return plain;
        }
        let mut by_id: BTreeMap<u64, u32> = BTreeMap::new();
        for (id, d) in plain.into_iter().chain(centered) {
            by_id
                .entry(id)
                .and_modify(|e| {
                    if d < *e {
                        *e = d;
                    }
                })
                .or_insert(d);
        }
        let mut out: Vec<(u64, u32)> = by_id.into_iter().collect();
        out.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        out.truncate(k.saturating_mul(2).max(1));
        out
    }

    pub fn storage_key_of(&self, id: u64) -> Option<&str> {
        self.id_to_sk.get(&id).map(|s| s.as_str())
    }

    /// v1.1.3 S4 — recuperação PROATIVA do BQ: o flat é append-only, então o
    /// `delete` físico deixa os ids no índice (inofensivos, o recall os pula
    /// — `storage_key_of` → None — mas medem o pool de candidatos). Quando a
    /// quantidade de órfãos passa de `threshold`, reempacota o índice
    /// (`BqFlatIndex::retain`) e devolve quantos removeu. `threshold = 0` =
    /// sempre reempacota. Idempotente por construção.
    pub fn reclaim_bq_orphans(&mut self, threshold: usize) -> usize {
        let orphans = self
            .bq
            .ids
            .iter()
            .filter(|id| self.id_to_sk.contains_key(id))
            .count();
        let orphans = self.bq.len().saturating_sub(orphans);
        if orphans == 0 {
            return 0;
        }
        if threshold > 0 && orphans < threshold {
            return 0;
        }
        self.bq.retain(|id| self.id_to_sk.contains_key(&id))
    }

    /// Storage keys cujo relógio tem `counter_of(node) == counter` — o
    /// vínculo versão CRDT ↔ docs (anti-entropy, P0-7): quando um peer pede
    /// a versão `counter` do nó `node`, estas são as memórias daquele write.
    /// Derivado do índice `clock_index` (reconstruível).
    pub fn keys_for_clock(&self, node: u8, counter: u64) -> Vec<String> {
        self.clock_index
            .get(&(node, counter))
            .cloned()
            .unwrap_or_default()
    }

    /// Acesso cru a uma side-table (escape hatch para metadados de
    /// replicação/host, ex: `sys/crdt/` do CRDT durável — P0-11). NÃO é
    /// uma API pública de leitura de memória.
    pub fn read_side_bytes(&mut self, key: &str) -> Result<Option<Vec<u8>>, SgdbError> {
        self.storage.get(key.as_bytes())
    }

    pub fn write_side_bytes(&mut self, key: &str, bytes: &[u8]) -> Result<(), SgdbError> {
        self.storage.put(key.as_bytes(), bytes)
    }

    // ── Conflicts (v0.9, roadmap Phase 14/15) ─────────────────────────────
    //
    // Persistido em `sys/conflict/<id>` — a evidência (records MDR1 dos
    // candidatos) sobrevive a restart e a resolução não depende do nó remoto.

    /// Upsert de um `ConflictRecord` (id determinístico → re-merge upserta).
    pub fn put_conflict(&mut self, c: &crate::conflict::ConflictRecord) -> Result<(), SgdbError> {
        let enc = c.try_encode().map_err(SgdbError::Invalid)?;
        self.storage.put(&conflict_key(&c.conflict_id), &enc)
    }

    pub fn get_conflict(&mut self, id: &str) -> Option<crate::conflict::ConflictRecord> {
        match self.storage.get(&conflict_key(id)) {
            Ok(Some(b)) => crate::conflict::ConflictRecord::decode(&b).ok(),
            _ => None,
        }
    }

    pub fn delete_conflict(&mut self, id: &str) -> Result<(), SgdbError> {
        self.storage.delete(&conflict_key(id))
    }

    /// Todos os conflitos persistidos, ordenados por id (determinístico).
    pub fn list_conflicts(&mut self) -> Vec<crate::conflict::ConflictRecord> {
        let mut out = Vec::new();
        if let Ok(rows) = self.storage.scan_prefix(b"sys/conflict/") {
            for (_, bytes) in rows {
                if let Ok(c) = crate::conflict::ConflictRecord::decode(&bytes) {
                    out.push(c);
                }
            }
        }
        out.sort_by(|a, b| a.conflict_id.cmp(&b.conflict_id));
        out
    }

    /// Contador próprio atual (watermark) — usado por `reinforce` (v0.9)
    /// para registrar `last_reinforced` sem tickar o relógio.
    pub fn own_counter(&self) -> u64 {
        self.own_clock_watermark
    }

    // ── L6 relations (v0.8, roadmap Phase 12) ─────────────────────────────
    //
    // Sem inferência: a camada superior afirma a relação, o SGDB armazena.
    // Persistência em `sys/rel/` (storage = fonte da verdade) + índice ART
    // forward/reverse (derivado, reconstruído no rebuild, removido no delete).

    /// Persiste `a --kind--> b` (idempotente: re-associate sobrescreve).
    /// Rejeita chaves com `#` (separador reservado do wire) e kinds inválidos.
    pub fn associate(
        &mut self,
        a: &str,
        rel: RelationKind,
        b: &str,
    ) -> Result<(), SgdbError> {
        // 11→1: valida `a`/`b` como escrita (mesma regra de `validate_written`)
        for k in [a, b] {
            if k.is_empty() || k.len() > 4096 {
                return Err(SgdbError::Invalid("relation key length"));
            }
            if k.contains('#') || k.contains('\0') {
                return Err(SgdbError::Invalid("relation key contains reserved '#' or NUL"));
            }
            if k.split('/').any(|p| p == "." || p == "..") {
                return Err(SgdbError::Invalid("relation key contains . or .."));
            }
            if k.bytes().any(|b| b < 0x20 || b == 0x7F) {
                return Err(SgdbError::Invalid("relation key contains control"));
            }
        }
        let sk = rel_storage_key(rel, a, b);
        // Regra 4: as chaves ART `rel/…`/`rev/…` também não podem ser prefixo
        // uma da outra (ex: `a="x"` e depois `a="x/y"`). Rejeita antes de
        // gravar storage.
        let fwd = rel_art_key(false, rel, a, b);
        let rev = rel_art_key(true, rel, b, a);
        if self.art.has_prefix_conflict(&fwd) || self.art.has_prefix_conflict(&rev) {
            return Err(SgdbError::Invalid(
                "relation key is a prefix of an existing key (ART requires non-prefix keys)",
            ));
        }
        // [fmt u8] = 0 — metadados futuros (created/node/confidence) entram
        // como versões posteriores sem quebrar o decode
        self.storage.put(&sk, &[0])?;
        self.art.insert(&fwd, 0);
        self.art.insert(&rev, 0);
        Ok(())
    }

    /// Alvos de `key --kind--> *` (relações de SAÍDA), via prefix scan ART.
    pub fn relations_outgoing(&self, kind: RelationKind, key: &str) -> Vec<String> {
        let prefix = alloc::format!("rel/{}/{}#", kind.as_str(), key);
        let mut out: Vec<String> = self
            .art
            .scan_prefix(&prefix)
            .into_iter()
            .filter_map(|(k, _)| k.strip_prefix(&prefix).map(|r| r.to_string()))
            .collect();
        out.sort();
        out
    }

    /// Todas as relações envolvendo `key` (saídas E entradas, todos os kinds),
    /// determinístico por (kind, alvo).
    pub fn related_to(&self, key: &str) -> Vec<(RelationKind, String)> {
        let mut out: Vec<(RelationKind, String)> = Vec::new();
        for kind in RelationKind::ALL {
            let fwd = alloc::format!("rel/{}/{}#", kind.as_str(), key);
            for (k, _) in self.art.scan_prefix(&fwd) {
                if let Some(rest) = k.strip_prefix(&fwd) {
                    out.push((kind, rest.to_string()));
                }
            }
            let rev = alloc::format!("rev/{}/{}#", kind.as_str(), key);
            for (k, _) in self.art.scan_prefix(&rev) {
                if let Some(rest) = k.strip_prefix(&rev) {
                    out.push((kind, rest.to_string()));
                }
            }
        }
        out.sort_by_key(|(k, t)| (*k as u8, t.clone()));
        out
    }

    /// Remove TODAS as relações que envolvem `key` (storage + ART forward +
    /// reverse) — chamado pelo delete: memória morta não mantém topologia.
    pub fn remove_relations_for(&mut self, key: &str) -> Result<(), SgdbError> {
        for kind in RelationKind::ALL {
            // saídas: rel/<kind>/<key>#<b>
            let fwd = alloc::format!("rel/{}/{}#", kind.as_str(), key);
            let out: Vec<String> = self
                .art
                .scan_prefix(&fwd)
                .into_iter()
                .map(|(k, _)| k)
                .collect();
            for k in out {
                if let Some(b) = k.strip_prefix(&fwd) {
                    self.storage.delete(&rel_storage_key(kind, key, b))?;
                }
                self.art.delete(&k);
                if let Some(b) = k.strip_prefix(&fwd) {
                    self.art.delete(&rel_art_key(true, kind, b, key));
                }
            }
            // entradas: rev/<kind>/<key>#<a>
            let rev = alloc::format!("rev/{}/{}#", kind.as_str(), key);
            let inc: Vec<String> = self
                .art
                .scan_prefix(&rev)
                .into_iter()
                .map(|(k, _)| k)
                .collect();
            for k in inc {
                if let Some(a) = k.strip_prefix(&rev) {
                    self.storage.delete(&rel_storage_key(kind, a, key))?;
                }
                self.art.delete(&k);
                if let Some(a) = k.strip_prefix(&rev) {
                    self.art.delete(&rel_art_key(false, kind, a, key));
                }
            }
        }
        Ok(())
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
        // AUDIT (1.3): estado ≠ Active cria side-table `sys/state/` — recusa
        // chave fantasma ANTES de gravar, senão `validate()` flagga órfã
        // (mesma família do bughunt do hot-test via MCP). `Active` é
        // remove-only e inócuo (supersede marca new→Active antes do new
        // existir); `import_record` grava o doc antes de setar estado.
        if st != MemoryState::Active && self.get_by_storage_key(sk)?.is_none() {
            return Err(SgdbError::Invalid("set_state: no memory at key"));
        }
        let k = state_key(sk);
        if st == MemoryState::Active {
            // Active = default: remove o registro lateral SOMENTE se existir
            // (LOW #5, review P6 — delete incondicional cresceria o log com
            // tombstone para chave nunca setada, ex: todo supersede novo→Active)
            if self.storage.get(&k)?.is_some() {
                self.storage.delete(&k)?;
            }
        } else {
            self.storage.put(&k, &[st as u8])?;
        }
        Ok(())
    }

    /// Janela de validade de um doc (#9): `from ≤ now < until`. `until <= from`
    /// remove a marcação (validade infinita = default). Side-table
    /// `sys/validity/` via Storage cru — NMD1 intacto.
    pub fn set_validity(&mut self, sk: &str, from: u64, until: u64) -> Result<(), SgdbError> {
        let k = validity_key(sk);
        if until <= from {
            self.storage.delete(&k)?;
        } else {
            let mut v = Vec::with_capacity(16);
            v.extend_from_slice(&from.to_le_bytes());
            v.extend_from_slice(&until.to_le_bytes());
            self.storage.put(&k, &v)?;
        }
        Ok(())
    }

    /// Janela de validade BRUTA de `sk` (`sys/validity/`) — `None` = sem
    /// marcação (sempre válido). Usada na exportação de `MemoryRecord`
    /// (P0-5): a janela viaja com o doc na replicação.
    pub fn validity_window(&mut self, sk: &str) -> Option<(u64, u64)> {
        match self.storage.get(&validity_key(sk)) {
            Ok(Some(b)) if b.len() == 16 => Some((
                u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
                u64::from_le_bytes([b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]]),
            )),
            _ => None,
        }
    }

    /// `true` se o doc está válido em `now`. Sem marcação = sempre válido.
    pub fn validity_at(&mut self, sk: &str, now: u64) -> bool {
        match self.storage.get(&validity_key(sk)) {
            Ok(Some(b)) if b.len() == 16 => {
                let from = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
                let until =
                    u64::from_le_bytes([b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]]);
                from <= now && now < until
            }
            _ => true,
        }
    }

    /// Invalida em `now` (preserva o `from` original, seta `until = now`).
    pub fn invalidate(&mut self, sk: &str, now: u64) -> Result<(), SgdbError> {
        let from = match self.storage.get(&validity_key(sk)) {
            Ok(Some(b)) if b.len() == 16 => u64::from_le_bytes([
                b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
            ]),
            _ => 0,
        };
        self.set_validity(sk, from, now)
    }

    /// TTL per-key (v1.1.15): `expires_at` em `now`-units. `expires_at=0`
    /// limpa (sem TTL = retenção infinita, default).
    pub fn set_ttl(&mut self, sk: &str, expires_at: u64) -> Result<(), SgdbError> {
        let k = ttl_key(sk);
        if expires_at == 0 {
            self.storage.delete(&k)?;
        } else {
            self.storage.put(&k, &expires_at.to_le_bytes())?;
        }
        Ok(())
    }

    /// Lê o TTL bruto (`None` = sem TTL).
    pub fn ttl_of(&mut self, sk: &str) -> Option<u64> {
        match self.storage.get(&ttl_key(sk)) {
            Ok(Some(b)) if b.len() == 8 => Some(u64::from_le_bytes([
                b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
            ])),
            _ => None,
        }
    }

    /// Expirado em `now` (`expires_at <= now`, `0` nunca expira).
    #[allow(dead_code)]
    pub fn ttl_expired(&mut self, sk: &str, now: u64) -> bool {
        matches!(self.ttl_of(sk), Some(e) if e != 0 && e <= now)
    }

    /// Evento temporal (v1.1.15): liga `sk` ao fato evolutivo `state_key`
    /// (`event_end=0` = aberto). `state_key` vazio limpa a marcação.
    pub fn set_event(
        &mut self,
        sk: &str,
        state_key: &str,
        event_end: u64,
    ) -> Result<(), SgdbError> {
        let k = event_key(sk);
        if state_key.is_empty() {
            self.storage.delete(&k)?;
            return Ok(());
        }
        if state_key.len() > crate::limits::MAX_KLEN {
            return Err(SgdbError::Invalid("state_key exceeds MAX_KLEN"));
        }
        let mut v = Vec::with_capacity(2 + state_key.len() + 8);
        v.extend_from_slice(&(state_key.len() as u16).to_le_bytes());
        v.extend_from_slice(state_key.as_bytes());
        v.extend_from_slice(&event_end.to_le_bytes());
        self.storage.put(&k, &v)?;
        Ok(())
    }

    /// Lê `(state_key, event_end)` (`None` = sem evento).
    pub fn event_of(&mut self, sk: &str) -> Option<(String, u64)> {
        let b = self.storage.get(&event_key(sk)).ok()??;
        if b.len() < 10 {
            return None;
        }
        let slen = u16::from_le_bytes([b[0], b[1]]) as usize;
        if b.len() != 2 + slen + 8 {
            return None;
        }
        let s = core::str::from_utf8(&b[2..2 + slen]).ok()?;
        let mut end = [0u8; 8];
        end.copy_from_slice(&b[2 + slen..]);
        Some((String::from(s), u64::from_le_bytes(end)))
    }

    /// Deleção FÍSICA por storage key canônica (`md/Lx/...`): remove do
    /// Storage (tombstone) + side-tables (`sys/state/`, `sys/validity/`) +
    /// índices derivados (ART, lexical, id→sk).
    ///
    /// Distinta de `set_state`/`supersede`/`invalidate` (invalidar-NÃO-
    /// deletar): aqui o doc SOME da história. O BQ é um índice flat
    /// append-only SEM remoção O(1) — as entradas ficam inertes (o recall
    /// resolve id→storage key e pula as que não resolvem; o rebuild/
    /// compactação eventualmente as reclama).
    ///
    /// Retorna `true` se o doc existia (RAM L0/L1 ou Storage) antes da
    /// remoção.
    pub fn delete(&mut self, sk: &str) -> Result<bool, SgdbError> {
        let old_bytes = self.storage.get(sk.as_bytes())?;
        let existed = self.ram_l0l1.contains_key(sk) || old_bytes.is_some();
        // ADC-lite: o vetor morto sai da média (média exata sem scan).
        if let Some(b) = old_bytes {
            if sk.starts_with("md/L4/") || sk.starts_with("md/L5/") {
                if let Ok(d) = MemoryDoc::decode(&b) {
                    if let Some(v) = Self::payload_floats(&d.payload) {
                        self.unnote_vec(&v);
                    }
                }
            }
        }
        self.storage.delete(sk.as_bytes())?;
        self.ram_l0l1.remove(sk);
        // side-tables da memória morrem com ela (estado + validade + meta)
        self.storage.delete(&state_key(sk))?;
        self.storage.delete(&validity_key(sk))?;
        self.storage.delete(&ttl_key(sk))?;
        self.storage.delete(&event_key(sk))?;
        // índice reverso da versão morre com a memória (DAG causal)
        if let Ok(Some(b)) = self.storage.get(&meta_key(sk)) {
            if let Ok(m) = MemoryMeta::decode(&b) {
                self.storage.delete(&version_key(&m.version_id))?;
            }
        }
        self.remove_entities(sk);
        self.storage.delete(&meta_key(sk))?;
        self.art.delete(sk);
        self.lexical.remove(sk);
        // relações L6 envolvendo a memória morta somem com ela (topologia)
        self.remove_relations_for(sk)?;
        // desliga o mapeamento id → sk: candidatos BQ desses ids são pulados
        let dead: Vec<u64> = self
            .id_to_sk
            .iter()
            .filter(|(_, v)| v.as_str() == sk)
            .map(|(id, _)| *id)
            .collect();
        for id in dead {
            self.id_to_sk.remove(&id);
        }
        // e o vínculo (nó, contador) → sk morre com o doc (anti-entropy)
        let dead_clock: Vec<(u8, u64)> = self
            .clock_index
            .iter()
            .filter(|(_, keys)| keys.iter().any(|k| k == sk))
            .map(|((n, c), _)| (*n, *c))
            .collect();
        for key in dead_clock {
            if let Some(v) = self.clock_index.get_mut(&key) {
                v.retain(|k| k != sk);
                if v.is_empty() {
                    self.clock_index.remove(&key);
                }
            }
        }
        Ok(existed)
    }

    /// Exporta uma memória como UNIDADE de replicação (P0-5): doc NMD1 com
    /// meta anexada + estado lógico + janela de validade. `None` = sem doc
    /// na chave. O lado remoto reimporta com `import_record` — estado e
    /// validade NÃO são mais perdidos no diff/pull (contradição #2).
    pub fn export_record(&mut self, sk: &str) -> Result<Option<MemoryRecord>, SgdbError> {
        let Some(doc) = self.get_by_storage_key(sk)? else {
            return Ok(None);
        };
        let state = self.get_state(sk);
        let validity = self.validity_window(sk);
        Ok(Some(MemoryRecord::new(doc, state, validity)))
    }

    /// Importa uma `MemoryRecord` replicada: grava o NMD1 SEM tick local (o
    /// receptor não vira escritor), preserva a meta do criador (ou deriva
    /// identidade determinística do relógio para registros pré-v0.6) e aplica
    /// estado + validade que viajam no record. Indexa ART/BQ/lexical como
    /// qualquer put.
    pub fn import_record(&mut self, mut rec: MemoryRecord) -> Result<u64, SgdbError> {
        if rec.doc.meta.is_none() {
            rec.doc.meta = Some(meta_for_import(&rec.doc));
        }
        let sk = rec.doc.storage_key();
        let id = self.put_inner(rec.doc, false)?;
        // side-metadata viaja com o doc (P0-5)
        self.set_state(&sk, rec.state)?;
        match rec.validity {
            Some((from, until)) => self.set_validity(&sk, from, until)?,
            None => self.set_validity(&sk, 0, 0)?, // sem marcação → limpa a local
        }
        Ok(id)
    }
}

/// Tolerância RELATIVA entre duas somas do corpus (v1.1.21, ADR-0011 §float).
///
/// f64 tem ~1e-16 de precisão relativa; a adição de N parcelas `f32` (exatas em
/// f64) em ordens diferentes acumula erro relativo da ordem de `N·eps` (~1e-11
/// para N=1e5). `1e-9` fica acima disso e ordens de magnitude abaixo de qualquer
/// deriva real (uma parcela faltante desloca a soma muito mais que isso).
const CORPUS_SUM_REL_TOL: f64 = 1e-9;

/// Acumula `v` no mapa de somas por dim, ignorando vetores com qualquer
/// componente não-finito POR INTEIRO (texto reinterpretado como f32 gera
/// NaN/Inf — não pode contaminar a média).
///
/// Compartilhado por `note_vec` (incremental) e `recompute_corpus_sums`
/// (referência do `validate`): a MESMA acumulação nos dois lados é o que torna
/// a comparação significativa.
fn corpus_add(map: &mut BTreeMap<usize, (Vec<f64>, u64)>, v: &[f32]) {
    if v.is_empty() || v.iter().any(|x| !x.is_finite()) {
        return;
    }
    let dim = v.len();
    let (sum, count) = map
        .entry(dim)
        .or_insert_with(|| (alloc::vec![0.0f64; dim], 0));
    for (i, s) in sum.iter_mut().enumerate() {
        let x = v[i];
        if x.is_finite() {
            *s += x as f64;
        }
    }
    *count = count.saturating_add(1);
}

/// Mesma magnitude relativa dentro de `CORPUS_SUM_REL_TOL` (com piso absoluto
/// de `1.0` para o caso de cancelamento total — soma ~0).
fn sums_within_tolerance(a: &[f64], b: &[f64]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for (x, y) in a.iter().zip(b.iter()) {
        let scale = x.abs().max(y.abs()).max(1.0);
        if (x - y).abs() > CORPUS_SUM_REL_TOL * scale {
            return false;
        }
    }
    true
}

/// Meta determinística para um doc REPLICADO sem meta (pré-v0.6): autor =
/// nó com o maior contador no relógio (tie-break: menor node_id) — nunca
/// reivindica autoria local de uma memória vinda de outro nó.
fn meta_for_import(doc: &MemoryDoc) -> MemoryMeta {
    let mut author = 0u8;
    let mut maxc = 0u64;
    for (n, c) in doc.clock.entries() {
        if c > maxc || (c == maxc && n < author) {
            maxc = c;
            author = n;
        }
    }
    let mid = generate_memory_id(author, maxc, doc.layer, &doc.key);
    MemoryMeta {
        memory_id: mid.clone(),
        version_id: mid, // import sem meta: a versão do autor == slot
        source: author,
        confidence: 1.0,
        importance: doc.layer.default_importance(),
        created_tick: maxc,
        parent_ids: Vec::new(),
        clock_overflow: doc.clock.overflow.clone(),
        last_reinforced: 0,
        scope: doc.meta.as_ref().map(|m| m.scope.clone()).unwrap_or_default(),
        entities: doc
            .meta
            .as_ref()
            .map(|m| m.entities.clone())
            .unwrap_or_default(),
        content_type: doc
            .meta
            .as_ref()
            .and_then(|m| m.content_type.clone()),
        scope_dims: doc
            .meta
            .as_ref()
            .map(|m| m.scope_dims.clone())
            .unwrap_or_default(),
        model_id: doc.meta.as_ref().map(|m| m.model_id.clone()).unwrap_or_default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// O caso que MOTIVA a tolerância: somas f64 corretas mas acumuladas em
    /// ordens diferentes não são bit-idênticas (adição não é associativa).
    #[test]
    fn tolerance_accepts_accumulation_order_difference() {
        let a = [0.1 + 0.2 + 0.3];
        let b = [0.3 + 0.2 + 0.1];
        assert!(sums_within_tolerance(&a, &b), "mesma soma, ordem diferente");
        assert!(sums_within_tolerance(&[0.0], &[0.0]));
    }

    /// Deriva REAL (uma parcela faltante) tem de ser rejeitada — senão o check
    /// seria decorativo.
    #[test]
    fn tolerance_rejects_real_drift() {
        assert!(!sums_within_tolerance(&[10.0, 0.0], &[13.0, 0.0]));
        assert!(!sums_within_tolerance(&[0.0], &[0.5]));
        assert!(!sums_within_tolerance(&[1.0], &[-1.0]));
    }

    /// Dimensões diferentes nunca são "quase iguais".
    #[test]
    fn tolerance_rejects_length_mismatch() {
        assert!(!sums_within_tolerance(&[1.0, 2.0], &[1.0]));
    }

    /// `payload_floats_truncated` é a leitura do ramo "sem bitvec" do
    /// `index_doc`: trunca em múltiplo de 4 (ao contrário de `payload_floats`,
    /// que exige `len % 4 == 0`).
    #[test]
    fn payload_floats_truncated_matches_index_doc_semantics() {
        assert!(AiosDatabaseEngine::payload_floats_truncated(&[]).is_none());
        assert!(
            AiosDatabaseEngine::payload_floats_truncated(&[1, 2, 3]).is_none(),
            "n = 0"
        );
        // 10 bytes: 2 floats + cauda de 2 → o truncamento ignora a cauda.
        let mut ten = Vec::new();
        ten.extend_from_slice(&1.0f32.to_le_bytes());
        ten.extend_from_slice(&2.0f32.to_le_bytes());
        ten.extend_from_slice(&[9, 9]);
        let v = AiosDatabaseEngine::payload_floats_truncated(&ten).unwrap();
        assert_eq!(
            v,
            alloc::vec![1.0f32, 2.0],
            "10 bytes → 2 floats, cauda ignorada"
        );
        assert!(
            AiosDatabaseEngine::payload_floats(&ten).is_none(),
            "payload_floats exige múltiplo de 4 — as duas leituras diferem"
        );
    }
}
