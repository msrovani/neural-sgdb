//! Engine de lifecycle determinístico (v0.8, roadmap Phase 15–16).
//!
//! Nenhum relógio de parede oculto, nenhuma thread em background: o caller
//! controla o agendamento chamando [`MemoryLifecycle::tick`] com `now`
//! explícito. Dado o mesmo db + a mesma sequência de `now`, o resultado é
//! idêntico (reprodutível em testes e em replay).
//!
//! Transições por tick (todas configuráveis, todas idempotentes — a fonte
//! só é promovida se ainda estiver `Active`):
//!
//! - **L1 → L2** (commit): memória de trabalho vira episódio curto; o L1 é
//!   `Archived` (working memory limpa, nunca deletada).
//! - **L2 → L3** (promoção): episódio curto → longo, por importância + idade.
//! - **L3 → L4** (semanticização heurística): importância + idade; o L4
//!   nasce SEM bitvec — embeddings são da camada superior (o core nunca
//!   gera representação semântica). O L3 vira `Superseded`.
//! - **L4 → L5** NUNCA é automático: procedimento exige decisão explícita da
//!   camada superior (trust/HITL — roadmap §16).
//! - **Decay**: `importance` cai por tick (desligado por padrão); abaixo do
//!   limiar, `Decayed` — NUNCA delete.
//! - **Archive**: `Superseded` mais velho que o limiar vira `Archived`.
//!
//! Toda promoção registra a linhagem: `parent_ids += [version_id da fonte]`
//! e relação L6 `new --derived_from--> old` (DAG causal + topologia).

use alloc::string::String;
use alloc::vec::Vec;

use crate::memory_doc::{MemoryLayer, MemoryState, RelationKind};
use crate::sgdb::Sgdb;
use crate::storage::SgdbError;

/// Política do lifecycle. Todos os campos têm default sensato; decay e
/// archive ficam DESLIGADOS por padrão (0.0 / None) — ative explicitamente.
#[derive(Clone, Debug)]
pub struct LifecycleConfig {
    /// L1 → L2: idade mínima (now − created_tick) para commitar.
    pub l1_commit_after_ticks: u64,
    /// L2 → L3: importance mínima para promoção.
    pub l2_to_l3_importance: f32,
    /// L2 → L3: idade mínima.
    pub l2_to_l3_min_age_ticks: u64,
    /// L3 → L4: importance mínima (semanticização heurística).
    pub l3_to_l4_importance: f32,
    /// L3 → L4: idade mínima.
    pub l3_to_l4_min_age_ticks: u64,
    /// Decay: importância multiplicada por (1 − decay_per_tick) por tick.
    /// `0.0` = desligado.
    pub decay_per_tick: f32,
    /// Abaixo desta importância, a memória vira `Decayed` (nunca deletada).
    pub decayed_below: f32,
    /// `Superseded` mais velho que isto vira `Archived` (None = desligado).
    pub archive_superseded_after_ticks: Option<u64>,
}

impl Default for LifecycleConfig {
    fn default() -> Self {
        LifecycleConfig {
            l1_commit_after_ticks: 1,
            l2_to_l3_importance: 0.5,
            l2_to_l3_min_age_ticks: 2,
            l3_to_l4_importance: 0.8,
            l3_to_l4_min_age_ticks: 3,
            decay_per_tick: 0.0,
            decayed_below: 0.1,
            archive_superseded_after_ticks: None,
        }
    }
}

/// Resultado estruturado de um tick (observabilidade — roadmap §32): quem
/// transitou e para onde. Nada aqui é human-readable-only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LifecycleReport {
    pub tick: u64,
    /// storage keys L1 → L2 (commit)
    pub committed: Vec<String>,
    /// storage keys L2 → L3 (promoção)
    pub promoted: Vec<String>,
    /// storage keys L3 → L4 (semanticização heurística)
    pub semanticized: Vec<String>,
    /// storage keys que viraram `Archived`
    pub archived: Vec<String>,
    /// storage keys que viraram `Decayed`
    pub decayed: Vec<String>,
}

/// Determinístico: o estado interno é só o contador de ticks + a config.
#[derive(Clone, Debug)]
pub struct MemoryLifecycle {
    config: LifecycleConfig,
    tick: u64,
}

impl MemoryLifecycle {
    pub fn new(config: LifecycleConfig) -> Self {
        MemoryLifecycle { config, tick: 0 }
    }

    pub fn tick(&mut self, db: &mut Sgdb, now: u64) -> Result<LifecycleReport, SgdbError> {
        self.tick = self.tick.saturating_add(1);
        let mut report = LifecycleReport {
            tick: self.tick,
            ..LifecycleReport::default()
        };

        // 1) L1 → L2 (commit episódico)
        let l1: Vec<String> = db
            .scan_prefix("md/L1/")?
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        for sk in l1 {
            if db.get_state(&sk)? != MemoryState::Active {
                continue; // idempotência: só promove fonte ativa
            }
            let Some(rec) = db.export_record(&sk)? else {
                continue;
            };
            let age = now.saturating_sub(rec.doc.meta.as_ref().map(|m| m.created_tick).unwrap_or(0));
            if age < self.config.l1_commit_after_ticks {
                continue;
            }
            let new_sk = promote(
                db,
                &rec,
                &sk,
                MemoryLayer::L2EpisodicShort,
                MemoryState::Archived,
            )?;
            report.committed.push(new_sk);
        }

        // 2) L2 → L3 (promoção por importância + idade)
        let l2: Vec<String> = db
            .scan_prefix("md/L2/")?
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        for sk in l2 {
            if db.get_state(&sk)? != MemoryState::Active {
                continue;
            }
            let Some(rec) = db.export_record(&sk)? else {
                continue;
            };
            let m = rec.doc.meta.as_ref();
            let importance = m.map(|m| m.importance).unwrap_or(0.0);
            let age = now.saturating_sub(m.map(|m| m.created_tick).unwrap_or(0));
            if importance < self.config.l2_to_l3_importance
                || age < self.config.l2_to_l3_min_age_ticks
            {
                continue;
            }
            let new_sk =
                promote(db, &rec, &sk, MemoryLayer::L3EpisodicLong, MemoryState::Archived)?;
            report.promoted.push(new_sk);
        }

        // 3) L3 → L4 (semanticização heurística — SEM LLM e SEM embedding
        //    no core: o L4 nasce com payload de texto e bitvec None; a
        //    camada superior anexa embeddings depois)
        let l3: Vec<String> = db
            .scan_prefix("md/L3/")?
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        for sk in l3 {
            if db.get_state(&sk)? != MemoryState::Active {
                continue;
            }
            let Some(rec) = db.export_record(&sk)? else {
                continue;
            };
            let m = rec.doc.meta.as_ref();
            let importance = m.map(|m| m.importance).unwrap_or(0.0);
            let age = now.saturating_sub(m.map(|m| m.created_tick).unwrap_or(0));
            if importance < self.config.l3_to_l4_importance
                || age < self.config.l3_to_l4_min_age_ticks
            {
                continue;
            }
            let new_sk =
                promote(db, &rec, &sk, MemoryLayer::L4Semantic, MemoryState::Superseded)?;
            report.semanticized.push(new_sk);
        }

        // 4) Decay (desligado por padrão) — importância cai, história fica
        if self.config.decay_per_tick > 0.0 {
            for layer in [
                MemoryLayer::L2EpisodicShort,
                MemoryLayer::L3EpisodicLong,
                MemoryLayer::L4Semantic,
                MemoryLayer::L5Procedural,
            ] {
                let prefix = alloc::format!("md/{}/", layer.as_str());
                let keys: Vec<String> = db
                    .scan_prefix(&prefix)?
                    .into_iter()
                    .map(|(k, _)| k)
                    .collect();
                for sk in keys {
                    if db.get_state(&sk)? != MemoryState::Active {
                        continue;
                    }
                    let Some(m) = db.meta(&sk)? else {
                        continue;
                    };
                    let imp = (m.importance * (1.0 - self.config.decay_per_tick)).max(0.0);
                    db.set_importance(&sk, imp)?;
                    if imp < self.config.decayed_below {
                        db.set_state(&sk, MemoryState::Decayed)?;
                        report.decayed.push(sk);
                    }
                }
            }
        }

        // 5) Archive: Superseded envelhecido → Archived (nunca delete)
        if let Some(after) = self.config.archive_superseded_after_ticks {
            let all: Vec<String> = db
                .scan_prefix("md/")?
                .into_iter()
                .map(|(k, _)| k)
                .collect();
            for sk in all {
                if db.get_state(&sk)? != MemoryState::Superseded {
                    continue;
                }
                let age = db
                    .meta(&sk)?
                    .map(|m| now.saturating_sub(m.created_tick))
                    .unwrap_or(0);
                if age >= after {
                    db.set_state(&sk, MemoryState::Archived)?;
                    report.archived.push(sk);
                }
            }
        }

        Ok(report)
    }

    /// Contador interno de ticks (diagnóstico / observabilidade).
    pub fn tick_count(&self) -> u64 {
        self.tick
    }
}

/// Promove uma memória: copia payload para a camada alvo, registra a
/// linhagem (parent_ids + relação L6 `derived_from`) e envelhece a fonte.
/// Retorna a storage key do novo doc.
fn promote(
    db: &mut Sgdb,
    rec: &crate::memory_doc::MemoryRecord,
    origin_sk: &str,
    to_layer: MemoryLayer,
    origin_new_state: MemoryState,
) -> Result<String, SgdbError> {
    let origin_vid = rec
        .doc
        .meta
        .as_ref()
        .map(|m| m.version_id.clone())
        .unwrap_or_else(|| String::from("pre-v0.6"));
    let mut doc = crate::memory_doc::MemoryDoc::new(to_layer, &rec.doc.key, rec.doc.payload.clone());
    // L4 semanticizado nasce sem bitvec (embedding é da camada superior)
    doc.bitvec = None;
    db.put(doc)?;
    let new_sk = alloc::format!("md/{}/{}", to_layer.as_str(), rec.doc.key);
    db.add_parents(&new_sk, &[origin_vid])?;
    db.associate(&new_sk, RelationKind::DerivedFrom, origin_sk)?;
    db.set_state(origin_sk, origin_new_state)?;
    Ok(new_sk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;
    use crate::memory_doc::{MemoryDoc, MemoryLayer, MemoryState};
    use crate::sgdb::Sgdb;
    use crate::storage::InMemory;

    fn open_db() -> Sgdb {
        Sgdb::open(InMemory::new()).unwrap()
    }

    #[test]
    fn l1_commits_to_l2_and_is_idempotent() {
        let mut db = open_db();
        db.remember_exchange("oi", "tudo bem")
            .unwrap(); // L1 "last_user" + L2 "last_asst"
        let mut lc = MemoryLifecycle::new(LifecycleConfig {
            l1_commit_after_ticks: 1,
            ..LifecycleConfig::default()
        });
        // antes do tick: L1 ativo, sem L3
        assert_eq!(db.get_state("md/L1/last_user").unwrap(), MemoryState::Active);
        assert!(db.scan_prefix("md/L3/").unwrap().is_empty());
        let r1 = lc.tick(&mut db, 10).unwrap();
        // L1 commitou → L2, origem Archived
        assert_eq!(r1.committed.len(), 1, "L1 last_user deveria commitar");
        assert_eq!(
            db.get_state("md/L1/last_user").unwrap(),
            MemoryState::Archived
        );
        assert_eq!(db.get_state("md/L2/last_user").unwrap(), MemoryState::Active);
        // linhagem: parent + relação derived_from
        let m = db.meta("md/L2/last_user").unwrap().unwrap();
        assert_eq!(m.parent_ids.len(), 1, "L2 deveria ter o parent L1");
        assert_eq!(db.derived_from("md/L2/last_user"), vec!["md/L1/last_user".to_string()]);
        // idempotência: L1 já Archived → segundo tick não re-commita
        let r2 = lc.tick(&mut db, 20).unwrap();
        assert!(r2.committed.is_empty());
        assert_eq!(db.get_state("md/L1/last_user").unwrap(), MemoryState::Archived);
    }

    #[test]
    fn l2_promotes_to_l3_by_importance_and_age() {
        let mut db = open_db();
        let doc = MemoryDoc::new(MemoryLayer::L2EpisodicShort, "ep", b"episodio importante".to_vec());
        db.put(doc).unwrap();
        db.set_importance("md/L2/ep", 0.9).unwrap(); // acima do limiar 0.5
        let mut lc = MemoryLifecycle::new(LifecycleConfig {
            l2_to_l3_importance: 0.5,
            l2_to_l3_min_age_ticks: 2,
            ..LifecycleConfig::default()
        });
        // jovem demais no primeiro tick
        let r1 = lc.tick(&mut db, 1).unwrap();
        assert!(r1.promoted.is_empty());
        // amadurece e promove
        let r2 = lc.tick(&mut db, 3).unwrap();
        assert_eq!(r2.promoted, vec!["md/L3/ep".to_string()]);
        assert_eq!(db.get_state("md/L2/ep").unwrap(), MemoryState::Archived);
        assert_eq!(db.get_state("md/L3/ep").unwrap(), MemoryState::Active);
        // fonte Archived → nunca re-promove
        let r3 = lc.tick(&mut db, 5).unwrap();
        assert!(r3.promoted.is_empty());
        assert_eq!(db.scan_prefix("md/L3/").unwrap().len(), 1);
    }

    #[test]
    fn l3_semanticizes_to_l4_with_lineage() {
        let mut db = open_db();
        let doc = MemoryDoc::new(
            MemoryLayer::L3EpisodicLong,
            "know",
            b"conhecimento repetido".to_vec(),
        );
        db.put(doc).unwrap();
        db.set_importance("md/L3/know", 0.95).unwrap();
        let mut lc = MemoryLifecycle::new(LifecycleConfig {
            l3_to_l4_importance: 0.8,
            l3_to_l4_min_age_ticks: 1,
            ..LifecycleConfig::default()
        });
        let r = lc.tick(&mut db, 5).unwrap();
        assert_eq!(r.semanticized, vec!["md/L4/know".to_string()]);
        // L3 superseded (semanticizado), L4 ativo com linhagem e derived_from
        assert_eq!(db.get_state("md/L3/know").unwrap(), MemoryState::Superseded);
        assert_eq!(db.get_state("md/L4/know").unwrap(), MemoryState::Active);
        assert_eq!(db.derived_from("md/L4/know"), vec!["md/L3/know".to_string()]);
        assert!(!db.meta("md/L4/know").unwrap().unwrap().parent_ids.is_empty());
        // determinismo: db novo + mesmos inputs → mesmo resultado
        let mut db2 = open_db();
        db2.put(MemoryDoc::new(
            MemoryLayer::L3EpisodicLong,
            "know",
            b"conhecimento repetido".to_vec(),
        ))
        .unwrap();
        db2.set_importance("md/L3/know", 0.95).unwrap();
        let mut lc2 = MemoryLifecycle::new(LifecycleConfig {
            l3_to_l4_importance: 0.8,
            l3_to_l4_min_age_ticks: 1,
            ..LifecycleConfig::default()
        });
        let r2 = lc2.tick(&mut db2, 5).unwrap();
        assert_eq!(r.semanticized, r2.semanticized);
    }

    #[test]
    fn decay_marks_decayed_never_deletes() {
        let mut db = open_db();
        let doc = MemoryDoc::new(MemoryLayer::L4Semantic, "d", b"decaindo".to_vec());
        db.put(doc).unwrap();
        let mut lc = MemoryLifecycle::new(LifecycleConfig {
            decay_per_tick: 0.5, // cai pela metade por tick
            decayed_below: 0.3,
            ..LifecycleConfig::default()
        });
        // importance default L4 = 1.0 → 0.5 → 0.25 < 0.3 → Decayed no 2º tick
        let r1 = lc.tick(&mut db, 1).unwrap();
        assert!(r1.decayed.is_empty());
        assert_eq!(db.get_state("md/L4/d").unwrap(), MemoryState::Active);
        let r2 = lc.tick(&mut db, 2).unwrap();
        assert_eq!(r2.decayed, vec!["md/L4/d".to_string()]);
        assert_eq!(db.get_state("md/L4/d").unwrap(), MemoryState::Decayed);
        // NUNCA deleta: o doc continua no storage
        assert!(db.get(MemoryLayer::L4Semantic, "d").unwrap().is_some());
    }

    #[test]
    fn archive_superseded_after_ticks() {
        let mut d = open_db();
        let doc = MemoryDoc::new(MemoryLayer::L4Semantic, "old", b"antiga".to_vec());
        d.put(doc).unwrap();
        d.set_state("md/L4/old", MemoryState::Superseded).unwrap();
        let mut lc = MemoryLifecycle::new(LifecycleConfig {
            archive_superseded_after_ticks: Some(3),
            ..LifecycleConfig::default()
        });
        let r1 = lc.tick(&mut d, 1).unwrap();
        assert!(r1.archived.is_empty(), "jovem demais");
        let r2 = lc.tick(&mut d, 4).unwrap();
        assert_eq!(r2.archived, vec!["md/L4/old".to_string()]);
        assert_eq!(d.get_state("md/L4/old").unwrap(), MemoryState::Archived);
    }

    #[test]
    fn tick_count_is_explicit_no_hidden_clock() {
        let mut lc = MemoryLifecycle::new(LifecycleConfig::default());
        let mut db = open_db();
        assert_eq!(lc.tick_count(), 0);
        lc.tick(&mut db, 100).unwrap();
        lc.tick(&mut db, 200).unwrap();
        assert_eq!(lc.tick_count(), 2);
    }
}
