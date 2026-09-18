//! Harness Engineering facade (ADR-0010): `commit_run`, `deprecate_run`.
//!
//! Orchestrates existing cognitive verbs — core does not decide content.
//! No new MemoryState; no MDM1 bump; NMD1/TKLV untouched.

use alloc::string::String;
use alloc::vec::Vec;

use crate::memory_doc::{MemoryState, ScopeDims, ScopeFilter};
use crate::sgdb::{
    ConsolidateConfig, RememberOptions, RememberOutcome, Sgdb,
};
use crate::storage::SgdbError;

/// Canonical MOM entity for first-class anti-patterns (ADR-0010).
pub const MOM_ANTI_PATTERN: &str = "mom/anti-pattern";

/// One fact or anti-pattern the upper layer chose to promote at task end.
#[derive(Clone, Debug)]
pub struct CommitFact<'a> {
    pub key: &'a str,
    pub text: &'a str,
    pub entities: &'a [&'a str],
    pub content_type: Option<&'a str>,
    /// `Some` → L4 semantic; `None` → L3 lexical (ADR-0008).
    pub embedding: Option<&'a [f32]>,
}

/// Explicit supersede pair (full storage keys).
#[derive(Clone, Debug)]
pub struct CommitSupersede<'a> {
    pub old: &'a str,
    pub new: &'a str,
}

/// End-of-task flush plan (upper layer supplies all content).
#[derive(Clone, Debug)]
pub struct CommitRunPlan<'a> {
    pub facts: &'a [CommitFact<'a>],
    pub anti_patterns: &'a [CommitFact<'a>],
    pub supersede: &'a [CommitSupersede<'a>],
    /// Archive leftover `md/L2/.../ts/...` episodics matching the filter.
    pub archive_remaining_episodic: bool,
    /// If set with `now`, apply `set_ttl(key, now + ms)` on those episodics.
    pub ttl_episodic_ms: Option<u64>,
    pub close_event_key: Option<&'a str>,
    pub now: u64,
    pub audit: bool,
    /// Dims applied to new writes when the plan does not override per-fact.
    pub write_dims: Option<ScopeDims>,
}

impl Default for CommitRunPlan<'static> {
    fn default() -> Self {
        Self {
            facts: &[],
            anti_patterns: &[],
            supersede: &[],
            archive_remaining_episodic: false,
            ttl_episodic_ms: None,
            close_event_key: None,
            now: 0,
            audit: false,
            write_dims: None,
        }
    }
}

/// Auditável resultado de [`Sgdb::commit_run`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitRunReport {
    pub written: Vec<String>,
    pub superseded: usize,
    pub archived: usize,
    pub ttl_set: usize,
    pub closed_event: bool,
    pub audit_seq: Option<u64>,
}

/// Relatório de [`Sgdb::deprecate_run`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeprecateRunReport {
    pub archived: usize,
    pub ttl_set: usize,
}

/// Null-scoping-aware filter match (same rule as recall_*_dims).
pub(crate) fn dims_pass_filter(dims: &ScopeDims, filter: &ScopeFilter) -> bool {
    if filter.is_global_only() {
        dims.is_global()
    } else {
        dims.matches(filter)
    }
}

fn is_timestamped_episodic(sk: &str) -> bool {
    sk.starts_with("md/L2/") && sk.contains("/ts/")
}

impl Sgdb {
    /// ADR-0010: consolidate recurrences only for episodics matching `filter`.
    pub fn consolidate_recurrences_scoped(
        &mut self,
        cfg: &ConsolidateConfig,
        filter: &ScopeFilter,
    ) -> Result<usize, SgdbError> {
        self.consolidate_recurrences_filtered(cfg, Some(filter))
    }

    /// Soft-close a run: archive and/or TTL timestamped L2 episodics in filter.
    /// Requires a non-empty `ScopeFilter` (typically `run=…`) — refuses to
    /// archive the global universe by accident.
    pub fn deprecate_run(
        &mut self,
        filter: &ScopeFilter,
        archive_episodic: bool,
        ttl_expires_at: Option<u64>,
    ) -> Result<DeprecateRunReport, SgdbError> {
        if filter.is_global_only() {
            return Err(SgdbError::Invalid(
                "deprecate_run requires a non-empty ScopeFilter (typically run=...)",
            ));
        }
        if !archive_episodic && ttl_expires_at.is_none() {
            return Ok(DeprecateRunReport::default());
        }
        let mut report = DeprecateRunReport::default();
        let keys = self.list_timestamped_episodic(filter)?;
        for sk in keys {
            if archive_episodic {
                match self.engine.get_state(&sk) {
                    MemoryState::Active => {
                        self.forget(&sk)?;
                        report.archived += 1;
                    }
                    _ => {}
                }
            }
            if let Some(exp) = ttl_expires_at {
                if self.engine.get_by_storage_key(&sk)?.is_some() {
                    self.set_ttl(&sk, exp)?;
                    report.ttl_set += 1;
                }
            }
        }
        Ok(report)
    }

    /// End-of-task flush (ADR-0010): write facts/anti-patterns, supersede,
    /// optionally archive/TTL remaining episodics, close event, audit.
    ///
    /// Archive/TTL require a non-empty `filter` (same guard as `deprecate_run`).
    pub fn commit_run(
        &mut self,
        filter: &ScopeFilter,
        plan: &CommitRunPlan<'_>,
    ) -> Result<CommitRunReport, SgdbError> {
        if (plan.archive_remaining_episodic || plan.ttl_episodic_ms.is_some())
            && filter.is_global_only()
        {
            return Err(SgdbError::Invalid(
                "commit_run archive/ttl requires a non-empty ScopeFilter (typically run=...)",
            ));
        }

        let mut report = CommitRunReport::default();
        let write_dims = plan.write_dims.clone();

        for fact in plan.facts {
            let out = self.write_commit_fact(fact, write_dims.as_ref(), false)?;
            report.written.push(out.storage_key);
        }
        for ap in plan.anti_patterns {
            let out = self.write_commit_fact(ap, write_dims.as_ref(), true)?;
            report.written.push(out.storage_key);
        }

        for pair in plan.supersede {
            self.supersede(pair.old, pair.new)?;
            report.superseded += 1;
        }

        if plan.archive_remaining_episodic || plan.ttl_episodic_ms.is_some() {
            let ttl_abs = plan
                .ttl_episodic_ms
                .map(|ms| plan.now.saturating_add(ms));
            let dep = self.deprecate_run(
                filter,
                plan.archive_remaining_episodic,
                ttl_abs,
            )?;
            report.archived = dep.archived;
            report.ttl_set = dep.ttl_set;
        }

        if let Some(ek) = plan.close_event_key {
            if !ek.is_empty() {
                self.close_event(ek, plan.now)?;
                report.closed_event = true;
            }
        }

        if plan.audit {
            let seq = self.audit_checkpoint(plan.now)?;
            report.audit_seq = Some(seq);
        }

        Ok(report)
    }

    /// Episodic L2 with optional multi-dim scope (harness write path).
    pub fn remember_episodic_scoped(
        &mut self,
        user: &str,
        response: &str,
        now: u64,
        dims: &ScopeDims,
    ) -> Result<(String, String), SgdbError> {
        let (ku, ka) = self.remember_episodic(user, response, now)?;
        if !dims.is_global() {
            self.set_scope_dims(&ku, dims)?;
            self.set_scope_dims(&ka, dims)?;
        }
        Ok((ku, ka))
    }

    fn write_commit_fact(
        &mut self,
        fact: &CommitFact<'_>,
        write_dims: Option<&ScopeDims>,
        anti: bool,
    ) -> Result<RememberOutcome, SgdbError> {
        if fact.key.is_empty() || fact.text.is_empty() {
            return Err(SgdbError::Invalid(
                "commit_run fact requires non-empty key and text",
            ));
        }
        let mut ents: Vec<&str> = fact.entities.to_vec();
        if anti && !ents.iter().any(|e| *e == MOM_ANTI_PATTERN) {
            ents.insert(0, MOM_ANTI_PATTERN);
        }
        let opts = RememberOptions {
            scope: None,
            entities: &ents,
            content_type: fact.content_type,
            scope_dims: write_dims.cloned(),
            model_id: None,
        };
        match fact.embedding {
            Some(emb) => self.remember_semantic_with(fact.key, fact.text, emb, opts),
            None => self.remember_text_with(fact.key, fact.text, opts),
        }
    }

    fn list_timestamped_episodic(
        &mut self,
        filter: &ScopeFilter,
    ) -> Result<Vec<String>, SgdbError> {
        let rows = self.engine.scan_prefix_storage(b"md/L2/")?;
        let mut out = Vec::new();
        for (k, _) in rows {
            let sk = String::from_utf8_lossy(&k).into_owned();
            if !is_timestamped_episodic(&sk) {
                continue;
            }
            let dims = self.engine.effective_scope_dims(&sk);
            if dims_pass_filter(&dims, filter) {
                out.push(sk);
            }
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_doc::ScopeDims;
    use crate::storage::InMemory;

    fn run_filter(run: &str) -> ScopeFilter {
        ScopeFilter {
            user: None,
            agent: None,
            app: None,
            run: Some(String::from(run)),
        }
    }

    #[test]
    fn commit_run_writes_facts_and_anti_patterns() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let dims = ScopeDims {
            user: String::new(),
            agent: String::new(),
            app: String::new(),
            run: "task-1".into(),
        };
        let filter = run_filter("task-1");
        let facts = [CommitFact {
            key: "fact/ok",
            text: "use sqrt_f32 no bare metal",
            entities: &["mom/fact", "arch/rev/1"],
            content_type: Some("text"),
            embedding: None,
        }];
        let antis = [CommitFact {
            key: "avoid/f32-sqrt",
            text: "nunca chamar f32::sqrt em x86_64-unknown-none",
            entities: &["avoid/f32-sqrt"],
            content_type: Some("text"),
            embedding: None,
        }];
        let plan = CommitRunPlan {
            facts: &facts,
            anti_patterns: &antis,
            write_dims: Some(dims),
            ..CommitRunPlan::default()
        };
        let r = db.commit_run(&filter, &plan).unwrap();
        assert_eq!(r.written.len(), 2);
        assert!(r.written[0].starts_with("md/L3/"));
        let hits = db
            .recall_entities_dims(&[MOM_ANTI_PATTERN], 5, &filter)
            .unwrap();
        assert!(
            hits.iter().any(|h| h.text.contains("f32::sqrt")),
            "anti-pattern recuperável via mom/anti-pattern"
        );
        let pos = db
            .recall_entities_dims(&["mom/fact"], 5, &filter)
            .unwrap();
        assert!(pos.iter().any(|h| h.text.contains("sqrt_f32")));
    }

    #[test]
    fn commit_run_archives_remaining_episodic_in_run() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let dims = ScopeDims {
            run: "task-2".into(),
            ..ScopeDims::new()
        };
        let (ku, ka) = db
            .remember_episodic_scoped("pergunta ruido", "resposta ruido", 1000, &dims)
            .unwrap();
        assert_eq!(db.get_state(&ku).unwrap(), MemoryState::Active);

        let facts = [CommitFact {
            key: "fact/clean",
            text: "fato limpo da tarefa",
            entities: &["mom/fact"],
            content_type: None,
            embedding: None,
        }];
        let plan = CommitRunPlan {
            facts: &facts,
            archive_remaining_episodic: true,
            write_dims: Some(dims.clone()),
            now: 2000,
            ..CommitRunPlan::default()
        };
        let r = db.commit_run(&run_filter("task-2"), &plan).unwrap();
        assert_eq!(r.written.len(), 1);
        assert!(r.archived >= 2, "archived={}", r.archived);
        assert_eq!(db.get_state(&ku).unwrap(), MemoryState::Archived);
        assert_eq!(db.get_state(&ka).unwrap(), MemoryState::Archived);
        // fato novo permanece Active
        assert_eq!(
            db.get_state(&r.written[0]).unwrap(),
            MemoryState::Active
        );
    }

    #[test]
    fn commit_run_archive_rejects_empty_filter() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let plan = CommitRunPlan {
            archive_remaining_episodic: true,
            ..CommitRunPlan::default()
        };
        let err = db.commit_run(&ScopeFilter::global(), &plan).unwrap_err();
        assert!(matches!(err, SgdbError::Invalid(_)));
    }

    #[test]
    fn deprecate_run_sets_ttl() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let dims = ScopeDims {
            run: "task-3".into(),
            ..ScopeDims::new()
        };
        let (ku, _) = db
            .remember_episodic_scoped("q", "a", 50, &dims)
            .unwrap();
        let r = db
            .deprecate_run(&run_filter("task-3"), false, Some(999))
            .unwrap();
        assert_eq!(r.ttl_set, 2);
        assert_eq!(db.ttl_of(&ku).unwrap(), Some(999));
    }

    #[test]
    fn consolidate_recurrences_scoped_ignores_other_run() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let d1 = ScopeDims {
            run: "r-a".into(),
            ..ScopeDims::new()
        };
        let d2 = ScopeDims {
            run: "r-b".into(),
            ..ScopeDims::new()
        };
        let q = "onde fica a sala de reunioes?";
        for (i, d) in [(1000u64, &d1), (2000, &d1), (3000, &d1), (4000, &d2)].iter() {
            let _ = db
                .remember_episodic_scoped(q, &alloc::format!("resp {i}"), *i, d)
                .unwrap();
        }
        let cfg = ConsolidateConfig {
            min_repeats: 3,
            min_len: 4,
            max_new: 8,
        };
        let made = db
            .consolidate_recurrences_scoped(&cfg, &run_filter("r-a"))
            .unwrap();
        assert_eq!(made, 1);
        // r-b só tem 1 — scoped não consolida sozinho
        let made_b = db
            .consolidate_recurrences_scoped(&cfg, &run_filter("r-b"))
            .unwrap();
        assert_eq!(made_b, 0);
    }

    #[test]
    fn commit_run_supersede_and_audit() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let dims = ScopeDims {
            run: "task-4".into(),
            ..ScopeDims::new()
        };
        let old = db
            .remember_text_with(
                "old",
                "solucao antiga",
                RememberOptions {
                    scope_dims: Some(dims.clone()),
                    entities: &["mom/decision"],
                    ..Default::default()
                },
            )
            .unwrap();
        let facts = [CommitFact {
            key: "new",
            text: "solucao nova",
            entities: &["mom/decision", "arch/rev/2"],
            content_type: None,
            embedding: None,
        }];
        let plan1 = CommitRunPlan {
            facts: &facts,
            write_dims: Some(dims),
            audit: true,
            now: 42,
            ..CommitRunPlan::default()
        };
        let r1 = db.commit_run(&run_filter("task-4"), &plan1).unwrap();
        assert!(r1.audit_seq.is_some());
        let pairs = [CommitSupersede {
            old: old.storage_key.as_str(),
            new: r1.written[0].as_str(),
        }];
        let plan2 = CommitRunPlan {
            supersede: &pairs,
            ..CommitRunPlan::default()
        };
        let r2 = db.commit_run(&run_filter("task-4"), &plan2).unwrap();
        assert_eq!(r2.superseded, 1);
        assert_eq!(
            db.get_state(&old.storage_key).unwrap(),
            MemoryState::Superseded
        );
    }

    #[test]
    fn archive_run_a_does_not_touch_run_b() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let da = ScopeDims {
            run: "iso-a".into(),
            ..ScopeDims::new()
        };
        let dims_b = ScopeDims {
            run: "iso-b".into(),
            ..ScopeDims::new()
        };
        let (au, _) = db
            .remember_episodic_scoped("qa", "ra", 10, &da)
            .unwrap();
        let (bu, _) = db
            .remember_episodic_scoped("qb", "rb", 20, &dims_b)
            .unwrap();
        let r = db
            .deprecate_run(&run_filter("iso-a"), true, None)
            .unwrap();
        assert_eq!(r.archived, 2);
        assert_eq!(db.get_state(&au).unwrap(), MemoryState::Archived);
        assert_eq!(
            db.get_state(&bu).unwrap(),
            MemoryState::Active,
            "run B deve permanecer Active"
        );
    }

    #[test]
    fn deprecate_run_rejects_empty_filter_and_is_idempotent() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        assert!(matches!(
            db.deprecate_run(&ScopeFilter::global(), true, None),
            Err(SgdbError::Invalid(_))
        ));
        let dims = ScopeDims {
            run: "idem".into(),
            ..ScopeDims::new()
        };
        let _ = db
            .remember_episodic_scoped("q", "a", 1, &dims)
            .unwrap();
        let r1 = db.deprecate_run(&run_filter("idem"), true, None).unwrap();
        assert_eq!(r1.archived, 2);
        let r2 = db.deprecate_run(&run_filter("idem"), true, None).unwrap();
        assert_eq!(r2.archived, 0, "segunda passada não re-arquiva");
    }

    #[test]
    fn commit_run_rejects_empty_fact_and_sets_ttl() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let dims = ScopeDims {
            run: "ttl-1".into(),
            ..ScopeDims::new()
        };
        let (ku, _) = db
            .remember_episodic_scoped("ep", "ody", 5, &dims)
            .unwrap();
        let bad = [CommitFact {
            key: "",
            text: "x",
            entities: &[],
            content_type: None,
            embedding: None,
        }];
        let err = db
            .commit_run(
                &run_filter("ttl-1"),
                &CommitRunPlan {
                    facts: &bad,
                    write_dims: Some(dims.clone()),
                    ..CommitRunPlan::default()
                },
            )
            .unwrap_err();
        assert!(matches!(err, SgdbError::Invalid(_)));

        let r = db
            .commit_run(
                &run_filter("ttl-1"),
                &CommitRunPlan {
                    archive_remaining_episodic: false,
                    ttl_episodic_ms: Some(100),
                    now: 1000,
                    write_dims: Some(dims),
                    ..CommitRunPlan::default()
                },
            )
            .unwrap();
        assert_eq!(r.ttl_set, 2);
        assert_eq!(db.ttl_of(&ku).unwrap(), Some(1100));
        assert_eq!(db.get_state(&ku).unwrap(), MemoryState::Active);
    }

    #[test]
    fn commit_run_does_not_archive_l4_companion() {
        // Companions md/L2/<key> (sem /ts/) não são episódicos de tarefa.
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let dims = ScopeDims {
            run: "comp".into(),
            ..ScopeDims::new()
        };
        let out = db
            .remember_semantic_with(
                "sem/k",
                "texto semântico",
                &[1.0, -1.0, 1.0, -1.0],
                RememberOptions {
                    scope_dims: Some(dims.clone()),
                    ..Default::default()
                },
            )
            .unwrap();
        let _ = db
            .remember_episodic_scoped("noise", "n", 7, &dims)
            .unwrap();
        let r = db
            .commit_run(
                &run_filter("comp"),
                &CommitRunPlan {
                    archive_remaining_episodic: true,
                    write_dims: Some(dims),
                    ..CommitRunPlan::default()
                },
            )
            .unwrap();
        assert_eq!(r.archived, 2, "só o par /ts/u+/ts/a");
        assert_eq!(
            db.get_state(&out.storage_key).unwrap(),
            MemoryState::Active
        );
        assert_eq!(
            db.get_state(&out.companion_key).unwrap(),
            MemoryState::Active,
            "companion L2 de L4 não deve ser Archived"
        );
    }

    #[test]
    fn anti_pattern_entity_not_duplicated_when_already_present() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let dims = ScopeDims {
            run: "dedup".into(),
            ..ScopeDims::new()
        };
        let antis = [CommitFact {
            key: "ap1",
            text: "evitar unwrap em no_std",
            entities: &[MOM_ANTI_PATTERN, "avoid/unwrap"],
            content_type: None,
            embedding: None,
        }];
        let r = db
            .commit_run(
                &run_filter("dedup"),
                &CommitRunPlan {
                    anti_patterns: &antis,
                    write_dims: Some(dims),
                    ..CommitRunPlan::default()
                },
            )
            .unwrap();
        let ents = db.entities_of(&r.written[0]).unwrap();
        let count = ents.iter().filter(|e| e.as_str() == MOM_ANTI_PATTERN).count();
        assert_eq!(count, 1, "entities={ents:?}");
    }

    #[test]
    fn scoped_consolidate_inherits_run_no_global_leak() {
        // Inconsistência corrigida: L3 consolidado herdava nada → vazava global.
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let dims = ScopeDims {
            run: "cons-scope".into(),
            ..ScopeDims::new()
        };
        let q = "qual o horario do daily standup?";
        for i in 0u64..3 {
            let _ = db
                .remember_episodic_scoped(q, &alloc::format!("r{i}"), 100 + i, &dims)
                .unwrap();
        }
        let cfg = ConsolidateConfig {
            min_repeats: 3,
            min_len: 4,
            max_new: 4,
        };
        assert_eq!(
            db.consolidate_recurrences_scoped(&cfg, &run_filter("cons-scope"))
                .unwrap(),
            1
        );
        let global = db.recall_lexical(q, 5).unwrap();
        assert!(
            !global.iter().any(|h| h.key.contains("consolidated/")),
            "fato consolidado NÃO deve aparecer no recall global: {global:?}"
        );
        let scoped = db
            .recall_lexical_dims(q, 5, &run_filter("cons-scope"))
            .unwrap();
        assert!(
            scoped.iter().any(|h| h.key.contains("consolidated/")),
            "deve aparecer no scope do run: {scoped:?}"
        );
    }

    #[test]
    fn commit_run_null_scoping_hides_facts_from_global_recall() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let dims = ScopeDims {
            run: "null-s".into(),
            ..ScopeDims::new()
        };
        let facts = [CommitFact {
            key: "secret/fact",
            text: "xyzzy-harness-unique-token",
            entities: &["mom/fact"],
            content_type: None,
            embedding: None,
        }];
        let _ = db
            .commit_run(
                &run_filter("null-s"),
                &CommitRunPlan {
                    facts: &facts,
                    write_dims: Some(dims),
                    ..CommitRunPlan::default()
                },
            )
            .unwrap();
        let g = db.recall_lexical("xyzzy-harness-unique-token", 5).unwrap();
        assert!(g.is_empty(), "null-scoping: global vazia, got {g:?}");
        let s = db
            .recall_lexical_dims("xyzzy-harness-unique-token", 5, &run_filter("null-s"))
            .unwrap();
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn dims_pass_filter_null_scoping_contract() {
        let global = ScopeDims::new();
        let scoped = ScopeDims {
            run: "r".into(),
            ..ScopeDims::new()
        };
        assert!(dims_pass_filter(&global, &ScopeFilter::global()));
        assert!(!dims_pass_filter(&scoped, &ScopeFilter::global()));
        assert!(dims_pass_filter(&scoped, &run_filter("r")));
        assert!(!dims_pass_filter(&scoped, &run_filter("other")));
    }
}
