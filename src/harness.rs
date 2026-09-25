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
#[derive(Clone, Debug, Default)]
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

/// Estratégia de conflito do [`Sgdb::promote_run`] (seekdb FORK/MERGE,
/// mapeada para scopes — o core reporta, o host escolhe via este enum).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MergeStrategy {
    /// Base já tem memória com a MESMA key e texto DIFERENTE → erro (nada é
    /// escrito; o host decide manualmente).
    #[default]
    Fail,
    /// Run vence: o texto do run SOBRESCREVE o payload do primário do base
    /// (overwrite preserva memory_id — identidade estável).
    Ours,
    /// Base vence: memória conflitante do run fica no run (não promovida),
    /// listada em `report.conflicts_kept`.
    Theirs,
}

/// Relatório de [`Sgdb::promote_run`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PromoteRunReport {
    /// Storage keys promovidas (re-escopadas para o base).
    pub promoted: Vec<String>,
    /// Memórias idênticas no base — dedup, sem version bump.
    pub deduped: usize,
    /// Conflitos resolvidos com a estratégia (Ours/THEIRS), por key.
    pub conflicts: Vec<String>,
    /// THEIRS: keys que ficaram no run (base vence).
    pub conflicts_kept: Vec<String>,
}

/// Extrai a key BASE (a parte da chave, SEM `md/L{N}/`) de um sk do run:
/// o layout comum é `md/L{N}/<run>/<base>`; remove o prefixo do run. O host
/// escreve no run com keys derivadas da base por convenção — o promote
/// reverte. (O sk base completo é `md/{layer}/{base}`.)
fn base_key_of(sk: &str, run: &str) -> Option<String> {
    let rest = sk.strip_prefix("md/")?;
    let (_layer, key) = rest.split_once('/')?;
    let base = key.strip_prefix(run)?.strip_prefix('/')?;
    if base.is_empty() {
        return None;
    }
    Some(String::from(base))
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
            if archive_episodic && self.engine.get_state(&sk) == MemoryState::Active {
                self.forget(&sk)?;
                report.archived += 1;
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

    /// ADR-0010 + seekdb-analysis item 1: PROMOVE as memórias ATIVAS de um
    /// run (sandbox) para o escopo base — o MERGE do fork/merge de memória.
    ///
    /// Regras (ADD-only preservada; o core reporta, nunca inventa):
    /// - Só memórias ATIVAS do filter (L3 fatos e primários L4/L5; companions
    ///   L2 `/ts/` episódicos NÃO são promovidos — são ruído do sandbox).
    /// - Mesma key + texto igual no base → dedup (no-op).
    /// - Mesma key + texto diferente → estratégia (`Fail`/`Ours`/`Theirs`).
    /// - Key nova → re-escope dims para o base e promove.
    ///
    /// Convenção de identidade: as keys do run derivam da base por prefixo
    /// `<run>/` (ex.: base `md/L3/adr-0001` ⇒ run `md/L3/r-42/adr-0001`);
    /// `base_key_of` reverte. Keys sem o prefixo são promovidas verbatim.
    ///
    /// Requer filter não-global (nunca "promove" o universo inteiro).
    pub fn promote_run(
        &mut self,
        filter: &ScopeFilter,
        base_dims: &ScopeDims,
        strategy: MergeStrategy,
    ) -> Result<PromoteRunReport, SgdbError> {
        if filter.is_global_only() {
            return Err(SgdbError::Invalid(
                "promote_run requires a non-empty ScopeFilter (typically run=...)",
            ));
        }
        // O run não pode ser o próprio base (dims vazias = global).
        if base_dims.is_global() {
            return Err(SgdbError::Invalid(
                "promote_run requires non-empty base_dims (the sandbox must differ from the base)",
            ));
        }
        let run = base_dims.run.as_str();
        if run.is_empty() {
            return Err(SgdbError::Invalid(
                "promote_run: base_dims.run identifies the sandbox run",
            ));
        }

        // Fonte: scan de L3 + L4 + L5, filtra por dims e estado Active.
        let mut source: Vec<String> = Vec::new();
        for prefix in ["md/L3/", "md/L4/", "md/L5/"] {
            let rows = self.engine.scan_prefix_storage(prefix.as_bytes())?;
            for (k, _) in rows {
                let sk = String::from_utf8_lossy(&k).into_owned();
                if base_key_of(&sk, run).is_none() {
                    continue; // convenção: só keys com prefixo do run
                }
                if self.engine.get_state(&sk) != MemoryState::Active {
                    continue;
                }
                let dims = self.engine.effective_scope_dims(&sk);
                if dims_pass_filter(&dims, filter) {
                    source.push(sk);
                }
            }
        }
        source.sort();

        // Base dims do merge: mesmas dims SEM o run (sandbox → base).
        let mut target_dims = base_dims.clone();
        target_dims.run = alloc::string::String::new();

        let mut report = PromoteRunReport::default();
        for sk in source {
            // bk = key crua do base (sem md/L{N}/); bsk = storage key completa.
            let Some(bk) = base_key_of(&sk, run) else { continue };
            let layer = sk
                .strip_prefix("md/")
                .and_then(|r| r.split_once('/'))
                .map(|(l, _)| String::from(l))
                .unwrap_or_default();
            let bsk = alloc::format!("md/{layer}/{bk}");
            let run_text = self.text_of(&sk)?;
            // Companion do run acompanha a promoção (identidade de texto).
            let run_companion = sk
                .replacen("/L4/", "/L2/", 1)
                .replacen("/L5/", "/L2/", 1);
            let base_companion = alloc::format!("md/L2/{bk}");

            match self.engine.get_by_storage_key(&bsk) {
                Ok(Some(_)) => {
                    let base_text = self.text_of(&bsk)?;
                    if base_text == run_text {
                        report.deduped += 1;
                        continue;
                    }
                    match strategy {
                        MergeStrategy::Fail => {
                        return Err(SgdbError::Invalid(
                            "promote_run conflict: base and run texts differ (strategy=fail; resolve manually or use ours/theirs)",
                        ));
                        }
                        MergeStrategy::Ours => {
                            // Run vence: copia payload+layer do doc do run
                            // por cima do primário do base (overwrite preserva
                            // memory_id e a identidade do criador).
                            let mut doc = self
                                .engine
                                .get_by_storage_key(&sk)?
                                .ok_or(SgdbError::Invalid("promote_run: source vanished"))?;
                            doc.key = bk.clone();
                            self.engine.put(doc)?;
                            self.set_scope_dims(&bsk, &target_dims)?;
                            report.conflicts.push(bsk.clone());
                            report.promoted.push(bsk.clone());
                        }
                        MergeStrategy::Theirs => {
                            // Base vence: fica no run.
                            report.conflicts_kept.push(bsk.clone());
                        }
                    }
                }
                Ok(None) => {
                    // Key nova no base: escreve o doc do run sob a base key e
                    // re-escope. O companion L2 é copiado para o texto
                    // permanecer legível no base.
                    let mut doc = self
                        .engine
                        .get_by_storage_key(&sk)?
                        .ok_or(SgdbError::Invalid("promote_run: source vanished"))?;
                    doc.key = bk.clone();
                    self.engine.put(doc)?;
                    self.set_scope_dims(&bsk, &target_dims)?;
                    // Companion L2 só existe para primários L4/L5 (payload de
                    // embedding): copia para o texto permanecer legível no
                    // base. L3 é texto direto — sem companion.
                    if (sk.contains("/L4/") || sk.contains("/L5/"))
                        && self.engine.get_by_storage_key(&run_companion)?.is_some()
                        && self.engine.get_by_storage_key(&base_companion)?.is_none()
                    {
                        let mut cd = self
                            .engine
                            .get_by_storage_key(&run_companion)?
                            .ok_or(SgdbError::Invalid("promote_run: companion vanished"))?;
                        cd.layer = crate::memory_doc::MemoryLayer::L2EpisodicShort;
                        cd.key = bk.clone();
                        self.engine.put(cd)?;
                        self.set_scope_dims(&base_companion, &target_dims)?;
                    }
                    report.promoted.push(bsk.clone());
                }
                Err(e) => return Err(e),
            }
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
        if anti && !ents.contains(&MOM_ANTI_PATTERN) {
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

    // ── promote_run (fork/merge de memória, seekdb-analysis item 1) ────

    const BASE_DIMS: fn(&str) -> ScopeDims = |run: &str| ScopeDims {
        run: alloc::string::String::from(run),
        ..ScopeDims::new()
    };

    fn write_run_fact(db: &mut Sgdb, run: &str, key: &str, text: &str) -> String {
        let dims = BASE_DIMS(run);
        let out = db
            .remember_text_with(
                &alloc::format!("{run}/{key}"),
                text,
                RememberOptions {
                    scope_dims: Some(dims),
                    ..Default::default()
                },
            )
            .unwrap();
        out.storage_key
    }

    fn write_base_fact(db: &mut Sgdb, key: &str, text: &str) -> String {
        let out = db
            .remember_text_with(key, text, RememberOptions::default())
            .unwrap();
        out.storage_key
    }

    #[test]
    fn promote_run_promotes_new_keys_to_base_scope() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let sk = write_run_fact(&mut db, "r-42", "adr-0001", "decisao do sandbox");
        assert_eq!(db.get_state(&sk).unwrap(), MemoryState::Active);

        let r = db
            .promote_run(&run_filter("r-42"), &BASE_DIMS("r-42"), MergeStrategy::Fail)
            .unwrap();
        assert_eq!(r.promoted.len(), 1);
        assert_eq!(r.promoted[0], "md/L3/adr-0001");
        // No base (run vazio): recall global enxerga; escopo do run não.
        let base = db.recall_lexical("decisao do sandbox", 5).unwrap();
        assert_eq!(base.len(), 1);
        assert_eq!(base[0].key, "md/L3/adr-0001");
        let dims = db.scope_dims_of("md/L3/adr-0001").unwrap();
        assert!(dims.run.is_empty(), "promovida deve ficar SEM run");
    }

    #[test]
    fn promote_run_dedups_identical_text() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let _ = write_base_fact(&mut db, "fact-x", "texto igual");
        let _ = write_run_fact(&mut db, "r-1", "fact-x", "texto igual");
        let r = db
            .promote_run(&run_filter("r-1"), &BASE_DIMS("r-1"), MergeStrategy::Fail)
            .unwrap();
        assert_eq!(r.deduped, 1);
        assert!(r.promoted.is_empty());
        // Sem version bump: só uma memória com o texto no base.
        let hits = db.recall_lexical("texto igual", 10).unwrap();
        assert_eq!(hits.len(), 1, "dedup não deve criar segunda memória");
    }

    #[test]
    fn promote_run_fail_strategy_refuses_and_writes_nothing() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let _ = write_base_fact(&mut db, "adr-9", "versao base");
        let _ = write_run_fact(&mut db, "r-2", "adr-9", "versao run");
        let err = db
            .promote_run(&run_filter("r-2"), &BASE_DIMS("r-2"), MergeStrategy::Fail)
            .unwrap_err();
        assert!(matches!(err, SgdbError::Invalid(_)));
        // Nada escrito: base preserva o texto.
        assert_eq!(db.text_of("md/L3/adr-9").unwrap(), "versao base");
    }

    #[test]
    fn promote_run_ours_strategy_run_wins_overwrite() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let base_sk = write_base_fact(&mut db, "adr-7", "versao base");
        let base_id = db.meta(&base_sk).unwrap().unwrap().memory_id.clone();
        let _ = write_run_fact(&mut db, "r-3", "adr-7", "versao run vence");
        let r = db
            .promote_run(&run_filter("r-3"), &BASE_DIMS("r-3"), MergeStrategy::Ours)
            .unwrap();
        assert_eq!(r.conflicts.len(), 1);
        assert_eq!(db.text_of("md/L3/adr-7").unwrap(), "versao run vence");
        // Identidade estável: overwrite preserva memory_id.
        assert_eq!(db.meta(&base_sk).unwrap().unwrap().memory_id, base_id);
        // Re-escopada para o base.
        assert!(db.scope_dims_of("md/L3/adr-7").unwrap().run.is_empty());
    }

    #[test]
    fn promote_run_theirs_strategy_base_wins_stays_in_run() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        let _ = write_base_fact(&mut db, "adr-8", "versao base vence");
        let sk = write_run_fact(&mut db, "r-4", "adr-8", "versao run");
        let r = db
            .promote_run(&run_filter("r-4"), &BASE_DIMS("r-4"), MergeStrategy::Theirs)
            .unwrap();
        assert_eq!(r.conflicts_kept.len(), 1);
        assert!(r.promoted.is_empty());
        assert_eq!(db.text_of("md/L3/adr-8").unwrap(), "versao base vence");
        // A memória do run permanece no run, intacta.
        assert_eq!(db.get_state(&sk).unwrap(), MemoryState::Active);
        assert_eq!(db.scope_dims_of(&sk).unwrap().run, "r-4");
    }

    #[test]
    fn promote_run_ignores_other_runs_and_episodics_and_rejects_global() {
        let mut db = Sgdb::open(InMemory::new()).unwrap();
        // Run alvo
        let _ = write_run_fact(&mut db, "r-5", "k1", "fato alvo");
        // Outro run — não deve ser promovido.
        let _ = write_run_fact(&mut db, "r-outro", "k2", "fato de outro run");
        // Episódico /ts/ do run — ruído do sandbox, nunca promovido.
        let dims = BASE_DIMS("r-5");
        let (ep, _) = db
            .remember_episodic_scoped("q", "a", 10, &dims)
            .unwrap();
        let r = db
            .promote_run(&run_filter("r-5"), &BASE_DIMS("r-5"), MergeStrategy::Fail)
            .unwrap();
        assert_eq!(r.promoted.len(), 1);
        assert!(!r.promoted[0].contains("r-outro"));
        assert!(r.promoted[0].ends_with("k1"));
        // Episódico permanece no run.
        assert_eq!(db.scope_dims_of(&ep).unwrap().run, "r-5");
        // Guardas.
        assert!(matches!(
            db.promote_run(&ScopeFilter::global(), &BASE_DIMS("r-5"), MergeStrategy::Fail),
            Err(SgdbError::Invalid(_))
        ));
        assert!(matches!(
            db.promote_run(&run_filter("r-5"), &ScopeDims::new(), MergeStrategy::Fail),
            Err(SgdbError::Invalid(_))
        ));
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
