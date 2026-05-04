//! Default values for invariant numerics shared across primer-core
//! modules. Per the no-magic-numbers convention, every numeric used
//! by primer-core helpers is defined here (or in a sibling settings
//! struct field for tunables).

/// Defaults for the retry helper. Mirrors the per-crate `consts.rs`
/// pattern used by `primer-classifier`, `primer-extractor`, etc.
pub mod retry {
    use std::time::Duration;

    /// Total attempts including the first.
    pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;

    /// Initial backoff delay before the second attempt.
    pub const DEFAULT_BASE_DELAY: Duration = Duration::from_millis(250);

    /// Multiplicative growth factor between attempts (250 → 500 → 1000 ms).
    pub const DEFAULT_BACKOFF_FACTOR: u32 = 2;

    /// Jitter as a fraction of the computed delay (±50 %).
    pub const DEFAULT_JITTER_FRACTION: f32 = 0.5;

    /// Maximum honored Retry-After. Above this we give up immediately
    /// rather than block the conversational hot path.
    pub const DEFAULT_RETRY_AFTER_BUDGET: Duration = Duration::from_secs(5);
}
