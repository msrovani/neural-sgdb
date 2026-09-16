//! Staleness report (host harvest 1): classify memories — never mutate.
//!
//! Levels: Expired (TTL) > Stale (Decayed/Invalidated/contradicts) > Aging
//! (age since last_reinforced|created_tick) > Fresh. Units of `now` match
//! the host clock used for TTL / `decay_importance`.

use alloc::string::String;
use alloc::vec::Vec;

/// Severity of a classified memory (higher = more urgent).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StalenessLevel {
    Fresh = 0,
    Aging = 1,
    Stale = 2,
    Expired = 3,
}

impl StalenessLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            StalenessLevel::Fresh => "fresh",
            StalenessLevel::Aging => "aging",
            StalenessLevel::Stale => "stale",
            StalenessLevel::Expired => "expired",
        }
    }
}

/// Why a memory was classified above Fresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StalenessReason {
    TtlExpired,
    Decayed,
    Invalidated,
    Contradiction,
    Aging,
}

impl StalenessReason {
    pub fn as_str(self) -> &'static str {
        match self {
            StalenessReason::TtlExpired => "ttl_expired",
            StalenessReason::Decayed => "decayed",
            StalenessReason::Invalidated => "invalidated",
            StalenessReason::Contradiction => "contradiction",
            StalenessReason::Aging => "aging",
        }
    }
}

/// Thresholds in the same units as host `now` (often ms wall-clock).
#[derive(Clone, Debug, PartialEq)]
pub struct StalenessConfig {
    /// Age above this → Aging (if not already Stale/Expired).
    pub aging_after: u64,
}

impl Default for StalenessConfig {
    fn default() -> Self {
        Self {
            aging_after: 7 * 24 * 3600 * 1000, // 7 days if `now` is ms
        }
    }
}

/// One classified storage key (read-only; agent decides curate).
#[derive(Clone, Debug, PartialEq)]
pub struct StalenessHit {
    pub key: String,
    pub level: StalenessLevel,
    pub reasons: Vec<StalenessReason>,
    pub age: u64,
    pub expires_at: Option<u64>,
    pub recommendation: &'static str,
}

impl StalenessHit {
    pub fn recommendation_for(level: StalenessLevel) -> &'static str {
        match level {
            StalenessLevel::Fresh => "keep",
            StalenessLevel::Aging => "monitor_or_reinforce",
            StalenessLevel::Stale => "review_supersede_or_forget",
            StalenessLevel::Expired => "expire_ttl_or_delete",
        }
    }
}
