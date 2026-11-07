//! Defaults for the retry helper. Mirrors the per-crate `consts.rs`
//! pattern used by `primer-classifier`, `primer-extractor`, etc.

use std::time::Duration;

/// Total attempts including the first.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// Initial backoff delay before the second attempt.
pub const DEFAULT_BASE_DELAY: Duration = Duration::from_millis(250);

/// Multiplicative growth factor between attempts (250 → 500 ms at the default 3 attempts).
pub const DEFAULT_BACKOFF_FACTOR: u32 = 2;

/// Jitter as a fraction of the computed delay (±50 %).
pub const DEFAULT_JITTER_FRACTION: f32 = 0.5;

/// Maximum honored Retry-After. Above this we give up immediately
/// rather than block the conversational hot path.
pub const DEFAULT_RETRY_AFTER_BUDGET: Duration = Duration::from_secs(5);

/// Pre-stream attempts for a decorated leg that has another leg behind it
/// (`FallbackBackend` primary, `RouterBackend` first leg): the first try
/// plus one retry, then the decorator fails over, so a down first leg
/// can't stack its whole budget in front of the second leg's (issue #312).
/// The last leg keeps the full budget.
pub const FAILOVER_LEG_MAX_ATTEMPTS: u32 = 2;

// Owner policy: a leg always gets at least one retry before failover.
const _: () = assert!(FAILOVER_LEG_MAX_ATTEMPTS >= 2);
