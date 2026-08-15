//! Sgdb — facade pública do neural-sgdb (instance-based).
//! Ponte Hermes/Cortex ↔ camadas MemoryDoc L0–L7 (port de `k_ai::sgdb::layers`).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::bq::{quantize_f32, BqFlatIndex};
use crate::engine::AiosDatabaseEngine;
use crate::memory_doc::{
    LineageEntry, MemoryDoc, MemoryLayer, MemoryMeta, MemoryRecord, MemoryState, RelationKind,
};
use crate::storage::{Storage, SgdbError};

/// Proveniência de um hit (v0.6 — Phase 9 parcial): epistemologia exposta ao
/// caller — memórias com estados diferentes NÃO se parecem iguais no recall.
#[derive(Clone, Debug, PartialEq)]
pub struct HitProvenance {
    /// Identidade do SLOT (estável através de overwrites).
    pub memory_id: String,
    /// Identidade da VERSÃO corrente (Phase 3 — DAG causal).
    pub version_id: String,
    pub layer: MemoryLayer,
    pub state: MemoryState,
    pub source: u8,
    pub confidence: f32,
    pub importance: f32,
    pub created_tick: u64,
    pub parent_ids: Vec<String>,
}

/// Estado observável de uma instância `Sgdb` (P2-3, substitui o `ready()`
/// de baixo valor). `no_std`-safe — só inteiros/str/vec.
#[derive(Clone, Debug, PartialEq)]
pub struct HealthReport {
    /// Backend ativo (`in-memory`, `file`, `tickv`).
    pub backend: &'static str,
    /// node_id local (vector clock / origem).
    pub node_id: u8,
    /// Backend respondeu a uma sonda de leitura sem erro.
    pub storage_ok: bool,
    /// Docs indexados no ART (md/L0..L7).
    pub doc_count: usize,
    /// Embeddings no índice BQ (L4).
    pub bq_len: usize,
    /// Blobs L0/L1 em RAM (não persistidos).
    pub ram_len: usize,
    /// Conflitos persistidos em aberto (`sys/conflict/`).
    pub open_conflicts: usize,
}

/// Problema de integridade encontrado por [`Sgdb::validate`] (P2-3).
/// Agregado: um erro não impede os demais checks — `validate()` retorna
/// TODOS os issues (vazia = saudável).
#[derive(Clone, Debug, PartialEq)]
pub struct ValidateIssue {
    /// Storage key (ou região) afetada.
    pub key: String,
    /// Descrição estática do problema.
    pub message: &'static str,
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

/// Explicação ESTRUTURADA do estado corrente de uma memória (v0.9,
/// roadmap Phase 17): por que ela está no estado em que está. Machine-
/// readable — a camada cognitiva converte em texto, nunca o contrário.
#[derive(Clone, Debug, PartialEq)]
pub struct MemoryExplanation {
    pub key: String,
    pub layer: MemoryLayer,
    pub state: MemoryState,
    pub memory_id: String,
    pub version_id: String,
    pub source: u8,
    pub confidence: f32,
    pub importance: f32,
    pub created_tick: u64,
    /// Último tick de reforço (0 = nunca) — evidência de uso/decay.
    pub last_reinforced: u64,
    /// Parentes causais (DAG) — supersessão/derivação/fusão.
    pub parents: Vec<String>,
    /// Janela de validade (None = sempre válido).
    pub validity: Option<(u64, u64)>,
    /// Quem supersedeu/foi supersedido: versões FILHAS deste slot (derivadas
    /// do índice `sys/version/` + `parent_ids`).
    pub children: Vec<String>,
}

/// Cognitive memory database. `Sgdb::open(backend)` + remember/recall.
pub struct Sgdb {
    engine: AiosDatabaseEngine,
    /// Observabilidade estruturada (v1.0, Phase 32): contadores nomeados,
    /// incrementados nos pontos de entrada. Snapshot via [`Sgdb::metrics`].
    pub(crate) metrics: crate::metrics::Metrics,
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
        let metrics = crate::metrics::Metrics {
            storage_recoveries: 1,
            index_rebuilds: 1,
            ..crate::metrics::Metrics::default()
        };
        Ok(Sgdb { engine, metrics })
    }

    /// Node_id local (vector clock / origem) — estável por instância.
    pub fn node_id(&self) -> u8 {
        self.engine.node_id
    }

    /// Snapshot de observabilidade (v1.0, Phase 32): contadores estruturados
    /// por subsistema — escreve/recall/lifecycle/conflitos/replicação/
    /// recovery. Para diffing entre instâncias, use `snapshot()`.
    pub fn metrics(&self) -> &crate::metrics::Metrics {
        &self.metrics
    }

    /// Diário por agente (mempalace): memórias L2 episódicas cujo `source`
    /// casa com `node_id`, mais recentes primeiro (keys `ts/<hex>` são
    /// sortable — a ordem lexicográfica do ART é temporal). Devolve
    /// `(storage_key, payload)`; limita ao agente via side-table `sys/meta/`.
    pub fn diary(&mut self, node_id: u8, limit: usize) -> Result<Vec<(String, String)>, SgdbError> {
        let mut matches = Vec::new();
        for (sk, _) in self.engine.art.scan_prefix("md/L2/") {
            let src = match self.engine.meta(&sk) {
                Ok(Some(m)) => m.source,
                _ => continue,
            };
            if src != node_id {
                continue;
            }
            match self.engine.get_by_storage_key(&sk) {
                Ok(Some(doc)) => {
                    matches.push((sk.clone(), String::from_utf8_lossy(&doc.payload).into_owned()));
                }
                _ => continue,
            }
        }
        // keys ts sortable são asc (antigas→novas) — reverter p/ recentes primeiro
        matches.reverse();
        matches.truncate(limit);
        Ok(matches)
    }

    /// Perfil agregado por agente (supermemory): fatos estáveis do `node_id`
    /// (L3/L4/L5, onde mora conhecimento de longo prazo), ordenados por
    /// importância desc e truncados ao `limit`. Devolve `(storage_key,
    /// importance, confidence, payload)` — pronto para injetar no prompt
    /// ("o que sabemos sobre este agente"). Filtro via `sys/meta/`. Para L4/L5
    /// o payload é o embedding — o texto legível é lido do companion L2
    /// (`md/L2/<key>`, mesma convenção do recall).
    pub fn profile(&mut self, node_id: u8, limit: usize) -> Result<Vec<(String, f32, f32, String)>, SgdbError> {
        let mut facts = Vec::new();
        for layer in ["md/L3/", "md/L4/", "md/L5/"] {
            for (sk, _) in self.engine.art.scan_prefix(layer) {
                let m = match self.engine.meta(&sk) {
                    Ok(Some(m)) => m,
                    _ => continue,
                };
                if m.source != node_id {
                    continue;
                }
                match self.engine.get_by_storage_key(&sk) {
                    Ok(Some(doc)) => {
                        // texto: payload legível ou companion L2 (para L4/L5 o
                        // payload é o embedding — mesmo path do recall)
                        let mut text = String::from_utf8_lossy(&doc.payload).into_owned();
                        if doc.bitvec.is_some() || text.is_empty() {
                            let csk = sk.replacen("/L4/", "/L2/", 1);
                            if let Ok(Some(cdoc)) = self.engine.get_by_storage_key(&csk) {
                                text = String::from_utf8_lossy(&cdoc.payload).into_owned();
                            }
                        }
                        facts.push((sk.clone(), m.importance, m.confidence, text));
                    }
                    _ => continue,
                }
            }
        }
        // importância desc (tie-break: chave, determinístico)
        facts.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0)));
        facts.truncate(limit);
        Ok(facts)
    }

    /// Redefine todos os contadores (ex: antes de um teste de carga).
    pub fn reset_metrics(&mut self) {
        self.metrics = crate::metrics::Metrics::default();
    }

    /// Estado lógico de uma memória (default `Active`). Estado ≠ deleção
    /// física: `superseded`/`archived`/`invalidated` continuam representáveis
    /// na história (maturation P5).
    pub fn get_state(&mut self, key: &str) -> Result<MemoryState, SgdbError> {
        let sk = self.resolve_known_key(key);
        Ok(self.engine.get_state(&sk))
    }

    /// Seta estado lógico de uma memória (persiste em `sys/state/`).
    pub fn set_state(&mut self, key: &str, st: MemoryState) -> Result<(), SgdbError> {
        let sk = self.resolve_known_key(key);
        self.engine.set_state(&sk, st)
    }

    /// Conveniência cognitiva: marca `old` como `Superseded` (histórico
    /// preservado, sem `delete` físico) — a cadeia causal sobrevive para CRDT
    /// (Doc 04 §3). Phase 3 (v0.7): registra a VERSÃO corrente de `old` em
    /// `new.parent_ids` (DAG causal — `version_id`, não só o slot).
    pub fn supersede(&mut self, old: &str, new: &str) -> Result<(), SgdbError> {
        let old_sk = self.resolve_known_key(old);
        let new_sk = self.resolve_known_key(new);
        self.engine.set_state(&old_sk, MemoryState::Superseded)?;
        self.engine.set_state(&new_sk, MemoryState::Active)?;
        if let Some(old_meta) = self.engine.meta(&old_sk)? {
            if let Some(mut nm) = self.engine.meta(&new_sk)? {
                let parent = old_meta.version_id;
                if !nm.parent_ids.contains(&parent) {
                    nm.parent_ids.push(parent);
                }
                self.engine.write_meta(&new_sk, &nm)?;
            }
        }
        Ok(())
    }

    /// Identidade estável da memória (v0.6). `None` = sem doc na chave ou
    /// registro pré-v0.6 ainda sem meta (re-put/`ensure_meta` atribui).
    pub fn memory_id(&mut self, key: &str) -> Result<Option<String>, SgdbError> {
        let sk = self.resolve_known_key(key);
        Ok(self.engine.meta(&sk)?.map(|m| m.memory_id))
    }

    /// Identidade da VERSÃO corrente (Phase 3): muda a cada overwrite local;
    /// `None` = sem doc/meta.
    pub fn version_of(&mut self, key: &str) -> Result<Option<String>, SgdbError> {
        let sk = self.resolve_known_key(key);
        Ok(self.engine.meta(&sk)?.map(|m| m.version_id))
    }

    /// Linhagem causal (Phase 3 — DAG): do doc em `key` para trás, seguindo
    /// o parent mais recente de cada versão. Cada elo expõe version_id,
    /// memory_id (slot), storage key, origem, tick de criação e parents —
    /// quem quiser explorar ramos (merge com 2 parents) caminha por
    /// `parent_ids`. Guarda de ciclos: nunca loopa. Determinística.
    pub fn lineage(&mut self, key: &str) -> Result<Vec<LineageEntry>, SgdbError> {
        use alloc::collections::BTreeSet;
        let sk = self.resolve_known_key(key);
        let mut out: Vec<LineageEntry> = Vec::new();
        let mut visited: BTreeSet<String> = BTreeSet::new();
        let mut cur = self.engine.meta(&sk)?.map(|m| (m, sk.clone()));
        while let Some((m, ck)) = cur {
            if !visited.insert(m.version_id.clone()) {
                break; // ciclo (não deveria existir; nunca loopa)
            }
            out.push(LineageEntry {
                version_id: m.version_id.clone(),
                memory_id: m.memory_id.clone(),
                storage_key: ck.clone(),
                source: m.source,
                created_tick: m.created_tick,
                parent_ids: m.parent_ids.clone(),
            });
            cur = match m.parent_ids.last() {
                // o índice guarda a meta DA PRÓPRIA versão — para overwrites
                // de mesma chave, a meta corrente da chave é a mais nova
                // (resolver só a chave looparia de volta).
                Some(parent) => match self.engine.version_record(parent)? {
                    Some((pk, pm)) => Some((pm, pk)),
                    None => break,
                },
                None => None,
            };
        }
        Ok(out)
    }

    /// Metadados completos (memory_id, source, confidence, importance,
    /// created_tick, parent_ids, clock_overflow).
    pub fn meta(&mut self, key: &str) -> Result<Option<MemoryMeta>, SgdbError> {
        let sk = self.resolve_known_key(key);
        self.engine.meta(&sk)
    }

    /// Importância [0..1] (normalizada; fora do intervalo é clampada, não-
    /// finita é rejeitada). Persistente em `sys/meta/`; usada pelo lifecycle
    /// (fase posterior) e exposta no recall via `Hit.provenance`.
    pub fn set_importance(&mut self, key: &str, importance: f32) -> Result<(), SgdbError> {
        if !importance.is_finite() {
            return Err(SgdbError::Invalid("importance must be finite"));
        }
        let sk = self.resolve_known_key(key);
        let mut m = self.engine.ensure_meta(&sk)?;
        m.importance = importance.clamp(0.0, 1.0);
        self.engine.write_meta(&sk, &m)
    }

    /// Confiança [0..1] (mesmo contrato da importância).
    pub fn set_confidence(&mut self, key: &str, confidence: f32) -> Result<(), SgdbError> {
        if !confidence.is_finite() {
            return Err(SgdbError::Invalid("confidence must be finite"));
        }
        let sk = self.resolve_known_key(key);
        let mut m = self.engine.ensure_meta(&sk)?;
        m.confidence = confidence.clamp(0.0, 1.0);
        self.engine.write_meta(&sk, &m)
    }

    /// Escopo de isolamento (v1.1.4 item 7, mem0 multi-tenancy): namespace
    /// opcional da memória — particiona recall/recall_scoped por
    /// user/agent/projeto. Vazio = escopo global (default, não bate filtro de
    /// scope). Registros pré-v1.1.4 decodificam com `scope = ""` (migração
    /// MDM1 v4 explícita).
    pub fn set_scope(&mut self, key: &str, scope: &str) -> Result<(), SgdbError> {
        let sk = self.resolve_known_key(key);
        let mut m = self.engine.ensure_meta(&sk)?;
        m.scope = scope.to_string();
        self.engine.write_meta(&sk, &m)
    }

    /// Lê o escopo atual de uma memória (vazio = global). `meta()` já expõe
    /// o campo — esta é a conveniência explícita para o filtro de scope.
    pub fn scope_of(&mut self, key: &str) -> Result<String, SgdbError> {
        let sk = self.resolve_known_key(key);
        Ok(self.engine.meta(&sk)?.map(|m| m.scope).unwrap_or_default())
    }

    /// Anexa `parent_ids` à meta da memória (linhagem causal do DAG) —
    /// usado pela promoção do lifecycle e pela fusão (`merge_memories`,
    /// v0.9). Idempotente; registros pré-v0.6 ganham meta via `ensure_meta`.
    pub fn add_parents(&mut self, key: &str, parents: &[String]) -> Result<(), SgdbError> {
        let sk = self.resolve_known_key(key);
        self.engine.add_parents(&sk, parents)
    }

    /// Reforço (v0.9, roadmap Phase 12): `importance += delta` (clampada a
    /// `[0,1]`) e `last_reinforced` = contador próprio atual. Persistente em
    /// `sys/meta/` (MDM1 v3). Não ticka o relógio — reforço é metadado
    /// cognitivo local, não uma nova versão causal.
    pub fn reinforce(&mut self, key: &str, delta: f32) -> Result<(), SgdbError> {
        if !delta.is_finite() {
            return Err(SgdbError::Invalid("reinforce delta must be finite"));
        }
        let sk = self.resolve_known_key(key);
        let mut m = self.engine.ensure_meta(&sk)?;
        m.importance = (m.importance + delta).clamp(0.0, 1.0);
        m.last_reinforced = self.engine.own_counter();
        self.engine.write_meta(&sk, &m)
    }

    /// Feedback de uso (cognee `improve`): re-pondera a memória pelo resultado
    /// real — `positive` sobe importância E confiança; `negative` desce ambas.
    /// `amount` (default 0.1) é a intensidade, aplicada com o mesmo contrato
    /// de clamp `[0,1]` + rejeição de não-finita. Não ticka o relógio (metadado
    /// cognitivo local). É o "a memória melhora com uso, não só cresce".
    pub fn feedback(&mut self, key: &str, positive: bool, amount: f32) -> Result<(), SgdbError> {
        if !amount.is_finite() {
            return Err(SgdbError::Invalid("feedback amount must be finite"));
        }
        let sk = self.resolve_known_key(key);
        let mut m = self.engine.ensure_meta(&sk)?;
        let d = if positive { amount } else { -amount };
        m.importance = (m.importance + d).clamp(0.0, 1.0);
        m.confidence = (m.confidence + d).clamp(0.0, 1.0);
        m.last_reinforced = self.engine.own_counter();
        self.engine.write_meta(&sk, &m)
    }

    /// `forget` cognitivo (roadmap §23): ARCHIVA a memória (`Archived`),
    /// nunca deleta. História permanece acessível (`recall_historical`,
    /// `lineage`). Para remoção física, `delete` é o caminho explícito.
    pub fn forget(&mut self, key: &str) -> Result<(), SgdbError> {
        let sk = self.resolve_known_key(key);
        self.engine.set_state(&sk, MemoryState::Archived)
    }

    /// Explicação estruturada (roadmap Phase 17): por que a memória está no
    /// estado em que está. Sem registro pré-v0.6 / sem doc → `Err`.
    pub fn explain(&mut self, key: &str) -> Result<MemoryExplanation, SgdbError> {
        let sk = self.resolve_known_key(key);
        let m = self.engine.ensure_meta(&sk)?;
        let doc = self.engine.get_by_storage_key(&sk)?;
        if doc.is_none() {
            return Err(SgdbError::Invalid("no memory at key"));
        }
        // filhos: versões que listam ESTA versão como parent
        let mut children = Vec::new();
        if let Ok(rows) = self.engine.scan_versions() {
            for (vid, _sk, meta) in rows {
                if meta.parent_ids.contains(&m.version_id) {
                    children.push(vid);
                }
            }
        }
        children.sort();
        Ok(MemoryExplanation {
            key: sk.clone(),
            layer: doc
                .as_ref()
                .map(|d| d.layer)
                .unwrap_or(crate::memory_doc::MemoryLayer::L4Semantic),
            state: self.engine.get_state(&sk),
            memory_id: m.memory_id,
            version_id: m.version_id,
            source: m.source,
            confidence: m.confidence,
            importance: m.importance,
            created_tick: m.created_tick,
            last_reinforced: m.last_reinforced,
            parents: m.parent_ids.clone(),
            validity: self.engine.validity_window(&sk),
            children,
        })
    }

    /// Transfere uma memória para outra camada (v0.9): novo doc na camada
    /// alvo com a MESMA identidade causal de slot, `parent_ids += [versão
    /// fonte]` e relação L6 `derived_from`; a fonte vira `Archived` (história
    /// preservada — nada é deletado). Generaliza a promoção do lifecycle.
    pub fn transfer_to(&mut self, key: &str, to_layer: MemoryLayer) -> Result<String, SgdbError> {
        let sk = self.resolve_known_key(key);
        let Some(rec) = self.engine.export_record(&sk)? else {
            return Err(SgdbError::Invalid("no memory at key"));
        };
        if rec.doc.layer == to_layer {
            return Ok(sk); // idempotente
        }
        let origin_vid = rec
            .doc
            .meta
            .as_ref()
            .map(|m| m.version_id.clone())
            .unwrap_or_else(|| String::from("pre-v0.6"));
        let mut doc =
            crate::memory_doc::MemoryDoc::new(to_layer, &rec.doc.key, rec.doc.payload.clone());
        doc.bitvec = None; // embedding é da camada superior
        self.engine.put(doc)?;
        let new_sk = alloc::format!("md/{}/{}", to_layer.as_str(), rec.doc.key);
        self.engine.add_parents(&new_sk, &[origin_vid])?;
        self.engine.associate(&new_sk, RelationKind::DerivedFrom, &sk)?;
        self.engine.set_state(&sk, MemoryState::Archived)?;
        Ok(new_sk)
    }

    /// Fusão de duas memórias (roadmap Phase 16): C nasce com
    /// `parent_ids = [A, B]`, payload concatenado (separador explícito) e
    /// importância/confiança = máximo das fontes. A e B ficam INTACTAS
    /// (história preservada). O alvo `target` deve ser uma chave NOVA;
    /// vazio → gerada de `a--b`.
    pub fn merge_memories(
        &mut self,
        a: &str,
        b: &str,
        target: &str,
    ) -> Result<String, SgdbError> {
        let sk_a = self.resolve_known_key(a);
        let sk_b = self.resolve_known_key(b);
        let Some(ra) = self.engine.export_record(&sk_a)? else {
            return Err(SgdbError::Invalid("no memory at key a"));
        };
        let Some(rb) = self.engine.export_record(&sk_b)? else {
            return Err(SgdbError::Invalid("no memory at key b"));
        };
        let layer = if (ra.doc.layer as u8) >= (rb.doc.layer as u8) {
            ra.doc.layer
        } else {
            rb.doc.layer
        };
        let key = if target.is_empty() {
            alloc::format!("{}--{}", ra.doc.key, rb.doc.key)
        } else {
            String::from(target)
        };
        let mut payload = ra.doc.payload.clone();
        payload.push(0x1F); // separador de unidade (0x1F = unit separator)
        payload.extend_from_slice(&rb.doc.payload);
        let mut doc = crate::memory_doc::MemoryDoc::new(layer, &key, payload);
        doc.bitvec = None;
        self.engine.put(doc)?;
        let new_sk = alloc::format!("md/{}/{}", layer.as_str(), key);
        let va = ra
            .doc
            .meta
            .as_ref()
            .map(|m| m.version_id.clone())
            .unwrap_or_else(|| String::from("pre-v0.6"));
        let vb = rb
            .doc
            .meta
            .as_ref()
            .map(|m| m.version_id.clone())
            .unwrap_or_else(|| String::from("pre-v0.6"));
        self.engine.add_parents(&new_sk, &[va, vb])?;
        let mut m = self.engine.ensure_meta(&new_sk)?;
        let ia = ra.doc.meta.as_ref().map(|m| m.importance).unwrap_or(0.0);
        let ib = rb.doc.meta.as_ref().map(|m| m.importance).unwrap_or(0.0);
        let ca = ra.doc.meta.as_ref().map(|m| m.confidence).unwrap_or(0.0);
        let cb = rb.doc.meta.as_ref().map(|m| m.confidence).unwrap_or(0.0);
        m.importance = ia.max(ib);
        m.confidence = ca.max(cb);
        self.engine.write_meta(&new_sk, &m)?;
        Ok(new_sk)
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

    /// Resolve uma chave para a storage key CANÔNICA existente (v1.1, AUDIT):
    /// se `md/{key}` não existe, tenta as camadas por prioridade (L4 semântica
    /// primeiro — o caso mais comum do agente), devolvendo a canônica
    /// encontrada. Se nada existe, devolve `md/{key}` puro (o caller decide —
    /// preserva a semântica de `None`/ghost). Determinístico; nunca inventa.
    fn resolve_known_key(&self, key: &str) -> String {
        let sk = self.resolve_storage_key(key);
        if sk.starts_with("sys/") || self.engine.art.get(&sk).is_some() {
            return sk;
        }
        // chave crua (sem `md/`): procurar nas camadas, prioridade semântica
        if !key.starts_with("md/") && !key.starts_with("sys/") {
            const ORDER: [MemoryLayer; 8] = [
                MemoryLayer::L4Semantic,
                MemoryLayer::L5Procedural,
                MemoryLayer::L3EpisodicLong,
                MemoryLayer::L2EpisodicShort,
                MemoryLayer::L6Reserved,
                MemoryLayer::L0Sensory,
                MemoryLayer::L1Working,
                MemoryLayer::L7Identity,
            ];
            for layer in ORDER {
                let cand = alloc::format!("md/{}/{key}", layer.as_str());
                if self.engine.art.get(&cand).is_some() {
                    return cand;
                }
            }
        }
        sk
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

    /// Camada episódica VERBATIM (mempalace): guarda o par user/response cru em
    /// L2 timestamped, sem extração nem resumo. A contraparte de retrieval é a
    /// mesma dos L2 — lexical (`recall_lexical`) e recall semântico via
    /// companions. Útil quando a extração perderia contexto (o banco nunca
    /// decide o que esquecer). Devolve as storage keys (`md/L2/<ts>/u`, `/a`).
    pub fn remember_episodic(&mut self, user: &str, response: &str, now: u64) -> Result<(String, String), SgdbError> {
        let ts = MemoryDoc::sortable_ts_key(now);
        let ts_u = format!("{ts}/u");
        let ts_a = format!("{ts}/a");
        let _ = crate::engine::remember_text(&mut self.engine, MemoryLayer::L2EpisodicShort, &ts_u, user)?;
        let _ = crate::engine::remember_text(&mut self.engine, MemoryLayer::L2EpisodicShort, &ts_a, response)?;
        Ok((format!("md/L2/{ts_u}"), format!("md/L2/{ts_a}")))
    }

    /// Indexa embedding L4 (BQ). `emb` vazio = no-op. O texto é armazenado em
    /// L2 (companion `md/L2/<key>`) para recall trazer texto legível.
    /// Grava uma memória semântica (L4 + companion L2). **Política de entrada
    /// (P1-1)**: `emb` vazio, com NaN/±Inf ou com dim > `MAX_EMBEDDING_DIM`
    /// retorna `SgdbError::Invalid` — nunca grava embeddas corruptas.
    pub fn remember_semantic(
        &mut self,
        key: &str,
        text: &str,
        emb: &[f32],
    ) -> Result<(), SgdbError> {
        crate::bq::check_embedding(emb)?;
        let mut payload = Vec::with_capacity(emb.len() * 4);
        for x in emb {
            payload.extend_from_slice(&x.to_le_bytes());
        }
        let mut doc = MemoryDoc::new(MemoryLayer::L4Semantic, key, payload);
        doc.bitvec = Some(quantize_f32(emb));
        let _ = self.engine.put(doc)?;
        // companion texto (para Hit.text / rag_context): MESMO contador causal
        // do write (put_companion, v0.7) — um write lógico = uma versão.
        let tdoc = MemoryDoc::new(MemoryLayer::L2EpisodicShort, key, text.as_bytes().to_vec());
        let _ = self.engine.put_companion(tdoc)?;
        self.metrics.memory_writes += 1;
        self.metrics.clock_changes += 1;
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
    /// Recall semântico **default = memórias ATIVAS** (v0.8, roadmap §13):
    /// `Superseded`/`Archived`/`Decayed`/`Invalidated` NUNCA se fingem de
    /// ativas no resultado — use [`Sgdb::recall_historical`] para incluí-las
    /// (com `provenance.state` exposto para o caller distinguir).
    pub fn recall(&mut self, query: &[f32], k: usize) -> Result<Vec<Hit>, SgdbError> {
        let words = self.engine.bq.words_per_vec;
        let ov = match words {
            0 | 1 => 16,
            2..=4 => 8,
            _ => 4,
        };
        self.recall_oversampled(query, k, ov)
    }

    /// Recall que INCLUI memórias inativas (superseded/archived/decayed/
    /// invalidated) — histórico explícito: cada `Hit.provenance.state` expõe
    /// o estado, nunca há silêncio sobre o que é corrente vs obsoleto.
    pub fn recall_historical(&mut self, query: &[f32], k: usize) -> Result<Vec<Hit>, SgdbError> {
        let words = self.engine.bq.words_per_vec;
        let ov = match words {
            0 | 1 => 16,
            2..=4 => 8,
            _ => 4,
        };
        self.recall_impl(query, k, ov, false, None)
    }

    /// Dimensionalidades (nº de f32) dos embeddings indexados no BQ (v1.1.3 S1).
    /// A query de recall precisa casar com UMA delas; `recall` avisa com
    /// `SgdbError::Invalid` se não casar com nenhuma (em vez de devolver ruído
    /// de hamming). Útil para o caller debugar o contrato "mesmo modelo na
    /// gravação e na busca" (P4).
    pub fn indexed_embedding_dims(&self) -> Vec<usize> {
        let mut dims: Vec<usize> = self.engine.indexed_dims.iter().copied().collect();
        dims.sort_unstable();
        dims
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
    ///    Filtros futuros (camada, metadado, temporal, proveniência) entram
    ///    entre 2 e 3 sem mudar o contrato público.
    pub fn recall_oversampled(
        &mut self,
        query: &[f32],
        k: usize,
        oversample: usize,
    ) -> Result<Vec<Hit>, SgdbError> {
        if query.is_empty() {
            return Ok(Vec::new());
        }
        self.recall_impl(query, k, oversample, true, None)
    }

    /// Recall **escopado** (v1.1.4 item 7, mem0 multi-tenancy): mesmo pipeline
    /// de `recall`, mas o filtro de `scope` corre DENTRO do pool de candidatos
    /// — memórias de outro user/agent/projeto não competem por vagas do
    /// top-k. Escopo vazio (`""`) = memórias GLOBAIS (sem scope). Memórias
    /// sem marcação NUNCA batem um filtro de scope não-vazio (default global
    /// ≠ escopadas, por design — busca sem scope não vaza de outros scopes).
    pub fn recall_scoped(
        &mut self,
        query: &[f32],
        k: usize,
        scope: &str,
    ) -> Result<Vec<Hit>, SgdbError> {
        self.recall_scoped_impl(query, k, scope, true, true)
    }

    /// Recall escopado com histórico explícito (todas as memórias do scope,
    /// inclusive inativas — mesmo contrato de `recall_historical`).
    pub fn recall_scoped_historical(
        &mut self,
        query: &[f32],
        k: usize,
        scope: &str,
    ) -> Result<Vec<Hit>, SgdbError> {
        self.recall_scoped_impl(query, k, scope, false, false)
    }

    fn recall_scoped_impl(
        &mut self,
        query: &[f32],
        k: usize,
        scope: &str,
        active_only: bool,
        auto_oversample: bool,
    ) -> Result<Vec<Hit>, SgdbError> {
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let ov = if auto_oversample {
            match self.engine.bq.words_per_vec {
                0 | 1 => 16,
                2..=4 => 8,
                _ => 4,
            }
        } else {
            1
        };
        self.recall_impl(query, k, ov, active_only, Some(scope))
    }

    /// Núcleo do recall com modo de estado explícito. `active_only = true`:
    /// memórias inativas são descartadas ANTES do ranking (não consomem
    /// vagas do top-k); `false` = histórico (todas, com provenance exposta).
    fn recall_impl(
        &mut self,
        query: &[f32],
        k: usize,
        oversample: usize,
        active_only: bool,
        scope: Option<&str>,
    ) -> Result<Vec<Hit>, SgdbError> {
        if query.is_empty() {
            return Ok(Vec::new());
        }
        // P1-1: query não-finita ou oversized corrompe o ranking (NaN → score 0)
        for &x in query {
            if !x.is_finite() {
                return Err(SgdbError::Invalid("query contains NaN/Inf"));
            }
        }
        if query.len() > crate::bq::MAX_EMBEDDING_DIM {
            return Err(SgdbError::Invalid("query exceeds MAX_EMBEDDING_DIM"));
        }
        // v1.1.3 S1: dimensionalidade incompatível NÃO é silêncio. Se o BQ já
        // indexou embeddings e a query não casa com NENHUMA dim (L4/L5 gravam
        // o embedding f32 em payload → dim = payload.len()/4), o recall nunca
        // achará a memória — 4-dim ≠ 256-dim (contrato P4: mesmo modelo na
        // gravação e na busca). Avisar é melhor que devolver ruído de hamming.
        if !self.engine.indexed_dims.is_empty()
            && !self.engine.indexed_dims.contains(&query.len())
        {
            return Err(SgdbError::Invalid(
                "query dimensionality does not match any indexed embedding \
                 (use the SAME model on write and query — see indexed_embedding_dims)",
            ));
        }
        self.metrics.recalls += 1;
        let k = k.max(1);
        let cand = k.saturating_mul(oversample.max(1));
        let hits = self.engine.bq_top_k_f32(query, cand);
        // Distância Hamming máxima de um vetor indexado (normaliza o fallback
        // p/ escala 0..1 do contrato de `Hit.dist` — bughunt #11).
        let ham_max = (self.engine.bq.words_per_vec.max(1) * 64) as f32;
        let mut out: Vec<(u32, Hit)> = Vec::new();
        // v1.1.3 S3: companions L2 buscados em BATCH (uma passada por N keys,
        // sem attach_meta) em vez de um get_by_storage_key por hit (N×2 reads).
        let mut companion_keys: Vec<String> = Vec::new();
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
                    // v0.8 — estado lido UMA vez e filtrado ANTES do ranking:
                    // memória inativa não compete por vagas do top-k
                    let state = self.engine.get_state(&sk);
                    if active_only && state != MemoryState::Active {
                        continue;
                    }
                    // v1.1.4 item 7 — filtro de SCOPE dentro do pool de
                    // candidatos (memória de outro user/agent/projeto não
                    // compete por vagas do top-k do scope corrente). None =
                    // GLOBAL (filtro implícito mem0: busca sem scope não
                    // vaza de scopes escopados).
                    let doc_scope = self.engine.effective_scope(&sk);
                    match scope {
                        Some(s) if doc_scope != s => continue,
                        None if !doc_scope.is_empty() => continue,
                        _ => {}
                    }
                    // v0.6 — provenance exposta (Phase 9 parcial): quem/quando/
                    // quão confiável/estado — memórias superseded não se fingem
                    // de ativas no resultado.
                    let prov = doc.meta.as_ref().map(|m| HitProvenance {
                        memory_id: m.memory_id.clone(),
                        version_id: m.version_id.clone(),
                        layer: doc.layer,
                        state,
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
            companion_keys.push(sk.replacen("/L4/", "/L2/", 1));
            out.push((
                score,
                Hit {
                    key: sk,
                    text: String::new(),
                    dist,
                    provenance,
                },
            ));
        }
        // batch-get dos textos companion (S3) — deduplicado por key
        let texts = self.engine.get_texts_batch(&companion_keys);
        for (_, h) in out.iter_mut() {
            if let Some(t) = texts.get(&h.key.replacen("/L4/", "/L2/", 1)) {
                h.text = t.clone();
            }
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
    /// **importance vem da meta do DOC** (`set_importance`/`reinforce` —
    /// exposta via `Hit.provenance.importance`); registros pré-v0.6 sem meta
    /// caem para a default da camada (`md/LX/`). Busca um pool maior (`k·16`)
    /// para que recência/importância possam puxar candidatos fora do top-k
    /// semântico.
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
            // importância no espaço de PENALIDADE (menor = melhor): da meta do
            // doc (0..1, 1 = mais importante → penalty 1−imp) ou, para
            // registros pré-v0.6 sem meta, a default da camada via
            // layer_importance (já é penalty — L4 default importance 1.0 → 0.0)
            let penalty = match h.provenance.as_ref() {
                Some(p) => 1.0 - p.importance as f64,
                None => layer_importance(&h.key),
            };
            let s = w_sem as f64 * h.dist as f64
                + w_rec as f64 * rec
                + w_imp as f64 * penalty;
            scored.push((s, h));
        }
        scored.sort_by(|a, b| {
            a.0.total_cmp(&b.0).then_with(|| a.1.key.cmp(&b.1.key))
        });
        Ok(scored.into_iter().take(k).map(|(_, h)| h).collect())
    }

    /// Janela de validade (#9, Zep/Graphiti pattern): `from ≤ now < until`.
    /// `key` = storage key canônica (`md/Lx/...`). Side-table `sys/validity/`;
    /// o doc NUNCA é deletado — só marcado.
    pub fn set_validity(&mut self, key: &str, from: u64, until: u64) -> Result<(), SgdbError> {
        let sk = self.resolve_known_key(key);
        self.engine.set_validity(&sk, from, until)
    }

    pub fn validity_at(&mut self, key: &str, now: u64) -> Result<bool, SgdbError> {
        let sk = self.resolve_known_key(key);
        Ok(self.engine.validity_at(&sk, now))
    }

    pub fn invalidate(&mut self, key: &str, now: u64) -> Result<(), SgdbError> {
        let sk = self.resolve_known_key(key);
        self.engine.invalidate(&sk, now)
    }

    /// Esquecimento temporal automático (supermemory): varre `sys/validity/` e
    /// marca como `Invalidated` as memórias cuja janela já fechou em `now`
    /// (`until <= now`). Idempotente (segunda passada não re-marca); devolve
    /// quantas expiraram. Passo periódico — a memória envelhece sem crescer
    /// sem limite, e o recall default (active-only) passa a ignorá-las sem
    /// apagá-las (história preservada via `recall_historical`).
    pub fn expire_old(&mut self, now: u64) -> Result<usize, SgdbError> {
        let rows = self.engine.scan_prefix_storage(b"sys/validity/")?;
        let mut expired = 0usize;
        for (vk, bytes) in rows {
            if bytes.len() != 16 {
                continue;
            }
            let until = u64::from_le_bytes([
                bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
            ]);
            if until > now {
                continue; // ainda válida
            }
            // sys/validity/<md/...> → storage key
            let sk = String::from_utf8_lossy(&vk[13..]).into_owned();
            if self.engine.get_by_storage_key(&sk)?.is_none() {
                continue; // chave fantasma — sem doc, não marca (AUDIT 1.3)
            }
            match self.engine.get_state(&sk) {
                MemoryState::Invalidated | MemoryState::Archived | MemoryState::Decayed => continue,
                _ => {}
            }
            self.engine.set_state(&sk, MemoryState::Invalidated)?;
            expired += 1;
        }
        Ok(expired)
    }

    /// Recall **lexical contextual** (#7, BM25-style sobre o índice invertido
    /// dos textos L2/L3): recupera casamentos de termos que o BQ perde.
    /// `dist` = 1 − score normalizado (0 = melhor hit lexical). Default =
    /// memórias ATIVAS (paridade com `recall`); `recall_lexical_historical`
    /// inclui as inativas com `provenance.state` exposto.
    pub fn recall_lexical(&mut self, query_text: &str, k: usize) -> Result<Vec<Hit>, SgdbError> {
        self.recall_lexical_impl(query_text, k, true, None)
    }

    /// Recall lexical incluindo memórias inativas (histórico explícito).
    pub fn recall_lexical_historical(
        &mut self,
        query_text: &str,
        k: usize,
    ) -> Result<Vec<Hit>, SgdbError> {
        self.recall_lexical_impl(query_text, k, false, None)
    }

    /// Recall lexical **escopado** (v1.1.4 item 8): mesmo path de
    /// `recall_lexical`, com o filtro de `scope` dentro do pool de candidatos
    /// (paridade com `recall_scoped`).
    pub fn recall_lexical_scoped(
        &mut self,
        query_text: &str,
        k: usize,
        scope: &str,
    ) -> Result<Vec<Hit>, SgdbError> {
        self.recall_lexical_impl(query_text, k, true, Some(scope))
    }

    /// Recall lexical escopado com histórico explícito.
    pub fn recall_lexical_scoped_historical(
        &mut self,
        query_text: &str,
        k: usize,
        scope: &str,
    ) -> Result<Vec<Hit>, SgdbError> {
        self.recall_lexical_impl(query_text, k, false, Some(scope))
    }

    fn recall_lexical_impl(
        &mut self,
        query_text: &str,
        k: usize,
        active_only: bool,
        scope: Option<&str>,
    ) -> Result<Vec<Hit>, SgdbError> {
        let scored = self.engine.lexical.search(query_text, k.max(1));
        let max = scored.first().map(|(_, s)| *s).unwrap_or(0.0).max(1e-6);
        let mut out = Vec::with_capacity(scored.len());
        for (sk, score) in scored {
            if let Ok(Some(doc)) = self.engine.get_by_storage_key(&sk) {
                let state = self.engine.get_state(&sk);
                if active_only && state != MemoryState::Active {
                    continue;
                }
                // v1.1.4 item 8 — paridade de scope com `recall_impl`: busca
                // lexical global não vaza de scopes escopados. O doc pode
                // ser um companion `/L2/` (sem scope na meta própria) — o
                // scope efetivo vem do primário `/L4/`/`/L5/`/`/L3/`.
                let doc_scope = self.engine.effective_scope(&sk);
                match scope {
                    Some(s) if doc_scope != s => continue,
                    None if !doc_scope.is_empty() => continue,
                    _ => {}
                }
                let provenance = doc.meta.as_ref().map(|m| HitProvenance {
                    memory_id: m.memory_id.clone(),
                    version_id: m.version_id.clone(),
                    layer: doc.layer,
                    state,
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
        self.recall_hybrid_impl(query_emb, query_text, k, None)
    }

    /// Recall híbrido **escopado** (v1.1.4 item 8): semântico + lexical, ambos
    /// restritos ao `scope` (paridade com `recall_scoped`).
    pub fn recall_hybrid_scoped(
        &mut self,
        query_emb: &[f32],
        query_text: &str,
        k: usize,
        scope: &str,
    ) -> Result<Vec<Hit>, SgdbError> {
        self.recall_hybrid_impl(query_emb, query_text, k, Some(scope))
    }

    fn recall_hybrid_impl(
        &mut self,
        query_emb: &[f32],
        query_text: &str,
        k: usize,
        scope: Option<&str>,
    ) -> Result<Vec<Hit>, SgdbError> {
        let mut out = match scope {
            Some(s) => self.recall_scoped(query_emb, k, s)?,
            None => self.recall(query_emb, k)?,
        };
        let mut seen: alloc::collections::BTreeSet<String> =
            out.iter().map(|h| h.key.clone()).collect();
        let lex = match scope {
            Some(s) => self.recall_lexical_scoped(query_text, k.max(1).saturating_mul(4), s)?,
            None => self.recall_lexical(query_text, k.max(1).saturating_mul(4))?,
        };
        for h in lex {
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

    /// Conveniência RAG com oversample explícito (P1-6: teto de bytes).
    pub fn rag_context_oversampled(
        &mut self,
        query: &[f32],
        k: usize,
        oversample: usize,
    ) -> Result<String, SgdbError> {
        self.rag_context_limited(query, k, oversample, crate::limits::MAX_RAG_CONTEXT_BYTES)
    }

    /// RAG context: recall + fetch payload + formato string pro prompt.
    pub fn rag_context(&mut self, query: &[f32], k: usize) -> Result<String, SgdbError> {
        self.rag_context_limited(query, k, 0, crate::limits::MAX_RAG_CONTEXT_BYTES)
    }

    /// RAG context com teto explícito de bytes (`max_bytes`; P1-6).
    ///
    /// Mesmo recall de `rag_context`, mas o contexto acumulado NUNCA excede
    /// `max_bytes` (truncado em fronteira de char, bughunt #7) — um `k` alto
    /// não pode materializar um prompt gigante. `max_bytes=0` = sem teto
    /// (comportamento legado). `oversample=0` usa a heurística padrão.
    pub fn rag_context_limited(
        &mut self,
        query: &[f32],
        k: usize,
        oversample: usize,
        max_bytes: usize,
    ) -> Result<String, SgdbError> {
        let hits = if oversample == 0 {
            self.recall(query, k)?
        } else {
            self.recall_oversampled(query, k, oversample)?
        };
        if hits.is_empty() {
            return Ok(String::new());
        }
        let mut out = format!("[SGDB-RAG top-{}]\n", hits.len());
        for (i, h) in hits.iter().enumerate() {
            if h.text.is_empty() {
                continue;
            }
            let line = format!("  #{}) d={:.4} {}\n", i + 1, h.dist, clamp(&h.text, 200));
            if max_bytes != 0 && out.len() + line.len() > max_bytes {
                // fecha no teto sem ultrapassar; para de acumular. A nota de
                // truncamento só entra se ela própria couber (bughunt: header
                // já consumiu budget — não pode estourar ao anotar).
                let note = "  … (contexto truncado por max_bytes)\n";
                if max_bytes != 0 && out.len() + note.len() <= max_bytes {
                    out.push_str(note);
                }
                break;
            }
            out.push_str(&line);
        }
        // tetos minúsculos: o próprio header pode exceder max_bytes (ex: 1).
        // Garante o contrato "nunca excede" truncando em fronteira de char.
        if max_bytes != 0 && out.len() > max_bytes {
            let cut = out
                .char_indices()
                .take_while(|(i, _)| *i < max_bytes)
                .map(|(i, _)| i)
                .last()
                .unwrap_or(0);
            out.truncate(cut);
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
        let v = self.engine.put(doc)?;
        self.metrics.memory_writes += 1;
        self.metrics.clock_changes += 1;
        Ok(v)
    }

    /// Exporta uma memória como UNIDADE de replicação (P0-5): doc NMD1 (com
    /// meta anexada) + estado lógico + janela de validade. `None` = sem doc
    /// na chave. O lado remoto reimporta com `import_record`/`merge_remote`.
    ///
    /// Fecha a contradição #2: estado/validade são side-tables (`sys/state/`,
    /// `sys/validity/`) que o antigo diff/pull doc-a-doc descartava — agora
    /// viajam com o doc.
    pub fn export_record(&mut self, key: &str) -> Result<Option<MemoryRecord>, SgdbError> {
        let sk = self.resolve_known_key(key);
        let r = self.engine.export_record(&sk)?;
        if r.is_some() {
            self.metrics.replication_sent += 1;
        }
        Ok(r)
    }

    /// Importa uma `MemoryRecord` replicada (P0-5): grava o NMD1 **sem tick
    /// do relógio local** (o receptor não vira autor de memória alheia),
    /// preserva a identidade do criador (`meta`) e aplica estado + validade
    /// que viajam no record. Indexa ART/BQ/lexical como qualquer put.
    pub fn import_record(&mut self, rec: MemoryRecord) -> Result<u64, SgdbError> {
        let v = self.engine.import_record(rec)?;
        self.metrics.replication_received += 1;
        self.metrics.memory_writes += 1;
        Ok(v)
    }

    /// Storage keys cujo relógio tem `counter_of(node) == counter` — o
    /// vínculo versão CRDT ↔ docs (anti-entropy, P0-7). Base do pull
    /// DIRECIONADO por versões faltantes; derivado e reconstruível.
    pub fn keys_for_clock(&self, node: u8, counter: u64) -> Vec<String> {
        self.engine.keys_for_clock(node, counter)
    }

    /// Acesso cru a uma side-table (escape hatch para metadados de
    /// replicação/host, ex: `sys/crdt/` do estado durável — P0-11). NÃO é
    /// uma API pública de leitura de memória.
    pub fn read_side_bytes(&mut self, key: &str) -> Result<Option<Vec<u8>>, SgdbError> {
        self.engine.read_side_bytes(key)
    }

    pub fn write_side_bytes(&mut self, key: &str, bytes: &[u8]) -> Result<(), SgdbError> {
        self.engine.write_side_bytes(key, bytes)
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
        self.metrics.replication_received += 1;
        let policy = MergePolicy::for_layer(rec.doc.layer);
        if !policy.accepts_remote() {
            self.metrics.replication_rejected += 1;
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
                self.metrics.replication_duplicate += 1;
                return Ok(MergeVerdict::Duplicate);
            }
            self.engine.import_record(rec)?;
            return Ok(MergeVerdict::Applied);
        }
        if rec.doc.clock.happens_before(&local.clock) {
            self.metrics.replication_stale += 1;
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
        // CONCORRENTE: nunca descartar memória — preserva o local e REGISTRA
        // o conflito com evidência completa (v0.9, Phase 14): id determinís-
        // tico (re-merge upserta), candidatos + records MDR1 de AMBOS os
        // lados. A resolução re-importa o vencedor sem depender do nó remoto.
        let local_rec = self.engine.export_record(&sk)?;
        let local_vid = local_rec
            .as_ref()
            .and_then(|r| r.doc.meta.as_ref())
            .map(|m| m.version_id.clone())
            .unwrap_or_else(|| String::from("pre-v0.6"));
        let remote_vid = rec
            .doc
            .meta
            .as_ref()
            .map(|m| m.version_id.clone())
            .unwrap_or_else(|| String::from("pre-v0.6"));
        // pares (vid, record) ordenados por vid — candidates e records ficam
        // paralelos (contrato do ConflictRecord)
        let mut pairs: Vec<(String, Vec<u8>)> = Vec::with_capacity(2);
        if let Some(lr) = &local_rec {
            pairs.push((local_vid.clone(), lr.encode()));
        }
        pairs.push((remote_vid.clone(), rec.encode()));
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        pairs.dedup_by(|a, b| a.0 == b.0);
        let mut candidates: Vec<String> = pairs.iter().map(|(v, _)| v.clone()).collect();
        let records: Vec<Vec<u8>> = pairs.iter().map(|(_, r)| r.clone()).collect();
        // nós fonte (autor do doc local + autor do remoto), únicos e ordenados
        let mut nodes: Vec<u8> = Vec::with_capacity(2);
        let local_author = local_rec
            .as_ref()
            .and_then(|r| r.doc.meta.as_ref())
            .map(|m| m.source)
            .unwrap_or(self.engine.node_id);
        for n in [local_author, rec.doc.meta.as_ref().map(|m| m.source).unwrap_or(0)] {
            if !nodes.contains(&n) {
                nodes.push(n);
            }
        }
        nodes.sort();
        let conflict = crate::conflict::ConflictRecord {
            conflict_id: crate::conflict::generate_conflict_id(&sk, &mut candidates),
            subject: sk.clone(),
            candidates,
            nodes,
            created_tick: self.engine.own_counter(),
            status: crate::conflict::ConflictStatus::Open,
            resolved_winner: None,
            records,
        };
        self.engine.put_conflict(&conflict)?;
        self.metrics.conflicts_detected += 1;
        Ok(MergeVerdict::Conflict)
    }

    /// Conflitos persistidos (v0.9, Phase 14/15): a camada superior enumera,
    /// inspeciona evidência e decide. O core detecta/preserva; NUNCA decide.
    pub fn conflicts(&mut self) -> Vec<crate::conflict::ConflictRecord> {
        self.engine.list_conflicts()
    }

    pub fn conflict(&mut self, conflict_id: &str) -> Option<crate::conflict::ConflictRecord> {
        self.engine.get_conflict(conflict_id)
    }

    /// Resolução EXPLÍCITA de conflito (Phase 15): o chamador (camada
    /// cognitiva/arbitração) escolhe o vencedor por `version_id`. Efeitos:
    ///
    /// - vencedor importado (se não for o local) → `Active` no slot;
    /// - perdedor: permanece na história (linhagem + evidência do conflito);
    /// - `parent_ids` do vencedor += perdedor (linhagem registrada);
    /// - conflito marcado `Resolved`.
    ///
    /// O CORE não decide o vencedor — apenas executa a decisão e preserva
    /// evidência. Idempotente (conflito já Resolved = Ok).
    pub fn resolve_conflict(
        &mut self,
        conflict_id: &str,
        winner_vid: &str,
    ) -> Result<(), SgdbError> {
        let Some(mut c) = self.engine.get_conflict(conflict_id) else {
            return Err(SgdbError::Invalid("conflict not found"));
        };
        if c.status == crate::conflict::ConflictStatus::Resolved {
            return Ok(()); // idempotente
        }
        let winner_idx = match c.candidates.iter().position(|v| v == winner_vid) {
            Some(i) => i,
            None => return Err(SgdbError::Invalid("winner not a candidate")),
        };
        // importa o record do VENCEDOR (evidência preservada no conflito — a
        // resolução não depende de re-buscar o nó remoto)
        if let Some(rec_bytes) = c.records.get(winner_idx) {
            if let Ok(rec) = crate::memory_doc::MemoryRecord::decode(rec_bytes) {
                let sk = rec.doc.storage_key();
                self.engine.import_record(rec)?;
                // decisão EXPLÍCITA da camada superior: o vencedor vira a
                // versão CORRENTE do slot (differe do overwrite implícito, que
                // preserva a identidade local); perdedores viram parents
                let mut m = self.engine.ensure_meta(&sk)?;
                m.version_id = String::from(winner_vid);
                for (i, v) in c.candidates.iter().enumerate() {
                    if i != winner_idx && !m.parent_ids.contains(v) {
                        m.parent_ids.push(v.clone());
                    }
                }
                self.engine.write_meta(&sk, &m)?;
            }
        }
        c.status = crate::conflict::ConflictStatus::Resolved;
        c.resolved_winner = Some(String::from(winner_vid));
        self.metrics.conflicts_resolved += 1;
        self.engine.put_conflict(&c)
    }

    /// Remove o REGISTRO do conflito após a camada superior encerrar o
    /// assunto (ex: `merge_memories` consumiu a evidência). A história das
    /// versões permanece via `lineage`/`sys/version/` — só o marcador some.
    pub fn dismiss_conflict(&mut self, conflict_id: &str) -> Result<(), SgdbError> {
        self.engine.delete_conflict(conflict_id)
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
        let sk = self.resolve_known_key(key);
        let existed = self.engine.delete(&sk)?;
        // v1.1.3 S4 — recuperação proativa: delete físico deixa o id no BQ
        // (append-only); quando os órfãos passam do limiar, reempacota na hora.
        if existed {
            self.reclaim_bq_orphans(crate::limits::DEFAULT_BQ_ORPHAN_THRESHOLD);
        }
        Ok(existed)
    }

    /// v1.1.3 S4 — recuperação proativa do BQ (índice derivado, append-only):
    /// remove do flat os ids sem doc vivo (`delete` físico deixa órfãos
    /// inertes — o recall os pula, mas eles inflam o pool de candidatos).
    /// Só age quando os órfãos ≥ `threshold` (`0` = sempre). Retorna quantos
    /// removeu. Idempotente; `Sgdb::delete` chama com o default.
    pub fn reclaim_bq_orphans(&mut self, threshold: usize) -> usize {
        self.engine.reclaim_bq_orphans(threshold)
    }

    // ── L6 associative memory (v0.8, roadmap Phase 12) ────────────────────
    //
    // Relações são memória-NATIVAS: persistidas em `sys/rel/` e indexadas no
    // ART (forward + reverse). NENHUMA inferência — a camada superior afirma,
    // o SGDB armazena. `a`/`b` são storage keys canônicas (`md/Lx/...`).

    /// Afirma `a --kind--> b` (idempotente). Rejeita chaves com `#`.
    pub fn associate(
        &mut self,
        a: &str,
        rel: crate::memory_doc::RelationKind,
        b: &str,
    ) -> Result<(), SgdbError> {
        let a = self.resolve_storage_key(a);
        let b = self.resolve_storage_key(b);
        self.engine.associate(&a, rel, &b)
    }

    /// Variante DEFENSIVA de [`Sgdb::associate`] (AUDIT v1.1 P3): valida que
    /// AMBOS os lados existem (doc vivo na storage key canônica) antes de
    /// afirmar a relação — chave fantasma → `Err`, nenhuma side-table órfã.
    /// O `associate` cru continua sem validar (design: a camada superior
    /// afirma; quem quer segurança usa esta variante).
    pub fn associate_checked(
        &mut self,
        a: &str,
        rel: crate::memory_doc::RelationKind,
        b: &str,
    ) -> Result<(), SgdbError> {
        let a = self.resolve_known_key(a);
        let b = self.resolve_known_key(b);
        if self.engine.art.get(&a).is_none() {
            return Err(SgdbError::Invalid(
                "associate_checked: no memory at key a (use the full canonical storage key, e.g. md/L4/<key>)",
            ));
        }
        if self.engine.art.get(&b).is_none() {
            return Err(SgdbError::Invalid(
                "associate_checked: no memory at key b (use the full canonical storage key, e.g. md/L4/<key>)",
            ));
        }
        self.engine.associate(&a, rel, &b)
    }

    /// Todas as relações envolvendo `key` (saídas E entradas, todos os kinds),
    /// determinístico por (kind, alvo).
    pub fn related_to(&self, key: &str) -> Vec<(crate::memory_doc::RelationKind, String)> {
        self.engine.related_to(key)
    }

    /// `causes(key)` → alvos de `key --causes--> *`.
    pub fn causes(&self, key: &str) -> Vec<String> {
        self.engine.relations_outgoing(RelationKind::Causes, key)
    }

    /// `supports(key)` → alvos de `key --supports--> *`.
    pub fn supports(&self, key: &str) -> Vec<String> {
        self.engine.relations_outgoing(RelationKind::Supports, key)
    }

    /// `contradicts(key)` → alvos de `key --contradicts--> *`.
    pub fn contradicts(&self, key: &str) -> Vec<String> {
        self.engine.relations_outgoing(RelationKind::Contradicts, key)
    }

    /// `derived_from(key)` → alvos de `key --derived_from--> *` (linhagem
    /// semântica; a consolidação escreve esta relação ao derivar).
    pub fn derived_from(&self, key: &str) -> Vec<String> {
        self.engine.relations_outgoing(RelationKind::DerivedFrom, key)
    }

    /// Lookup ART por prefixo de storage key (ex: "md/L1/").
    pub fn scan_prefix(&mut self, prefix: &str) -> Result<Vec<(String, u64)>, SgdbError> {
        Ok(self.engine.art.scan_prefix(prefix))
    }

    /// Página de `scan_prefix` com ordem lexicográfica determinística (P1-6).
    ///
    /// `offset` crescente (0, 100, 200, …) percorre o prefixo sem materializar
    /// tudo de uma vez; `limit=0` ou `offset` além do fim devolve vazio.
    /// `scan_prefix` (sem página) mantém a ordem de travessia da árvore.
    pub fn scan_prefix_page(
        &mut self,
        prefix: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<(String, u64)>, SgdbError> {
        Ok(self.engine.art.scan_prefix_page(prefix, offset, limit))
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

    pub(crate) fn engine_mut(&mut self) -> &mut AiosDatabaseEngine {
        &mut self.engine
    }

    /// Estado observável da instância (P2-3). Substitui o `ready()` v0.1
    /// (que retornava `true` hardcoded — o engine sempre tem storage).
    pub fn health(&mut self) -> HealthReport {
        let storage_ok = {
            let probe = self.engine.scan_prefix_storage(b"md/");
            probe.is_ok()
        };
        let open_conflicts = self.engine.list_conflicts().len();
        HealthReport {
            backend: self.engine.backend_name(),
            node_id: self.engine.node_id,
            storage_ok,
            doc_count: self.engine.art.scan_prefix("md/").len(),
            bq_len: self.engine.bq_len(),
            ram_len: self.engine.ram_l0l1_len(),
            open_conflicts,
        }
    }

    /// Checks de integridade agregados (P2-3): caminha o storage `md/`
    /// (fonte da verdade), decodifica cada NMD1 e cruza com os índices
    /// derivados (ART/BQ) e com as side-tables (`sys/state|validity|meta`).
    /// Retorna TODOS os issues encontrados — vazio = saudável.
    pub fn validate(&mut self) -> Vec<ValidateIssue> {
        let mut issues: Vec<ValidateIssue> = Vec::new();

        // 1. storage: docs `md/` decodificam e estão no ART
        let md = match self.engine.scan_prefix_storage(b"md/") {
            Ok(md) => md,
            Err(e) => {
                issues.push(ValidateIssue {
                    key: "md/".into(),
                    message: match e {
                        SgdbError::Corrupt => "storage corrupt",
                        SgdbError::Storage(m) => m,
                        SgdbError::Invalid(m) => m,
                    },
                });
                return issues;
            }
        };
        for (sk, bytes) in &md {
            let sk = String::from_utf8_lossy(sk).into_owned();
            if MemoryDoc::decode(bytes).is_err() {
                issues.push(ValidateIssue {
                    key: sk.clone(),
                    message: "NMD1 doc does not decode",
                });
            }
            if self.engine.art.get(&sk).is_none() {
                issues.push(ValidateIssue {
                    key: sk,
                    message: "doc missing from ART index",
                });
            }
        }

        // 2. índices derivados: contagens BATEM com o storage
        // O BQ indexa L4 (semântica) E L5 (procedural) — engine.rs put_inner
        // (AUDIT 3.5: um doc L5 legítimo com embedding quebrava o validate,
        // que só contava md/L4/). Regra replicada: layer ∈ {L4, L5} E
        // (bitvec presente OU payload com ≥4 bytes — reinterpretado como f32).
        let bq_doc_count = md.iter().filter(|(sk, bytes)| {
            if !(sk.starts_with(b"md/L4/") || sk.starts_with(b"md/L5/")) {
                return false;
            }
            match MemoryDoc::decode(bytes) {
                Ok(d) => d.bitvec.is_some() || d.payload.len() >= 4,
                Err(_) => false, // já flaggado em 1 — não conta
            }
        }).count();
        if self.engine.bq_len() != bq_doc_count {
            issues.push(ValidateIssue {
                key: "md/L4/L5/".into(),
                message: "BQ index count != L4+L5 embedding doc count",
            });
        }

        // 3. side-tables não órfãs: cada sys/state|validity|meta aponta para
        // um doc `md/` que existe (o inverso é permitido: doc sem meta =
        // pré-v0.6, meta lazy).
        for prefix in ["sys/state/", "sys/validity/", "sys/meta/"] {
            if let Ok(rows) = self.engine.scan_prefix_storage(prefix.as_bytes()) {
                for (sk, _) in rows {
                    let target = String::from_utf8_lossy(&sk[prefix.len()..]).into_owned();
                    if !target.starts_with("md/") {
                        continue; // side-table não-doc (reservada) — fora do check
                    }
                    match self.engine.storage_get(target.as_bytes()) {
                        Ok(Some(_)) => {}            // doc existe — ok
                        Ok(None) => {
                            issues.push(ValidateIssue {
                                key: target,
                                message: "side-table targets missing doc",
                            });
                        }
                        Err(_) => {
                            issues.push(ValidateIssue {
                                key: target,
                                message: "storage read failed during validate",
                            });
                        }
                    }
                }
            }
        }

        issues
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
    } else {
        key.strip_prefix("ts/")?
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
pub(crate) fn sqrt_f32(x: f32) -> f32 {
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
    use alloc::string::ToString; // no_std test build
    use alloc::vec; // no_std test build: `vec!` não está no prelude
    use crate::storage::InMemory;

    #[test]
    fn exchange_and_prompt() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_exchange("oi", "ola!").unwrap();
        let l1 = db.scan_prefix("md/L1/").unwrap();
        let l2 = db.scan_prefix("md/L2/").unwrap();
        assert!(!l1.is_empty());
        assert!(!l2.is_empty());
    }

    #[test]
    fn episodic_verbatim_roundtrip() {
        // v1.1.4 item 2 (mempalace): remember_episodic guarda o par cru em
        // L2 timestamped, sem extração — e devolve as storage keys completas
        // (md/L2/<ts>/u e /a) para follow-up (explain/reinforce/etc).
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let (ku, ka) = db.remember_episodic("qual e o capital?", "e Roma", 1700000000000u64).unwrap();
        assert!(ku.starts_with("md/L2/"), "chave user deve ser md/L2/<ts>/u");
        assert!(ku.ends_with("/u"));
        assert!(ka.starts_with("md/L2/"));
        assert!(ka.ends_with("/a"));
        // texto cru recuperável por chave (verbatim — nada foi resumido)
        let u = db.get(MemoryLayer::L2EpisodicShort, ku.trim_start_matches("md/L2/")).unwrap().unwrap();
        let a = db.get(MemoryLayer::L2EpisodicShort, ka.trim_start_matches("md/L2/")).unwrap().unwrap();
        assert_eq!(String::from_utf8_lossy(&u.payload), "qual e o capital?");
        assert_eq!(String::from_utf8_lossy(&a.payload), "e Roma");
        // lexical (BM25) sobre L2 encontra o verbatim
        let lex = db.recall_lexical("capital", 5).unwrap();
        assert!(lex.iter().any(|h| h.text == "qual e o capital?"), "lexical deve achar episodio verbatim");
        // episódios distintos não colidem (ts diferentes)
        let (ku2, _) = db.remember_episodic("e a populacao?", "600 mil", 1700000001000u64).unwrap();
        assert_ne!(ku, ku2);
    }

    #[test]
    fn diary_filters_by_agent_and_orders_recent_first() {
        // v1.1.4 item 4 (mempalace): diary(node_id) devolve SÓ as L2 do agente,
        // mais recentes primeiro (keys ts sortable revertidas).
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_episodic("pergunta antiga", "resposta antiga", 1000).unwrap();
        db.remember_episodic("pergunta nova", "resposta nova", 2000).unwrap();
        let me = db.node_id();
        // todas escritas por mim (source == node_id local); 2 episódios × 2 docs (u+a)
        let d = db.diary(me, 10).unwrap();
        assert_eq!(d.len(), 4);
        // mais recente primeiro
        assert!(d[0].0.ends_with("/u") || d[0].0.ends_with("/a"));
        // o par novo (ts 2000) deve vir antes do par antigo (ts 1000)
        let ts_new: Vec<&String> = d.iter().map(|(k, _)| k).collect();
        let pos_new = ts_new.iter().position(|k| k.contains("00000000000007d0")).unwrap(); // 2000
        let pos_old = ts_new.iter().position(|k| k.contains("00000000000003e8")).unwrap(); // 1000
        assert!(pos_new < pos_old, "mais recente primeiro");
        // limit respeitado
        let d1 = db.diary(me, 1).unwrap();
        assert_eq!(d1.len(), 1);
        // agente que nunca escreveu → vazio
        let other = db.diary(me.wrapping_add(1), 10).unwrap();
        assert!(other.is_empty());
    }

    #[test]
    fn profile_aggregates_stable_facts_by_importance() {
        // v1.1.4 item 5 (supermemory): profile(node_id) agrega fatos L3/L4/L5
        // do agente, ordenados por importância desc, truncados ao limit.
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("kFatoA", "gosta de Rust", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        db.remember_semantic("kFatoB", "trabalha com IAs", &[1.0, 1.0, 1.0, 1.0]).unwrap();
        db.reinforce("kFatoA", 0.6).unwrap(); // A mais importante
        let me = db.node_id();
        let p = db.profile(me, 10).unwrap();
        assert_eq!(p.len(), 2, "L4 dos 2 fatos");
        // importância desc: kFatoA (1.0 após reinforce 0.6 sobre default 1.0... clamp) antes de B
        assert_eq!(p[0].0, "md/L4/kFatoA");
        assert_eq!(p[1].0, "md/L4/kFatoB");
        // payload verbatim
        assert_eq!(p[0].3, "gosta de Rust");
        // limit
        let p1 = db.profile(me, 1).unwrap();
        assert_eq!(p1.len(), 1);
        assert_eq!(p1[0].0, "md/L4/kFatoA");
        // agente desconhecido → vazio
        assert!(db.profile(me.wrapping_add(5), 10).unwrap().is_empty());
    }

    #[test]
    fn expire_old_invalidates_expired_windows() {
        // v1.1.4 item 6 (supermemory): expire_old(now) marca Invalidated as
        // memórias cuja janela fechou em now — recall default (active-only)
        // as ignora; história preservada via recall_historical. Idempotente.
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("kPermanente", "regra fixa", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        db.remember_semantic("kTemporaria", "noticia de hoje", &[1.0, 1.0, 1.0, 1.0]).unwrap();
        // janela da temporária fecha em 2000
        db.set_validity("kTemporaria", 1000, 2000).unwrap();
        assert_eq!(db.expire_old(1500).unwrap(), 0, "nada expirou ainda");
        assert_eq!(db.expire_old(2000).unwrap(), 1, "a temporária expirou em now=2000");
        assert_eq!(db.expire_old(3000).unwrap(), 0, "idempotente: não re-marca");
        assert_eq!(db.get_state("kTemporaria").unwrap(), MemoryState::Invalidated);
        assert_eq!(db.get_state("kPermanente").unwrap(), MemoryState::Active, "sem janela = sempre válida");
        // recall_at com now após o fim exclui a expirada
        let q = [1.0, 1.0, 1.0, 1.0];
        let at_future = db.recall_at(&q, 10, 3000).unwrap();
        assert!(at_future.iter().all(|h| h.key != "md/L4/kTemporaria"), "expirada fora do recall_at");
        let hist = db.recall_historical(&q, 10).unwrap();
        assert!(hist.iter().any(|h| h.key == "md/L4/kTemporaria"), "história preservada (recall_historical)");
    }

    #[test]
    fn scoped_recall_isolates_tenants() {
        // v1.1.4 item 7 (mem0 multi-tenancy): scope particiona por
        // user/agent/projeto. Filtro DENTRO do pool de candidatos — a memória
        // do tenant A nunca compete por vagas do top-k do tenant B.
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("kA", "preferencia da ana: cafe", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        db.remember_semantic("kB", "preferencia do bruno: cha", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        db.remember_semantic("kG", "fato global compartilhado", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        db.set_scope("kA", "user/ana").unwrap();
        db.set_scope("kB", "user/bruno").unwrap();
        let q = [1.0, -1.0, 1.0, -1.0];
        // recall global (sem scope) = só memórias sem marcação (global default)
        let g = db.recall(&q, 10).unwrap();
        assert_eq!(g.len(), 1, "recall global não vaza de scopes");
        assert_eq!(g[0].key, "md/L4/kG");
        // recall escopado = só o tenant
        let a = db.recall_scoped(&q, 10, "user/ana").unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].key, "md/L4/kA");
        let b = db.recall_scoped(&q, 10, "user/bruno").unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].key, "md/L4/kB");
        // tenant que nunca escreveu → vazio
        assert!(db.recall_scoped(&q, 10, "user/carla").unwrap().is_empty());
        // scope_of / meta expõem o campo
        assert_eq!(db.scope_of("kA").unwrap(), "user/ana");
        assert_eq!(db.scope_of("kG").unwrap(), "", "sem marcação = global");
    }

    #[test]
    fn retrieval_modes_respect_scope_and_dispatch() {
        // v1.1.4 item 8 (cognee search_type): lexical e híbrido são expostos
        // como modos no MCP e honram o mesmo filtro de scope do recall — a
        // busca global não vaza de scopes em NENHUM path, e o modo escopado
        // isola o tenant.
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
        db.remember_semantic("kA", "ana prefere cafe", &emb(1)).unwrap();
        db.remember_semantic("kB", "bruno prefere cha", &emb(2)).unwrap();
        db.remember_semantic("kG", "fato global: cafe eh bebida", &emb(3)).unwrap();
        db.set_scope("kA", "user/ana").unwrap();
        db.set_scope("kB", "user/bruno").unwrap();
        // lexical global não vaza de scopes
        let lg = db.recall_lexical("cafe", 10).unwrap();
        assert!(lg.iter().all(|h| !h.key.contains("/kA") && !h.key.contains("/kB")),
            "lexical global vazou de scopes: {:?}", lg);
        assert!(lg.iter().any(|h| h.key.contains("/kG")), "lexical global deveria achar o fato global");
        // lexical escopado isola o tenant
        let la = db.recall_lexical_scoped("cafe", 10, "user/ana").unwrap();
        assert_eq!(la.len(), 1, "só ana tem 'cafe' marcada: {:?}", la);
        assert!(la[0].key.contains("/kA"));
        // híbrido global também não vaza
        let hg = db.recall_hybrid(&emb(1), "cafe", 10).unwrap();
        assert!(hg.iter().all(|h| !h.key.contains("/kB")), "híbrido global vazou de scopes: {:?}", hg);
        // híbrido escopado isola
        let ha = db.recall_hybrid_scoped(&emb(1), "cafe", 10, "user/ana").unwrap();
        assert!(ha.iter().all(|h| h.key.contains("/kA")), "híbrido escopado vazou: {:?}", ha);
        // histórico escopado inclui inativas do tenant (não outras)
        let hist = db.recall_lexical_scoped_historical("cha", 10, "user/bruno").unwrap();
        assert!(hist.iter().all(|h| h.key.contains("/kB")), "histórico escopado vazou: {:?}", hist);
    }

    #[test]
    fn feedback_reweights_importance_and_confidence() {
        // v1.1.4 item 3 (cognee improve): feedback positivo sobe importância
        // E confiança; negativo desce ambas; clamp [0,1]; amount não-finita
        // rejeitada (mesmo contrato de set_importance).
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("kf", "dica util", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        let before = db.meta("kf").unwrap().unwrap();
        assert_eq!(before.importance, 1.0, "L4 default importance = 1.0");
        assert_eq!(before.confidence, 1.0, "default de confiança é 1.0 (meta_for_import)");
        db.feedback("kf", true, 0.3).unwrap();
        let pos = db.meta("kf").unwrap().unwrap();
        assert_eq!(pos.importance, 1.0, "já no teto — clamp mantém 1.0");
        assert_eq!(pos.confidence, 1.0, "já no teto — clamp mantém 1.0");
        // negativo desce ambos
        db.feedback("kf", false, 0.5).unwrap();
        let neg = db.meta("kf").unwrap().unwrap();
        assert_eq!(neg.importance, 0.5, "importância desce com feedback -");
        assert_eq!(neg.confidence, 0.5, "confiança desce com feedback -");
        // clamp: feedback + repetido nunca passa de 1.0
        for _ in 0..10 {
            db.feedback("kf", true, 0.5).unwrap();
        }
        let top = db.meta("kf").unwrap().unwrap();
        assert_eq!(top.importance, 1.0);
        assert_eq!(top.confidence, 1.0);
        assert!(matches!(db.feedback("kf", true, f32::NAN),
            Err(SgdbError::Invalid(_))));
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
        assert!(!l3.is_empty());
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
        assert!(!l1.is_empty());
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
        assert!(!l1.is_empty());
        let l3 = db.scan_prefix("md/L3/").unwrap();
        assert!(!l3.is_empty());
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

    // ── P1-6: paginação scan_prefix + teto rag_context ──

    #[test]
    fn scan_prefix_page_is_lexicographic_and_partitioning() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        // chaves de largura fixa (regra 4: ART não suporta prefix-key)
        for i in 0..25 {
            db.remember_fact(&format!("fato {:02}", i), i as u64 + 1).unwrap();
        }
        let all = db.scan_prefix("md/L3/").unwrap();
        assert_eq!(all.len(), 25);

        // scan_prefix (legado) pode vir em ordem de travessia da árvore;
        // scan_prefix_page SEMPRE devolve ordem lexicográfica determinística.
        let page1 = db.scan_prefix_page("md/L3/", 0, 10).unwrap();
        let page2 = db.scan_prefix_page("md/L3/", 10, 10).unwrap();
        let page3 = db.scan_prefix_page("md/L3/", 20, 10).unwrap();
        assert_eq!(page1.len(), 10);
        assert_eq!(page2.len(), 10);
        assert_eq!(page3.len(), 5);
        assert!(page1.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(page2.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(page3.windows(2).all(|w| w[0].0 < w[1].0));
        // páginas não sobrepõem nem pulam itens
        let mut joined: Vec<String> = page1
            .into_iter()
            .chain(page2)
            .chain(page3)
            .map(|(k, _)| k)
            .collect();
        joined.sort();
        let mut expected: Vec<String> = all.iter().map(|(k, _)| k.clone()).collect();
        expected.sort();
        assert_eq!(joined, expected);

        // limites: offset além do fim / limit 0 / offset+limit parciais
        assert!(db.scan_prefix_page("md/L3/", 25, 10).unwrap().is_empty());
        assert!(db.scan_prefix_page("md/L3/", 0, 0).unwrap().is_empty());
        assert_eq!(db.scan_prefix_page("md/L3/", 23, 10).unwrap().len(), 2);
        assert!(db.scan_prefix_page("md/Lx/", 0, 10).unwrap().is_empty());
    }

    #[test]
    fn rag_context_respects_max_bytes() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let emb = vec![1.0f32; 64];
        for i in 0..5 {
            let mut e = emb.clone();
            e[i] = -1.0;
            db.remember_semantic(&format!("k{:02}", i), &format!("resposta longa numero {:02} do banco", i), &e)
                .unwrap();
        }

        let big = db.rag_context_limited(&emb, 5, 0, 210).unwrap();
        assert!(big.len() <= 210);
        assert!(big.contains("truncado por max_bytes"));
        assert!(big.contains("#1"));

        let huge = db.rag_context_limited(&emb, 5, 0, 0).unwrap();
        assert!(huge.len() > 200);
        assert!(!huge.contains("truncado por max_bytes"));

        // default = MAX_RAG_CONTEXT_BYTES, nunca estoura
        let dflt = db.rag_context(&emb, 5).unwrap();
        assert!(dflt.len() <= crate::limits::MAX_RAG_CONTEXT_BYTES);

        // teto apertado: ainda abre o header sem ultrapassar
        let tiny = db.rag_context_limited(&emb, 5, 0, 1).unwrap();
        assert!(tiny.len() <= 1);
    }

    // ── P1-7: prefix-key rejeitado na borda da API ──

    #[test]
    fn prefix_key_rejected_at_api_boundary() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let emb = vec![1.0f32, -1.0, 1.0, -1.0];
        db.remember_semantic("k", "texto", &emb).unwrap();
        // "md/L4/k" é prefixo de "md/L4/kx" → rejeita (regra 4)
        let e = db.remember_semantic("kx", "texto2", &emb).unwrap_err();
        assert!(matches!(e, SgdbError::Invalid(_)));
        // ordem inversa: nova chave é prefixo da existente → rejeita
        let mut db2 = Sgdb::open(InMemory::new()).unwrap();
        db2.remember_semantic("kx", "texto2", &emb).unwrap();
        let e = db2.remember_semantic("k", "texto", &emb).unwrap_err();
        assert!(matches!(e, SgdbError::Invalid(_)));
        // overwrite da MESMA chave continua válido (não é prefix-key)
        db.remember_semantic("k", "texto-novo", &emb).unwrap();
        let hits = db.recall(&emb, 5).unwrap();
        assert!(hits.iter().any(|h| h.key.contains("/L4/k")));
        // chaves não-prefixo (ex: `ts/` hex fixo, exchanges) nunca conflitam
        db.remember_exchange("user", "resp").unwrap();
        db.remember_fact("fato", 42).unwrap();
    }

    #[test]
    fn relation_prefix_key_rejected() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        use crate::memory_doc::RelationKind;
        db.associate("md/L4/a", RelationKind::Causes, "md/L4/b").unwrap();
        // "rel/causes/md/L4/a#md/L4/b" é prefixo de "…#md/L4/bc" → rejeita
        let e = db
            .associate("md/L4/a", RelationKind::Causes, "md/L4/bc")
            .unwrap_err();
        assert!(matches!(e, SgdbError::Invalid(_)));
        // mesma relação (idempotente) continua ok
        db.associate("md/L4/a", RelationKind::Causes, "md/L4/b").unwrap();
        assert_eq!(db.causes("md/L4/a"), vec!["md/L4/b".to_string()]);
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
            // chave fixed-width (regra 4: `d1` seria prefixo de `d10`…`d19`)
            db.remember_semantic(&format!("d{i:02}"), &format!("doc {i}"), &emb).unwrap();
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
    fn recall_weighted_total_cmp_nan_does_not_break_order() {
        // P1-1: pesos NaN devem produzir ordem total definida (total_cmp),
        // nunca panic nem empate falso via partial_cmp.unwrap_or(Equal).
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        for i in 0..5 {
            let emb = [1.0, -1.0, (i as f32) / 4.0, -1.0];
            db.remember_semantic(&format!("d{i}"), &format!("doc {i}"), &emb).unwrap();
        }
        let q = [1.0, -1.0, 0.5, -1.0];
        let res = db.recall_weighted(&q, 3, f32::NAN, 0.0, 0.0, 1_000_000).unwrap();
        assert_eq!(res.len(), 3);
        // total_cmp é total: repetir dá a mesma ordem
        let res2 = db.recall_weighted(&q, 3, f32::NAN, 0.0, 0.0, 1_000_000).unwrap();
        for (a, b) in res.iter().zip(res2.iter()) {
            assert_eq!(a.key, b.key);
        }
    }

    #[test]
    fn embedding_policy_rejects_nonfinite_and_oversized() {
        // P1-1: NaN/Inf/dim>MAX são erros de entrada, não corrupção silenciosa
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let err_nan = db.remember_semantic("k", "t", &[1.0, f32::NAN]);
        assert!(err_nan.is_err(), "NaN deve ser rejeitado");
        let err_inf = db.remember_semantic("k", "t", &[1.0, f32::INFINITY]);
        assert!(err_inf.is_err(), "Inf deve ser rejeitado");
        let err_neg = db.remember_semantic("k", "t", &[1.0, f32::NEG_INFINITY]);
        assert!(err_neg.is_err(), "-Inf deve ser rejeitado");
        let mut big = vec![0.0f32; crate::bq::MAX_EMBEDDING_DIM + 1];
        big[0] = 1.0;
        let err_big = db.remember_semantic("k", "t", &big);
        assert!(err_big.is_err(), "dim acima do MAX deve ser rejeitado");
        let err_empty = db.remember_semantic("k", "t", &[]);
        assert!(err_empty.is_err(), "emb vazio deve ser rejeitado");
        // nada foi gravado
        assert!(db.scan_prefix("md/L4/").unwrap().is_empty());
    }

    #[test]
    fn recall_rejects_nonfinite_query() {
        // P1-1: query NaN/Inf → Err em vez de ranking corrupto (score 0)
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("k", "t", &[1.0, -1.0]).unwrap();
        assert!(db.recall(&[f32::NAN, 0.0], 3).is_err());
        assert!(db.recall(&[f32::INFINITY, 0.0], 3).is_err());
        // query vazia continua sendo no-op (sem resultado, sem erro)
        assert!(db.recall(&[], 3).unwrap().is_empty());
        // query oversize → Err
        let mut big = vec![0.0f32; crate::bq::MAX_EMBEDDING_DIM + 1];
        big[0] = 1.0;
        assert!(db.recall(&big, 3).is_err());
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
            // fixed-width (regra 4: `d1` seria prefixo de `d10`…`d1999`)
            db.remember_semantic(&format!("d{i:04}"), &format!("doc {i}"), &emb16(i as u64))
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
            assert!(!db.scan_prefix("md/L1/").unwrap().is_empty());
            assert!(!db.scan_prefix("md/L3/").unwrap().is_empty());
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

    #[test]
    fn recall_weighted_uses_doc_importance_not_layer() {
        // AUDIT (v1.1 P2): recall_weighted ponderava importância POR CAMADA
        // (L4=0.0, L5=0.2) mesmo quando o DOC tinha `set_importance`/
        // `reinforce` explícitos — o nome prometia importância da memória,
        // o contrato entregava a da camada. Agora usa
        // `Hit.provenance.importance` (meta do doc); pré-v0.6 sem meta cai
        // para a default da camada.
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let emb = vec![1.0, -1.0, 1.0, -1.0];
        db.remember_semantic("a", "alvo", &emb).unwrap();
        db.remember_semantic("b", "alvo", &emb).unwrap();
        db.set_importance("md/L4/a", 0.9).unwrap();
        db.set_importance("md/L4/b", 0.1).unwrap();
        // mesma camada L4 + mesmo embedding → antes EMPATAVAM por camada e o
        // tie-break de key decidia; agora a importância do doc decide.
        let r = db.recall_weighted(&emb, 2, 0.0, 0.0, 1.0, 0).unwrap();
        assert_eq!(r.len(), 2);
        assert!(
            r[0].key.contains("/a"),
            "doc com importância 0.9 deveria vencer 0.1: {}",
            r[0].key
        );
        // hit expõe a importância do doc na provenance
        assert!((r[0].provenance.as_ref().unwrap().importance - 0.9).abs() < 1e-6);
        // doc recém-criado (put cru) tem meta default importance 1.0 (v0.6+):
        // sob w_imp, o doc com importância 0.9 ainda vence o "b" (0.1) e o cru
        // (1.0) — importância por DOC, não por camada.
        let mut raw = crate::memory_doc::MemoryDoc::new(
            crate::memory_doc::MemoryLayer::L4Semantic,
            "raw/cru",
            b"alvo".to_vec(),
        );
        raw.bitvec = Some(crate::bq::quantize_f32(&emb));
        db.engine.put(raw).unwrap();
        let r2 = db.recall_weighted(&emb, 3, 0.0, 0.0, 1.0, 0).unwrap();
        assert_eq!(r2.len(), 3);
        // ordenação por importância do DOC (penalty = 1−imp): cru (1.0) >
        // a (0.9) > b (0.1)
        assert!(
            r2[0].key.contains("raw/cru"),
            "cru (importância default 1.0) deveria vencer: {}",
            r2[0].key
        );
        assert!(
            r2[1].key.contains("/a"),
            "a (0.9) deveria vir antes de b (0.1): {}",
            r2[1].key
        );
        assert!(
            r2[2].key.contains("/b"),
            "b (0.1) deveria ser o último: {}",
            r2[2].key
        );
        let b = r2.iter().find(|h| h.key.contains("/b")).unwrap();
        assert!(
            (b.provenance.as_ref().unwrap().importance - 0.1).abs() < 1e-6,
            "b mantém importância 0.1 na provenance"
        );
    }

    #[test]
    fn recall_dim_mismatch_is_loud_not_silent() {
        // v1.1.3 S1: recall com query de dimensionalidade que NÃO casa com
        // NENHUMA dim indexada devolvia ruído de hamming (contrato P4: mesmo
        // modelo na gravação e na busca — 4-dim ≠ 256-dim não casa). Agora
        // devolve erro auto-explicativo em vez de silêncio.
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let emb = vec![1.0, -1.0, 1.0, -1.0];
        db.remember_semantic("a", "alvo", &emb).unwrap();
        // query com dim compatível → recall normal, não erro
        let r = db.recall(&emb, 3).unwrap();
        assert_eq!(r.len(), 1);
        assert!(r[0].key.contains("/a"));
        // query de dim INCOMPATÍVEL (256-dim demo) contra docs 4-dim → erro
        let wrong: Vec<f32> = (0..256).map(|i| (i as f32 / 256.0) - 0.5).collect();
        let err = db.recall(&wrong, 3).unwrap_err();
        match err {
            SgdbError::Invalid(msg) => {
                assert!(
                    msg.contains("dimensionality"),
                    "mensagem deve citar dimensionalidade: {}",
                    msg
                );
            }
            other => panic!("esperava Invalid, veio {:?}", other),
        }
        // accessor expõe as dims indexadas para o caller debugar
        assert_eq!(db.indexed_embedding_dims(), vec![4]);
        // recall_historical (mesmo caminho) também avisa
        let err2 = db.recall_historical(&wrong, 3).unwrap_err();
        assert!(matches!(err2, SgdbError::Invalid(_)));
    }

    #[test]
    fn recall_companion_texts_batch_parity() {
        // v1.1.3 S3: o recall preenchia `Hit.text` com um `get_by_storage_key`
        // por hit (N×2 reads: NMD1 + meta). Agora os companions L2 são lidos
        // em batch (1 passada, sem attach_meta). Paridade de contrato: os
        // textos devem ser IDÊNTICOS aos docs L2 companions.
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let emb = vec![1.0, -1.0, 1.0, -1.0];
        db.remember_semantic("a", "texto companion A", &emb).unwrap();
        db.remember_semantic("b", "texto companion B", &emb).unwrap();
        let r = db.recall(&emb, 3).unwrap();
        // ambos os companions presentes, sem perder texto nem ordem
        assert_eq!(r.len(), 2);
        let by_key: alloc::collections::BTreeMap<&str, &str> =
            r.iter().map(|h| (h.key.as_str(), h.text.as_str())).collect();
        assert_eq!(by_key.get("md/L4/a").copied(), Some("texto companion A"));
        assert_eq!(by_key.get("md/L4/b").copied(), Some("texto companion B"));
        // companion ausente → texto vazio (nunca panic, paridade com o get antigo)
        db.remember_semantic("c", "texto C", &emb).unwrap();
        let mut raw = crate::memory_doc::MemoryDoc::new(
            crate::memory_doc::MemoryLayer::L4Semantic,
            "sem-companion",
            b"x".to_vec(),
        );
        raw.bitvec = Some(crate::bq::quantize_f32(&emb));
        db.engine.put(raw).unwrap();
        let r2 = db.recall(&emb, 5).unwrap();
        let h = r2
            .iter()
            .find(|h| h.key.contains("sem-companion"))
            .expect("doc cru presente");
        assert_eq!(h.text, "", "sem companion L2 → texto vazio");
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn recall_dim_mismatch_survives_rebuild() {
        // v1.1.3 S1 (rebuild): `indexed_dims` é derivado e reconstruído do
        // storage (não é estado durável) — remontar a DB não perde a detecção.
        let emb = vec![1.0, -1.0, 1.0, -1.0];
        let wrong: Vec<f32> = (0..256).map(|i| (i as f32 / 256.0) - 0.5).collect();
        let dir = std::env::temp_dir().join(format!("nsgdb_dim_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut db2 = Sgdb::open(crate::FileStorage::open(dir.join("mem.db")).unwrap()).unwrap();
        db2.remember_semantic("a", "alvo", &emb).unwrap();
        drop(db2);
        let mut db3 = Sgdb::open(crate::FileStorage::open(dir.join("mem.db")).unwrap()).unwrap();
        let err3 = db3.recall(&wrong, 3).unwrap_err();
        assert!(matches!(err3, SgdbError::Invalid(_)), "rebuild perdeu indexed_dims");
        assert_eq!(db3.indexed_embedding_dims(), vec![4]);
        let _ = std::fs::remove_dir_all(&dir);
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
            assert!(!db.scan_prefix("md/L3/").unwrap().is_empty());

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
        assert!(!db.scan_prefix("md/L4/").unwrap().is_empty());

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

    #[test]
    fn reclaim_bq_orphans_recompacts_after_delete() {
        // v1.1.3 S4: o BQ é append-only — delete físico deixa o id no flat
        // (inofensivo mas infla o pool de candidatos). A recuperação proativa
        // reempacota acima do limiar; o recall continua correto após.
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let emb = [1.0, -1.0, 1.0, -1.0];
        let mut keys = Vec::new();
        // mais que o limiar default (64) de órfãos em potencial
        for i in 0..70 {
            let k = format!("m{:03}", i); // largura fixa: ART rejeita prefix-keys
            db.remember_semantic(&k, &format!("texto {}", i), &emb).unwrap();
            keys.push(format!("md/L4/{}", k));
        }
        let before = db.engine.bq_len();
        assert!(before >= 70, "BQ deveria ter os 70 docs, tem {}", before);

        // deleta TODOS: cada delete remove de id_to_sk; o BQ acumula órfãos.
        for k in &keys {
            db.delete(k).unwrap();
        }
        // recuperação proativa disparou ao cruzar o limiar (64): reempacotou
        // na hora, deixando a cauda de 6 órfãos (< limiar, sem novo disparo).
        // churn delimitado: reempacotar a cada delete seria O(N) sempre.
        assert_eq!(db.engine.bq_len(), 70 - crate::limits::DEFAULT_BQ_ORPHAN_THRESHOLD);
        // chamada manual com threshold 0 zera o resto
        assert_eq!(db.reclaim_bq_orphans(0), 6);
        assert_eq!(db.engine.bq_len(), 0, "BQ deveria ter sido recompactado");
        // órfãos zero → método idempotente, devolve 0
        assert_eq!(db.reclaim_bq_orphans(0), 0);
        // recall não vê nada (não ressuscita) e não quebra com BQ vazio
        let hits = db.recall(&emb, 5).unwrap();
        assert!(hits.is_empty(), "deletados ressuscitaram: {:?}", hits);

        // re-add: id NOVO, índice volta a funcionar
        db.remember_semantic("novo", "texto novo", &emb).unwrap();
        let hits = db.recall(&emb, 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].key.ends_with("/novo"));

        // abaixo do limiar: não reempacota (0 removidos, pool preservado)
        let mut db2 = Sgdb::open(InMemory::new()).unwrap();
        db2.remember_semantic("a", "x", &emb).unwrap();
        db2.remember_semantic("b", "y", &emb).unwrap();
        assert!(db2.delete("md/L4/a").unwrap());
        // 1 órfão < 64 → nada a fazer (delete com limiar alto preserva)
        assert_eq!(db2.engine.bq_len(), 2);
        assert_eq!(db2.reclaim_bq_orphans(64), 0);
        // chamada manual com threshold 0 reempacota mesmo com 1 órfão
        assert_eq!(db2.reclaim_bq_orphans(0), 1);
        assert_eq!(db2.engine.bq_len(), 1);
        let hits = db2.recall(&emb, 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].key.ends_with("/b"));
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

    #[test]
    fn set_state_rejects_ghost_key_no_orphan_side_table() {
        // AUDIT (battery 1): set_state em chave fantasma criava side-table
        // `sys/state/` órfã (validate flagga "side-table targets missing doc").
        // Active é remove-only (inócuo) — supersede marca new→Active antes de
        // o new existir; os demais estados recusam chave sem doc.
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        for st in [MemoryState::Superseded, MemoryState::Archived, MemoryState::Decayed] {
            let e = db.set_state("md/L4/ghost", st).unwrap_err();
            assert!(matches!(e, SgdbError::Invalid(_)), "{st:?}: {e:?}");
        }
        assert!(db.validate().is_empty(), "nenhuma side-table órfã");
        // Active em chave fantasma: remove-only, sem erro nem side-table
        db.set_state("md/L4/ghost", MemoryState::Active).unwrap();
        assert!(db.validate().is_empty());
        // supersede com old fantasma → Err (mesma proteção)
        assert!(db.supersede("md/L4/ghost-old", "md/L4/ghost-new").is_err());
        assert!(db.validate().is_empty());
    }

    #[test]
    fn resolve_known_key_finds_layer_for_raw_key() {
        // AUDIT (v1.1 P1): chave crua "h/imp" resolvia para `md/h/imp`
        // (inexistente) em vez de `md/L4/h/imp` — meta/set_importance/
        // reinforce falhavam silenciosamente com a chave certa e forma
        // errada. `resolve_known_key` faz o fallback determinístico por
        // prioridade de camada (L4 semântica primeiro).
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("h/imp", "hostilidade importante", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        // chave crua resolve para o doc L4 (não para `md/h/imp` fantasma)
        let m = db.meta("h/imp").unwrap().expect("meta via chave crua");
        assert_eq!(m.importance, 1.0);
        db.set_importance("h/imp", 0.3).unwrap();
        assert_eq!(db.meta("h/imp").unwrap().unwrap().importance, 0.3);
        db.set_confidence("h/imp", 0.7).unwrap();
        assert_eq!(db.meta("h/imp").unwrap().unwrap().confidence, 0.7);
        db.reinforce("h/imp", 0.2).unwrap();
        assert!((db.meta("h/imp").unwrap().unwrap().importance - 0.5).abs() < 1e-6);
        db.forget("h/imp").unwrap();
        assert_eq!(
            db.get_state("h/imp").unwrap(),
            MemoryState::Archived,
            "forget via chave crua arquiva o doc L4"
        );
        // chave canônica explícita continua funcionando (paridade)
        assert_eq!(db.meta("md/L4/h/imp").unwrap().unwrap().importance, 0.5);
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

    // ── Phase 3: identidade POR VERSÃO + DAG causal (v0.7) ────────────────

    #[test]
    fn version_identity_and_lineage() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("k", "v1", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        let slot = db.memory_id("md/L4/k").unwrap().unwrap();
        let v1 = db.version_of("md/L4/k").unwrap().unwrap();
        assert_eq!(v1, slot, "1ª versão do slot == slot");
        // overwrite → versão NOVA, slot estável, parent = versão anterior
        db.remember_semantic("k", "v2", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        let v2 = db.version_of("md/L4/k").unwrap().unwrap();
        assert_ne!(v2, v1, "overwrite deve criar versão nova");
        assert_eq!(db.memory_id("md/L4/k").unwrap().unwrap(), slot);
        let m = db.meta("md/L4/k").unwrap().unwrap();
        assert_eq!(m.parent_ids, vec![v1.clone()]);
        // lineage: [v2 (k), v1 (k)]
        let lin = db.lineage("md/L4/k").unwrap();
        assert_eq!(lin.len(), 2);
        assert_eq!(lin[0].version_id, v2);
        assert_eq!(lin[1].version_id, v1);
        assert_eq!(lin[1].storage_key, "md/L4/k");
        // chave sem doc → lineage vazia
        assert!(db.lineage("md/L4/nao-existe").unwrap().is_empty());
    }

    #[test]
    fn lineage_across_supersede_keys() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("k1", "velho", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        db.remember_semantic("k2", "novo", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        let v_k1 = db.version_of("md/L4/k1").unwrap().unwrap();
        db.supersede("md/L4/k1", "md/L4/k2").unwrap();
        let v_k2 = db.version_of("md/L4/k2").unwrap().unwrap();
        // lineage cruza CHAVES via o índice reverso sys/version/
        let lin = db.lineage("md/L4/k2").unwrap();
        assert_eq!(lin.len(), 2);
        assert_eq!(lin[0].version_id, v_k2);
        assert_eq!(lin[0].storage_key, "md/L4/k2");
        assert_eq!(lin[1].version_id, v_k1);
        assert_eq!(lin[1].storage_key, "md/L4/k1");
        // histórico preservado (invalidar-não-deletar)
        assert_eq!(db.get_state("md/L4/k1").unwrap(), MemoryState::Superseded);
        assert!(db.get(MemoryLayer::L4Semantic, "k1").unwrap().is_some());
    }

    #[test]
    fn version_id_travels_with_replication() {
        let mut a = Sgdb::open_with_node_id(1, InMemory::new()).unwrap();
        let mut b = Sgdb::open_with_node_id(2, InMemory::new()).unwrap();
        a.remember_semantic("k", "doc", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        let v = a.version_of("md/L4/k").unwrap().unwrap();
        let rec = a.export_record("md/L4/k").unwrap().unwrap();
        b.import_record(rec).unwrap();
        // a VERSÃO do criador viaja (identidade por versão)
        assert_eq!(b.version_of("md/L4/k").unwrap().unwrap(), v);
        // overwrite LOCAL no receptor → versão NOVA com linhagem preservada
        b.remember_semantic("k", "edit local", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        let v2 = b.version_of("md/L4/k").unwrap().unwrap();
        assert_ne!(v2, v);
        assert!(b.meta("md/L4/k").unwrap().unwrap().parent_ids.contains(&v));
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn version_identity_persists_across_reopen() {
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sgdb_version.db");
        let _ = std::fs::remove_file(&path);
        let (slot, v2);
        {
            let mut db = Sgdb::open(crate::storage::FileStorage::open(&path).unwrap()).unwrap();
            db.remember_semantic("k", "v1", &[1.0, -1.0, 1.0, -1.0])
                .unwrap();
            slot = db.memory_id("md/L4/k").unwrap().unwrap();
            db.remember_semantic("k", "v2", &[1.0, -1.0, 1.0, -1.0])
                .unwrap();
            v2 = db.version_of("md/L4/k").unwrap().unwrap();
            db.checkpoint().unwrap();
        }
        {
            let mut db = Sgdb::open(crate::storage::FileStorage::open(&path).unwrap()).unwrap();
            assert_eq!(db.memory_id("md/L4/k").unwrap().unwrap(), slot);
            assert_eq!(db.version_of("md/L4/k").unwrap().unwrap(), v2);
            // índice reverso reconstruído no rebuild → lineage pós-reopen
            assert_eq!(db.lineage("md/L4/k").unwrap().len(), 2);
        }
        let _ = std::fs::remove_file(&path);
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

    // ── L6 relations (v0.8) ───────────────────────────────────────────────

    #[test]
    fn relations_associate_and_query_all_directions() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let a = "md/L4/fire";
        let b = "md/L4/smoke";
        let c = "md/L4/heat";
        db.associate(a, RelationKind::Causes, b).unwrap();
        db.associate(a, RelationKind::Supports, c).unwrap();
        db.associate(b, RelationKind::Contradicts, c).unwrap();
        // queries direcionais (saídas)
        assert_eq!(db.causes(a), vec![b.to_string()]);
        assert_eq!(db.supports(a), vec![c.to_string()]);
        assert_eq!(db.contradicts(b), vec![c.to_string()]);
        // related_to: ambos os sentidos, todos os kinds, determinístico
        let rels = db.related_to(a);
        assert_eq!(
            rels,
            vec![
                (RelationKind::Causes, b.to_string()),
                (RelationKind::Supports, c.to_string()),
            ]
        );
        let rels_b = db.related_to(b);
        // aresta a--causes-->b vista de b devolve o OUTRO lado (a);
        // b--contradicts-->c devolve c — determinístico por (kind, alvo)
        assert_eq!(
            rels_b,
            vec![
                (RelationKind::Causes, a.to_string()),
                (RelationKind::Contradicts, c.to_string()),
            ]
        );
        // idempotente
        db.associate(a, RelationKind::Causes, b).unwrap();
        assert_eq!(db.causes(a), vec![b.to_string()]);
        // chave com '#' é rejeitada (separador reservado)
        assert!(db.associate("md/L4/x#y", RelationKind::Causes, b).is_err());
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn relations_persist_across_reopen_and_delete_cleans_topology() {
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("rel.db");
        let _ = std::fs::remove_file(&path);
        let (a, b) = ("md/L4/fire", "md/L4/smoke");
        {
            let mut db = Sgdb::open(crate::storage::FileStorage::open(&path).unwrap()).unwrap();
            db.associate(a, RelationKind::Causes, b).unwrap();
            db.associate(a, RelationKind::Supports, b).unwrap();
        }
        // reopen: índice reconstruído do storage (fonte da verdade)
        {
            let mut db = Sgdb::open(crate::storage::FileStorage::open(&path).unwrap()).unwrap();
            assert_eq!(db.causes(a), vec![b.to_string()]);
            assert_eq!(db.supports(a), vec![b.to_string()]);
            assert_eq!(db.related_to(a).len(), 2);
            // delete de uma memória limpa a topologia envolvendo ela
            db.delete(b).unwrap();
            assert!(db.related_to(b).is_empty());
            assert!(db.causes(a).is_empty(), "b morto não é mais alvo");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn relations_do_not_require_docs_and_support_derived_from() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        // a relação é afirmada pela camada superior; o doc não precisa existir
        db.associate("md/L4/sem", RelationKind::DerivedFrom, "md/L3/ep1").unwrap();
        db.associate("md/L4/sem", RelationKind::DerivedFrom, "md/L3/ep2").unwrap();
        assert_eq!(db.derived_from("md/L4/sem").len(), 2);
        assert_eq!(db.related_to("md/L3/ep1").len(), 1);
    }

    #[test]
    fn associate_checked_rejects_ghost_keys_no_orphan_relation() {
        // AUDIT (v1.1 P3): associate_checked valida existência dos DOIS lados
        // — chave fantasma → Err, nenhuma `sys/rel/` órfã. O associate cru
        // continua sem validar (design preservado: relations_do_not_require_docs).
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("a", "lado a", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        // a existe, b fantasma → Err
        let e = db
            .associate_checked("md/L4/a", RelationKind::RelatedTo, "md/L4/ghost")
            .unwrap_err();
        assert!(matches!(e, SgdbError::Invalid(_)), "{e:?}");
        // a fantasma, b existe → Err
        assert!(db
            .associate_checked("md/L4/ghost", RelationKind::RelatedTo, "md/L4/a")
            .is_err());
        // nenhuma relação órfã gravada
        assert!(db.validate().is_empty(), "nenhuma side-table órfã");
        assert_eq!(db.related_to("md/L4/a").len(), 0);
        // chave crua resolve antes de validar (fallback determinístico P1)
        db.remember_semantic("b", "lado b", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        db.associate_checked("a", RelationKind::RelatedTo, "b").unwrap();
        assert_eq!(db.related_to("md/L4/b").len(), 1);
        assert_eq!(db.related_to("md/L4/a").len(), 1);
    }

    // ── recall active vs historical (v0.8) ────────────────────────────────

    #[test]
    fn recall_defaults_to_active_historical_opts_in() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("k1", "memoria viva", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        db.remember_semantic("k2", "memoria velha", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        // k2 envelhece: superseded → não deve aparecer no recall default
        // (estado é POR DOC: o companion L2 também é marcado, senão o texto
        // continua ativo no recall lexical)
        db.set_state("md/L4/k2", MemoryState::Superseded).unwrap();
        db.set_state("md/L2/k2", MemoryState::Superseded).unwrap();
        db.set_state("md/L4/k1", MemoryState::Decayed).unwrap();
        db.set_state("md/L2/k1", MemoryState::Decayed).unwrap();
        let active = db.recall(&[1.0, -1.0, 1.0, -1.0], 10).unwrap();
        assert!(
            active.iter().all(|h| !h.key.ends_with("/k2") && !h.key.ends_with("/k1")),
            "recall default não deve conter memórias inativas: {:?}",
            active.iter().map(|h| h.key.clone()).collect::<Vec<_>>()
        );
        assert!(active.is_empty(), "só havia memórias inativas");
        // histórico: ambas aparecem COM estado exposto
        let hist = db.recall_historical(&[1.0, -1.0, 1.0, -1.0], 10).unwrap();
        let st: Vec<(String, MemoryState)> = hist
            .iter()
            .map(|h| {
                (
                    h.key.clone(),
                    h.provenance.as_ref().map(|p| p.state).unwrap_or(MemoryState::Active),
                )
            })
            .collect();
        assert!(st.iter().any(|(k, s)| k.ends_with("/k1") && *s == MemoryState::Decayed));
        assert!(st.iter().any(|(k, s)| k.ends_with("/k2") && *s == MemoryState::Superseded));
        // lexical: mesmo contrato
        let lx = db.recall_lexical("memoria velha", 10).unwrap();
        assert!(
            lx.iter().all(|h| !h.key.ends_with("/k2")),
            "lexical default filtra inativas"
        );
        let lxh = db.recall_lexical_historical("memoria velha", 10).unwrap();
        assert!(lxh.iter().any(|h| h.key.ends_with("/k2")));
    }

    // ── v0.9: reforço, conflito de 1ª classe, resolução, API cognitiva ────

    #[test]
    fn reinforce_updates_importance_and_last_reinforced() {
        let mut db = Sgdb::open_with_node_id(1, InMemory::new()).unwrap();
        let doc = MemoryDoc::new(MemoryLayer::L4Semantic, "pref", b"dark theme".to_vec());
        db.put(doc).unwrap();
        let before = db.meta("md/L4/pref").unwrap().unwrap();
        assert_eq!(before.importance, 1.0);
        assert_eq!(before.last_reinforced, 0, "nunca reforçada");
        db.reinforce("md/L4/pref", 0.1).unwrap();
        let after = db.meta("md/L4/pref").unwrap().unwrap();
        assert!((after.importance - 1.0).abs() < 1e-6, "1.0 já é o teto");
        assert_eq!(after.last_reinforced, 1, "contador próprio no reforço");
        // delta não-finita é rejeitada
        assert!(db.reinforce("md/L4/pref", f32::NAN).is_err());
    }

    #[test]
    fn reinforce_importance_is_clamped() {
        let mut db = Sgdb::open_with_node_id(1, InMemory::new()).unwrap();
        let doc = MemoryDoc::new(MemoryLayer::L4Semantic, "imp", b"x".to_vec());
        db.put(doc).unwrap();
        db.set_importance("md/L4/imp", 0.9).unwrap();
        db.reinforce("md/L4/imp", 0.5).unwrap(); // 1.4 → clamp 1.0
        assert_eq!(db.meta("md/L4/imp").unwrap().unwrap().importance, 1.0);
        db.reinforce("md/L4/imp", -1.0).unwrap(); // 0.0 (clamp)
        assert_eq!(db.meta("md/L4/imp").unwrap().unwrap().importance, 0.0);
    }

    #[cfg(feature = "file-storage")]
    #[test]
    fn reinforce_persists_across_reopen() {
        let dir = std::env::temp_dir().join("neural_sgdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sgdb_reinforce.db");
        let _ = std::fs::remove_file(&path);
        {
            let mut db = Sgdb::open(crate::storage::FileStorage::open(&path).unwrap()).unwrap();
            db.put(MemoryDoc::new(MemoryLayer::L4Semantic, "r", b"x".to_vec()))
                .unwrap();
            db.reinforce("md/L4/r", 0.3).unwrap();
            db.checkpoint().unwrap();
        }
        {
            let mut db = Sgdb::open(crate::storage::FileStorage::open(&path).unwrap()).unwrap();
            let m = db.meta("md/L4/r").unwrap().unwrap();
            assert!((m.importance - 1.0).abs() < 1e-6); // 0.7+0.3 → 1.0
            assert!(m.last_reinforced > 0, "last_reinforced sobrevive ao reopen");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(feature = "p2p")]
    #[test]
    fn concurrent_merge_creates_persisted_conflict() {
        let mut a = Sgdb::open_with_node_id(1, InMemory::new()).unwrap();
        let mut b = Sgdb::open_with_node_id(2, InMemory::new()).unwrap();
        a.remember_semantic("theme", "dark", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        b.remember_semantic("theme", "light", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        let a_vid = a.version_of("md/L4/theme").unwrap().unwrap();
        let b_vid = b.version_of("md/L4/theme").unwrap().unwrap();
        let rec_b = b.export_record("md/L4/theme").unwrap().unwrap();
        let rec_a = a.export_record("md/L4/theme").unwrap().unwrap();
        // A recebe B e B recebe A → AMBOS veem Conflict
        let v_a = a.merge_remote(rec_b).unwrap();
        let v_b = b.merge_remote(rec_a).unwrap();
        use crate::crdt::MergeVerdict;
        assert_eq!(v_a, MergeVerdict::Conflict);
        assert_eq!(v_b, MergeVerdict::Conflict);
        // conflito persistido com evidência dos DOIS lados
        let cs = a.conflicts();
        assert_eq!(cs.len(), 1, "um conflito, id determinístico (upsert)");
        let c = &cs[0];
        assert_eq!(c.subject, "md/L4/theme");
        assert_eq!(c.candidates.len(), 2);
        assert_eq!(c.records.len(), 2, "evidência MDR1 dos dois candidatos");
        assert_eq!(c.status, crate::conflict::ConflictStatus::Open);
        assert_eq!(b.conflicts().len(), 1, "B também preserva (id igual)");
        // re-entrega (duplicata causal) NÃO duplica o conflito
        let rec_b2 = b.export_record("md/L4/theme").unwrap().unwrap();
        let _ = a.merge_remote(rec_b2).unwrap();
        assert_eq!(a.conflicts().len(), 1);
        // nem o local nem o remoto foram sobrescritos silenciosamente
        assert_eq!(a.version_of("md/L4/theme").unwrap().unwrap(), a_vid);
        assert_eq!(b.version_of("md/L4/theme").unwrap().unwrap(), b_vid);
    }

    #[cfg(feature = "p2p")]
    #[test]
    fn resolve_conflict_imports_winner_and_preserves_loser() {
        let mut a = Sgdb::open_with_node_id(1, InMemory::new()).unwrap();
        let mut b = Sgdb::open_with_node_id(2, InMemory::new()).unwrap();
        a.remember_semantic("theme", "dark", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        b.remember_semantic("theme", "light", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        let a_vid = a.version_of("md/L4/theme").unwrap().unwrap();
        let b_vid = b.version_of("md/L4/theme").unwrap().unwrap();
        let rec_b = b.export_record("md/L4/theme").unwrap().unwrap();
        assert_eq!(a.merge_remote(rec_b).unwrap(), crate::crdt::MergeVerdict::Conflict);
        let cid = a.conflicts()[0].conflict_id.clone();
        // vencedor inválido → Err, nada muda (conflito ainda Open)
        assert!(a.resolve_conflict(&cid, "vid-inexistente").is_err());
        // a camada superior decide: vence B (o remoto)
        a.resolve_conflict(&cid, &b_vid).unwrap();
        // slot agora tem a versão do vencedor, ativa
        assert_eq!(a.version_of("md/L4/theme").unwrap().unwrap(), b_vid);
        assert_eq!(a.get_state("md/L4/theme").unwrap(), MemoryState::Active);
        // perdedor preservado na linhagem do vencedor
        let m = a.meta("md/L4/theme").unwrap().unwrap();
        assert!(m.parent_ids.contains(&a_vid), "perdedor vira parent");
        // conflito marcado Resolved (idempotente: resolver de novo = Ok)
        let c = a.conflict(&cid).unwrap();
        assert_eq!(c.status, crate::conflict::ConflictStatus::Resolved);
        assert_eq!(c.resolved_winner.as_deref(), Some(b_vid.as_str()));
        a.resolve_conflict(&cid, &b_vid).unwrap();
        // evidência original continua íntegra
        assert_eq!(c.records.len(), 2);
    }

    #[cfg(feature = "p2p")]
    #[test]
    fn dismiss_conflict_removes_only_the_marker() {
        let mut a = Sgdb::open_with_node_id(1, InMemory::new()).unwrap();
        let mut b = Sgdb::open_with_node_id(2, InMemory::new()).unwrap();
        a.remember_semantic("k", "a", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        b.remember_semantic("k", "b", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        let _ = a.merge_remote(b.export_record("md/L4/k").unwrap().unwrap()).unwrap();
        let cid = a.conflicts()[0].conflict_id.clone();
        a.dismiss_conflict(&cid).unwrap();
        assert!(a.conflicts().is_empty());
        // memórias continuam íntegras (só o marcador sumiu)
        assert!(a.get(MemoryLayer::L4Semantic, "k").unwrap().is_some());
    }

    #[test]
    fn merge_memories_creates_child_with_both_parents() {
        let mut db = Sgdb::open_with_node_id(1, InMemory::new()).unwrap();
        db.put(MemoryDoc::new(MemoryLayer::L4Semantic, "a", b"fato a".to_vec()))
            .unwrap();
        db.put(MemoryDoc::new(MemoryLayer::L3EpisodicLong, "b", b"fato b".to_vec()))
            .unwrap();
        db.set_importance("md/L4/a", 0.4).unwrap();
        db.set_importance("md/L3/b", 0.9).unwrap();
        let va = db.version_of("md/L4/a").unwrap().unwrap();
        let vb = db.version_of("md/L3/b").unwrap().unwrap();
        let new_sk = db.merge_memories("md/L4/a", "md/L3/b", "c").unwrap();
        assert_eq!(new_sk, "md/L4/c", "camada = max(a, b)");
        // C nasce com ambos os parents e payload fundido
        let m = db.meta(&new_sk).unwrap().unwrap();
        assert!(m.parent_ids.contains(&va));
        assert!(m.parent_ids.contains(&vb));
        assert!((m.importance - 0.9).abs() < 1e-6, "importância = max");
        let doc = db.get(MemoryLayer::L4Semantic, "c").unwrap().unwrap();
        let text = String::from_utf8_lossy(&doc.payload);
        assert!(text.contains("fato a") && text.contains("fato b"));
        // fontes intactas
        assert!(db.get(MemoryLayer::L4Semantic, "a").unwrap().is_some());
        assert!(db.get(MemoryLayer::L3EpisodicLong, "b").unwrap().is_some());
    }

    #[test]
    fn forget_archives_but_keeps_history() {
        let mut db = Sgdb::open_with_node_id(1, InMemory::new()).unwrap();
        let doc = MemoryDoc::new(MemoryLayer::L4Semantic, "t", b"texto".to_vec());
        db.put(doc).unwrap();
        db.set_importance("md/L4/t", 0.7).unwrap();
        db.forget("md/L4/t").unwrap();
        assert_eq!(db.get_state("md/L4/t").unwrap(), MemoryState::Archived);
        // história preservada: doc acessível e recall histórico o vê
        assert!(db.get(MemoryLayer::L4Semantic, "t").unwrap().is_some());
        let emb = vec![1.0, -1.0, 1.0, -1.0];
        assert!(db.recall(&emb, 5).unwrap().is_empty(), "default exclui Archived");
        assert!(
            db.recall_historical(&emb, 5).unwrap().len() == 1,
            "histórico mantém a memória"
        );
        // metadados intactos (importância não sumiu com o forget)
        assert!((db.meta("md/L4/t").unwrap().unwrap().importance - 0.7).abs() < 1e-6);
    }

    #[test]
    fn explain_reports_state_lineage_and_reinforcement() {
        let mut db = Sgdb::open_with_node_id(1, InMemory::new()).unwrap();
        db.put(MemoryDoc::new(MemoryLayer::L4Semantic, "e", b"x".to_vec()))
            .unwrap();
        db.set_importance("md/L4/e", 0.8).unwrap();
        db.set_confidence("md/L4/e", 0.9).unwrap();
        db.reinforce("md/L4/e", 0.1).unwrap();
        let ex = db.explain("md/L4/e").unwrap();
        assert_eq!(ex.key, "md/L4/e");
        assert_eq!(ex.layer, MemoryLayer::L4Semantic);
        assert_eq!(ex.state, MemoryState::Active);
        assert!((ex.importance - 0.9).abs() < 1e-6);
        assert!((ex.confidence - 0.9).abs() < 1e-6);
        assert_eq!(ex.last_reinforced, 1);
        assert_eq!(ex.validity, None);
        // supersede: estado muda e a versão nova aparece como child
        db.put(MemoryDoc::new(MemoryLayer::L4Semantic, "e", b"nova".to_vec()))
            .unwrap();
        let ex2 = db.explain("md/L4/e").unwrap();
        assert_eq!(ex2.state, MemoryState::Active);
        assert_eq!(ex2.parents.len(), 1, "overwrite vira parent");
        assert!(ex2.parents.contains(&ex.version_id));
    }

    #[test]
    fn transfer_to_moves_layer_with_lineage() {
        let mut db = Sgdb::open_with_node_id(1, InMemory::new()).unwrap();
        db.remember_exchange("oi", "tudo bem").unwrap(); // L1 last_user
        let src_vid = db.version_of("md/L1/last_user").unwrap().unwrap();
        let new_sk = db.transfer_to("md/L1/last_user", MemoryLayer::L4Semantic).unwrap();
        assert_eq!(new_sk, "md/L4/last_user");
        assert_eq!(db.get_state("md/L4/last_user").unwrap(), MemoryState::Active);
        assert_eq!(db.get_state("md/L1/last_user").unwrap(), MemoryState::Archived);
        let m = db.meta("md/L4/last_user").unwrap().unwrap();
        assert!(m.parent_ids.contains(&src_vid), "linhagem registrada");
        assert_eq!(db.derived_from("md/L4/last_user"), vec!["md/L1/last_user".to_string()]);
        // idempotente: transferir para a mesma camada = no-op
        assert_eq!(db.transfer_to("md/L4/last_user", MemoryLayer::L4Semantic).unwrap(), new_sk);
    }

    // ── v1.0 — observabilidade (Phase 32): contadores estruturados ────────

    #[test]
    fn metrics_count_writes_and_recalls() {
        let mut db = Sgdb::open(crate::storage::InMemory::new()).unwrap();
        // open incrementa storage_recoveries + index_rebuilds
        assert_eq!(db.metrics().storage_recoveries, 1);
        assert_eq!(db.metrics().index_rebuilds, 1);

        let mut doc = MemoryDoc::new(MemoryLayer::L4Semantic, "k1", vec![1, 2, 3]);
        doc.bitvec = Some(quantize_f32(&[0.1, 0.2]));
        db.put(doc).unwrap();
        db.remember_semantic("k2", "text", &[0.5, 0.5]).unwrap();
        assert_eq!(db.metrics().memory_writes, 2);
        assert_eq!(db.metrics().clock_changes, 2);

        db.recall(&[0.1, 0.2], 3).unwrap();
        assert_eq!(db.metrics().recalls, 1);

        db.reset_metrics();
        assert_eq!(db.metrics().memory_writes, 0);
    }

    #[test]
    fn metrics_snapshot_is_structured() {
        let db = Sgdb::open(crate::storage::InMemory::new()).unwrap();
        let snap = db.metrics().snapshot();
        assert!(snap.contains(&("storage_recoveries", 1)));
        assert!(snap.contains(&("memory_writes", 0)));
        assert!(snap.contains(&("replication_received", 0)));
    }

    #[test]
    fn metrics_count_lifecycle_transitions() {
        use crate::lifecycle::{LifecycleConfig, MemoryLifecycle};
        let mut db = Sgdb::open(crate::storage::InMemory::new()).unwrap();
        db.put(MemoryDoc::new(MemoryLayer::L1Working, "m", b"episode".to_vec()))
            .unwrap();
        db.reset_metrics();

        let mut lc = MemoryLifecycle::new(LifecycleConfig::default());
        let rep = lc.tick(&mut db, 10).unwrap();
        assert!(rep.transitions >= 1);
        assert_eq!(db.metrics().lifecycle_transitions, rep.transitions);
    }

    // ── P2-3: health() / validate() ────────────────────────────────────────

    #[test]
    fn health_reports_observable_state() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("d1", "clima ensolarado", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        db.remember_exchange("oi", "ola!").unwrap(); // L1 → RAM
        let h = db.health();
        assert!(h.storage_ok);
        assert_eq!(h.backend, "in-memory");
        assert!(h.doc_count >= 2); // L4 d1 + L2 companion
        assert_eq!(h.bq_len, 1);
        assert!(h.ram_len >= 1);
        assert_eq!(h.open_conflicts, 0);
    }

    #[test]
    fn validate_is_clean_on_healthy_db() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("d1", "clima ensolarado", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        db.remember_fact("fato", 100).unwrap();
        db.checkpoint().unwrap();
        assert!(db.validate().is_empty());
    }

    #[test]
    fn validate_accepts_l5_procedural_embedding() {
        // AUDIT (battery 3, 3.5): o BQ indexa L4 E L5 — um doc L5 legítimo
        // com embedding (bitvec ou payload f32) fazia o validate reportar
        // "BQ index count != L4 doc count" (falso positivo: contava só L4).
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        // L5 com bitvec explícito (procedural embedding)
        let mut l5 = MemoryDoc::new(MemoryLayer::L5Procedural, "proc/1", b"rotina de wake".to_vec());
        l5.bitvec = Some(crate::bq::quantize_f32(&[1.0, -1.0, 1.0, -1.0]));
        db.put(l5).unwrap();
        // L5 com payload f32 cru (sem bitvec) — mesma regra do put_inner
        let l5b = MemoryDoc::new(
            MemoryLayer::L5Procedural,
            "proc/2",
            [1.0f32, -1.0, 1.0, -1.0].iter().flat_map(|x| x.to_le_bytes()).collect(),
        );
        db.put(l5b).unwrap();
        // L4 com embedding (padrão)
        db.remember_semantic("sem/1", "conceito", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        assert_eq!(db.bq_len(), 3, "L4 + 2×L5 indexados");
        assert!(db.validate().is_empty(), "L5 com embedding NÃO é falso positivo");
    }

    #[test]
    fn validate_detects_missing_art_index_entry() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("d1", "clima ensolarado", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        // corrompe o índice derivado: remove a entrada ART (o storage fica
        // intacto) — validate deve acusar "doc missing from ART index"
        let sk = "md/L4/d1";
        let v = db.engine.storage_get(sk.as_bytes()).unwrap().unwrap();
        db.engine.art.delete(sk);
        db.engine.bq.clear();
        let issues = db.validate();
        assert!(
            issues
                .iter()
                .any(|i| i.key == sk && i.message == "doc missing from ART index"),
            "issues: {issues:?}"
        );
        let _ = v;
    }

    #[test]
    fn validate_detects_corrupt_doc_bytes() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("d1", "clima ensolarado", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        // grava bytes corrompidos por cima (simula bit rot no storage) —
        // corrompe o MAGIC (decode falha garantido; corromper o meio pode
        // cair no payload de texto livre e decodar igual)
        let sk = "md/L4/d1";
        let mut blob = db.engine.storage_get(sk.as_bytes()).unwrap().unwrap();
        blob[0] ^= 0xFF;
        db.engine.storage_put_raw(sk.as_bytes(), &blob).unwrap();
        let issues = db.validate();
        assert!(
            issues.iter().any(|i| i.key == sk && i.message == "NMD1 doc does not decode"),
            "issues: {issues:?}"
        );
    }

    #[test]
    fn validate_detects_orphan_side_table() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        db.remember_semantic("d1", "clima ensolarado", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        // side-table órfã: sys/state/ apontando para um doc que não existe
        db.engine
            .storage_put_raw(b"sys/state/md/L4/ghost", &[1])
            .unwrap();
        let issues = db.validate();
        assert!(
            issues
                .iter()
                .any(|i| i.key == "md/L4/ghost" && i.message == "side-table targets missing doc"),
            "issues: {issues:?}"
        );
    }

    #[test]
    #[cfg(feature = "p2p")]
    fn health_reports_open_conflicts() {
        let mut a = Sgdb::open_with_node_id(1, InMemory::new()).unwrap();
        let mut b = Sgdb::open_with_node_id(2, InMemory::new()).unwrap();
        a.remember_semantic("theme", "dark", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        b.remember_semantic("theme", "light", &[1.0, -1.0, 1.0, -1.0])
            .unwrap();
        let rec_b = b.export_record("md/L4/theme").unwrap().unwrap();
        let rec_a = a.export_record("md/L4/theme").unwrap().unwrap();
        let _ = a.merge_remote(rec_b).unwrap();
        let _ = b.merge_remote(rec_a).unwrap();
        assert_eq!(a.health().open_conflicts, 1);
    }
}
