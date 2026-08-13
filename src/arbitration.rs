//! Arbitração plugável para conflitos de memória (v1.0, roadmap Phase 21).
//!
//! O core NUNCA decide verdade semântica — detecta, preserva e executa
//! decisões da camada superior. Este módulo expõe um TRAIT de política
//! que a camada cognitiva/AI implementa; o default (`HeuristicArbitration`)
//! usa apenas sinais determinísticos (importância, confiança, temporalidade,
//! parentesco) — SEM chamada LLM dentro do core.
//!
//! ```text
//! Conflict
//!    ↓
//! collect candidates + provenance + related memories
//!    ↓
//! policy.evaluate(conflict, db) → ArbitrationDecision
//!    ↓
//! apply_decision(db, decision) → resolve_conflict / merge_memories / invalidate
//! ```

use alloc::string::String;
use alloc::vec::Vec;

use crate::conflict::ConflictRecord;
use crate::sgdb::Sgdb;
use crate::storage::SgdbError;

/// Veredicto de arbitração — como o conflito deve ser resolvido.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArbitrationAction {
    /// Um dos candidatos é preferido como vencedor (version_id).
    Prefer,
    /// O candidato indicado é invalidado (marcado `Invalidated`); o outro
    /// permanece ativo.
    Invalidate,
    /// Fusão: C = A ++ B, parents=[A, B]. A e B ficam supersedidos.
    Merge,
    /// Nenhuma decisão automática: escalar para humano/agent externo.
    Escalate,
}

/// Resultado estruturado de uma avaliação de conflito. A camada superior
/// pode inspecionar e auditar antes de aplicar.
#[derive(Clone, Debug)]
pub struct ArbitrationDecision {
    pub action: ArbitrationAction,
    /// Versão vencedora (quando `action = Prefer`).
    pub winner_version_id: Option<String>,
    /// Versão a invalidar (quando `action = Invalidate`).
    pub invalidated_version_id: Option<String>,
    /// Chave do novo doc fundido (quando `action = Merge`).
    pub merged_key: Option<String>,
    /// Evidência que sustenta a decisão (explícita, não redutível a string).
    pub evidence: Vec<Evidence>,
    /// Razão textual (opcional, para logging/humano).
    pub reason: String,
}

/// Sinal de evidência que sustenta uma decisão.
#[derive(Clone, Debug, PartialEq)]
pub struct Evidence {
    pub source: EvidenceSource,
    pub key: String,
    pub value: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EvidenceSource {
    Importance,
    Confidence,
    Recency,
    Age,
    ParentCount,
    ConflictCount,
    Layer,
}

/// Trait de política de arbitração. A camada cognitiva/AI implementa este
/// trait; o core não contém chamada a LLM ou modelo externo.
pub trait ArbitrationPolicy {
    /// Avalia um conflito aberto e produz um veredicto.
    fn evaluate(&self, conflict: &ConflictRecord, db: &mut Sgdb) -> Result<ArbitrationDecision, SgdbError>;
}

/// Política heurística determinística (default): escolhe o candidato com
/// maior `importance * confidence` e mais recente (created_tick maior).
/// `Escalate` quando: ambos importância > 0.8 e confiança > 0.8
/// (ambos são altamente credíveis — decisão requer julgamento humano).
/// `Merge` quando: ambos candidatos têm a mesma importância máxima (1.0).
pub struct HeuristicArbitration {
    /// Limiar de importância para forçar `Escalate` (default: 0.8).
    pub escalate_above_importance: f32,
    /// Limiar de confiança para forçar `Escalate` (default: 0.8).
    pub escalate_above_confidence: f32,
}

impl Default for HeuristicArbitration {
    fn default() -> Self {
        HeuristicArbitration {
            escalate_above_importance: 0.8,
            escalate_above_confidence: 0.8,
        }
    }
}

impl ArbitrationPolicy for HeuristicArbitration {
    fn evaluate(&self, conflict: &ConflictRecord, db: &mut Sgdb) -> Result<ArbitrationDecision, SgdbError> {
        if conflict.records.len() < 2 {
            return Ok(ArbitrationDecision {
                action: ArbitrationAction::Escalate,
                winner_version_id: None,
                invalidated_version_id: None,
                merged_key: None,
                evidence: Vec::new(),
                reason: String::from("insufficient candidates"),
            });
        }

        // Coleta sinais de cada candidato
        let mut scores: Vec<(String, f64, Vec<Evidence>)> = Vec::with_capacity(conflict.candidates.len());
        for (i, vid) in conflict.candidates.iter().enumerate() {
            // resolve version_id → storage_key via sys/version/, then read
            // the CURRENT meta from sys/meta/<sk> (importance/confidence may
            // have been updated after the version was persisted). If the
            // candidate was never imported (Conflict branch preserves both
            // sides without importing the loser), fall back to decoding the
            // MDR1 evidence stored in the conflict record.
            let meta = match db.engine_mut().version_record(vid) {
                Ok(Some((sk, _))) => {
                    // prefer current meta (may have been updated by
                    // set_importance/set_confidence after the put)
                    db.meta(&sk).ok().flatten()
                }
                _ => None,
            }
            .or_else(|| {
                // Fallback: decode the MDR1 evidence from conflict.records[i]
                conflict
                    .records
                    .get(i)
                    .and_then(|bytes| crate::memory_doc::MemoryRecord::decode(bytes).ok())
                    .and_then(|rec| rec.doc.meta.clone())
            });
            let meta = match meta {
                Some(m) => m,
                _ => {
                    scores.push((vid.clone(), 0.0, Vec::new()));
                    continue;
                }
            };
            let age = db.engine_mut().own_counter().saturating_sub(meta.created_tick);
            let parent_count = meta.parent_ids.len() as f64;

            let mut evidence = Vec::new();
            let mut score = 0.0f64;

            // Importance
            let imp = meta.importance as f64;
            evidence.push(Evidence { source: EvidenceSource::Importance, key: vid.clone(), value: imp });
            score += imp;

            // Confidence
            let conf = meta.confidence as f64;
            evidence.push(Evidence { source: EvidenceSource::Confidence, key: vid.clone(), value: conf });
            score += conf;

            // Recency (inverso da idade: mais recente → maior pontuação)
            let recency = 1.0 / (1.0 + age as f64);
            evidence.push(Evidence { source: EvidenceSource::Recency, key: vid.clone(), value: recency });
            score += recency * 0.1; // peso menor

            // Parent count (mais lineage → mais confiança)
            evidence.push(Evidence { source: EvidenceSource::ParentCount, key: vid.clone(), value: parent_count });
            score += parent_count * 0.05;

            scores.push((vid.clone(), score, evidence));
        }

        // Escala: se ambos são altamente credíveis
        let all_evidence: Vec<Evidence> = scores.iter().flat_map(|(_, _, e)| e.clone()).collect();
        let high_credibility = scores.iter().filter(|(_, s, _)| *s > 1.5).count() >= 2;

        if high_credibility {
            // Merge quando ambos máximos; Escalate caso contrário
            let both_max = scores.iter().all(|(_, _, evidence)| {
                let imp = evidence.iter()
                    .find(|e| e.source == EvidenceSource::Importance)
                    .map(|e| e.value)
                    .unwrap_or(0.0);
                let conf = evidence.iter()
                    .find(|e| e.source == EvidenceSource::Confidence)
                    .map(|e| e.value)
                    .unwrap_or(0.0);
                imp >= 1.0 && conf >= 1.0
            });

            if both_max {
                return Ok(ArbitrationDecision {
                    action: ArbitrationAction::Merge,
                    winner_version_id: None,
                    invalidated_version_id: None,
                    merged_key: Some(conflict.subject.clone()),
                    evidence: all_evidence,
                    reason: String::from("both candidates max importance+confidence → merge"),
                });
            } else {
                return Ok(ArbitrationDecision {
                    action: ArbitrationAction::Escalate,
                    winner_version_id: None,
                    invalidated_version_id: None,
                    merged_key: None,
                    evidence: all_evidence,
                    reason: String::from("multiple high-credibility candidates → escalate to human"),
                });
            }
        }

        // Default: Prefer o candidato com maior score
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
        let winner = &scores[0];

        Ok(ArbitrationDecision {
            action: ArbitrationAction::Prefer,
            winner_version_id: Some(winner.0.clone()),
            invalidated_version_id: scores.get(1).map(|(v, _, _)| v.clone()),
            merged_key: None,
            evidence: winner.2.clone(),
            reason: alloc::format!("heuristic: highest score ({:.3})", winner.1),
        })
    }
}

/// Aplica um veredicto de arbitração ao banco (conveniência).
pub fn apply_decision(
    db: &mut Sgdb,
    conflict_id: &str,
    decision: &ArbitrationDecision,
) -> Result<(), SgdbError> {
    match decision.action {
        ArbitrationAction::Prefer => {
            if let Some(ref vid) = decision.winner_version_id {
                db.resolve_conflict(conflict_id, vid)?;
            }
        }
        ArbitrationAction::Invalidate => {
            if let Some(ref vid) = decision.invalidated_version_id {
                // Encontrar qual candidato NÃO é o invalidado → preferir
                let c = db.conflict(conflict_id).ok_or(SgdbError::Invalid("conflict not found"))?;
                let winner = c.candidates.iter()
                    .find(|v| *v != vid)
                    .cloned()
                    .ok_or(SgdbError::Invalid("no valid alternative"))?;
                db.resolve_conflict(conflict_id, &winner)?;
            }
        }
        ArbitrationAction::Merge => {
            if let Some(ref target) = decision.merged_key {
                let c = db.conflict(conflict_id).ok_or(SgdbError::Invalid("conflict not found"))?;
                if c.candidates.len() >= 2 {
                    let a = c.candidates[0].clone();
                    let b = c.candidates[1].clone();
                    // resolve com o primeiro candidato e depois merge
                    db.resolve_conflict(conflict_id, &a)?;
                    db.merge_memories(&a, &b, target)?;
                }
            }
        }
        ArbitrationAction::Escalate => {
            // Nada a fazer — a camada superior decide externamente
        }
    }
    Ok(())
}

#[cfg(test)]
#[cfg(feature = "p2p")]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;
    use crate::conflict::{ConflictStatus, ConflictRecord, generate_conflict_id};
    use crate::memory_doc::{MemoryDoc, MemoryLayer};
    use crate::storage::InMemory;

    fn open_db_with_concurrent_writes() -> (Sgdb, Sgdb, ConflictRecord) {
        let mut a = Sgdb::open_with_node_id(1, crate::storage::InMemory::new()).unwrap();
        let mut b = Sgdb::open_with_node_id(2, crate::storage::InMemory::new()).unwrap();
        a.remember_semantic("k", "A", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        b.remember_semantic("k", "B", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        let vid_a = a.version_of("md/L4/k").unwrap().unwrap();
        let vid_b = b.version_of("md/L4/k").unwrap().unwrap();
        let rec_a = a.export_record("md/L4/k").unwrap().unwrap();
        let rec_b = b.export_record("md/L4/k").unwrap().unwrap();
        let _ = a.merge_remote(rec_b).unwrap();
        let _ = b.merge_remote(rec_a).unwrap();
        let conflict = a.conflicts().remove(0);
        (a, b, conflict)
    }

    #[test]
    fn heuristic_prefers_higher_score() {
        let (mut a, _b, conflict) = open_db_with_concurrent_writes();
        let policy = HeuristicArbitration::default();
        let decision = policy.evaluate(&conflict, &mut a).unwrap();
        // Com importance=1.0, conf=1.0 para ambos, ambos são high-credibility
        // e ambos têm importance=1.0, confidence=1.0 → Merge (ambos máximos)
        assert!(decision.action == ArbitrationAction::Merge
            || decision.action == ArbitrationAction::Prefer);
    }

    #[test]
    fn escalate_when_both_highly_credible_and_not_max() {
        let mut a = Sgdb::open_with_node_id(1, crate::storage::InMemory::new()).unwrap();
        let mut b = Sgdb::open_with_node_id(2, crate::storage::InMemory::new()).unwrap();
        a.remember_semantic("k", "A", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        b.remember_semantic("k", "B", &[1.0, -1.0, 1.0, -1.0]).unwrap();
        // ambos importance 0.9 (não é 1.0), confidence 0.9
        a.set_importance("md/L4/k", 0.9).unwrap();
        a.set_confidence("md/L4/k", 0.9).unwrap();
        b.set_importance("md/L4/k", 0.9).unwrap();
        b.set_confidence("md/L4/k", 0.9).unwrap();
        let rec_a = a.export_record("md/L4/k").unwrap().unwrap();
        let rec_b = b.export_record("md/L4/k").unwrap().unwrap();
        let _ = a.merge_remote(rec_b).unwrap();
        let _ = b.merge_remote(rec_a).unwrap();
        let conflict = a.conflicts().remove(0);
        let policy = HeuristicArbitration::default();
        let decision = policy.evaluate(&conflict, &mut a).unwrap();
        assert_eq!(decision.action, ArbitrationAction::Escalate,
            "both highly credible but not max → escalate");
    }

    #[test]
    fn apply_prefer_resolves_conflict() {
        let (mut a, _b, conflict) = open_db_with_concurrent_writes();
        let cid = conflict.conflict_id.clone();
        let vid = conflict.candidates[0].clone();
        let decision = ArbitrationDecision {
            action: ArbitrationAction::Prefer,
            winner_version_id: Some(vid.clone()),
            invalidated_version_id: None,
            merged_key: None,
            evidence: Vec::new(),
            reason: "test".into(),
        };
        apply_decision(&mut a, &cid, &decision).unwrap();
        assert_eq!(a.conflict(&cid).unwrap().status, ConflictStatus::Resolved);
        assert_eq!(a.version_of("md/L4/k").unwrap().unwrap(), vid);
    }

    #[test]
    fn insufficient_candidates_escalates() {
        let mut db = Sgdb::open(crate::storage::InMemory::new()).unwrap();
        let conflict = ConflictRecord {
            conflict_id: "single".into(),
            subject: "md/L4/k".into(),
            candidates: vec!["v1".into()],
            nodes: vec![1],
            created_tick: 1,
            status: ConflictStatus::Open,
            resolved_winner: None,
            records: vec![vec![0]],
        };
        let policy = HeuristicArbitration::default();
        let decision = policy.evaluate(&conflict, &mut db).unwrap();
        assert_eq!(decision.action, ArbitrationAction::Escalate);
    }
}
