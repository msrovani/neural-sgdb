//! Observabilidade estruturada (v1.0, roadmap Phase 32) — contadores
//! nomeados para cada subsistema, SEM depender de logs human-readable.
//!
//! O `Sgdb` mantém um [`Metrics`] e o incrementa nos pontos de entrada
//! (writes, recalls, lifecycle, conflitos, replicação, recovery). O caller
//! lê um snapshot `(&str, u64)` — fácil de expor em MCP/HTTP/monitoramento.
//! `no_std`-safe (u64 puro, sem atomics — único dono `&mut self`).

use alloc::vec;
use alloc::vec::Vec;

/// Contadores de runtime do Sgdb.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metrics {
    // memória
    pub memory_writes: u64,
    pub recalls: u64,
    pub lifecycle_transitions: u64,
    // conflitos
    pub conflicts_detected: u64,
    pub conflicts_resolved: u64,
    // replicação (p2p)
    pub replication_sent: u64,
    pub replication_received: u64,
    pub replication_rejected: u64,
    pub replication_stale: u64,
    pub replication_duplicate: u64,
    // relógio / recovery
    pub clock_changes: u64,
    pub storage_recoveries: u64,
    pub index_rebuilds: u64,
    // custo de `open` (v1.1.21 — contrato de medição do ADR-0009 §4)
    /// Wall time do ULTIMO rebuild de índices, em ms. No `open` este é o custo
    /// de cold start do agente (o gargalo medido: ~94% do `open` é build de
    /// índice, não leitura). `0` sob `no_std`: não existe `Instant` no core do
    /// alvo bare-metal — a medição é explicitamente ausente, não falsa.
    pub open_rebuild_ms_last: u64,
    /// Pior caso observado na instância (dimensiona o orçamento de cold start).
    pub open_rebuild_ms_max: u64,
    /// Quantos `open` este PROCESSO já fez. Dor #2 do ADR-0009 ("muitos
    /// reopens"): `opens × open_rebuild_ms` é o custo de churn de sessão.
    /// Contador de processo, não durável — o histórico entre restarts é do
    /// HOST (`opens_per_hour` no ADR-0009 §4).
    pub opens: u64,
}

impl Metrics {
    /// Snapshot estruturado (nome, valor) — ordem estável para diffing.
    pub fn snapshot(&self) -> Vec<(&'static str, u64)> {
        vec![
            ("memory_writes", self.memory_writes),
            ("recalls", self.recalls),
            ("lifecycle_transitions", self.lifecycle_transitions),
            ("conflicts_detected", self.conflicts_detected),
            ("conflicts_resolved", self.conflicts_resolved),
            ("replication_sent", self.replication_sent),
            ("replication_received", self.replication_received),
            ("replication_rejected", self.replication_rejected),
            ("replication_stale", self.replication_stale),
            ("replication_duplicate", self.replication_duplicate),
            ("clock_changes", self.clock_changes),
            ("storage_recoveries", self.storage_recoveries),
            ("index_rebuilds", self.index_rebuilds),
            ("open_rebuild_ms_last", self.open_rebuild_ms_last),
            ("open_rebuild_ms_max", self.open_rebuild_ms_max),
            ("opens", self.opens),
        ]
    }

    pub fn value(&self, name: &str) -> u64 {
        for (n, v) in self.snapshot() {
            if n == name {
                return v;
            }
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_snapshot_roundtrip() {
        let m = Metrics {
            memory_writes: 3,
            recalls: 5,
            ..Metrics::default()
        };
        assert_eq!(m.value("memory_writes"), 3);
        assert_eq!(m.value("recalls"), 5);
        assert_eq!(m.value("nope"), 0);
        let snap = m.snapshot();
        assert!(snap.len() >= 13);
        assert!(snap.contains(&("memory_writes", 3)));
        assert!(snap.contains(&("recalls", 5)));
    }

    /// O contrato de medição do ADR-0009 §4 tem de estar exposto no snapshot
    /// (é o que o host lê para decidir o snapshot de índice).
    #[test]
    fn metrics_snapshot_exposes_open_cost() {
        let m = Metrics {
            open_rebuild_ms_last: 42,
            open_rebuild_ms_max: 99,
            opens: 7,
            ..Metrics::default()
        };
        assert_eq!(m.value("open_rebuild_ms_last"), 42);
        assert_eq!(m.value("open_rebuild_ms_max"), 99);
        assert_eq!(m.value("opens"), 7);
    }
}
