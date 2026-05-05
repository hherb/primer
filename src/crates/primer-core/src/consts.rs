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

/// Defaults for the vocabulary spaced-repetition feature.
///
/// See [`crate::vocab`] and the design spec at
/// `docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md`.
pub mod vocab {

    /// Box-level interval table (days). Index = `box_level`.
    /// - box 0 (freshly seen / failed) → review after 1 day
    /// - box 1 (one successful review) → 3 days
    /// - box 2 (two)                    → 7 days
    /// - box 3 (three)                  → 14 days
    /// - box 4 (max — never graduates)  → 30 days
    pub const BOX_INTERVALS_DAYS: &[u32] = &[1, 3, 7, 14, 30];

    /// Highest `box_level` a concept can occupy. After reaching this, further
    /// successful reviews keep `box_level` pinned at MAX (interval stays 30d).
    /// There is no terminal "graduated" state — review continues every 30d
    /// until either the child consistently fails (depth=Aware → box reset)
    /// or the concept is genuinely never engaged with again.
    pub const MAX_BOX_LEVEL: u8 = 4;

    /// Minimum confidence for a comprehension assessment to count toward
    /// box advancement. Assessments below this threshold reset the box to 0.
    /// Numerically equal to the comprehension classifier's
    /// `confidence_threshold` (also 0.6) but kept independent so a future
    /// researcher can tune box-advancement strictness without affecting
    /// depth promotion.
    pub const MIN_CONF_FOR_BOX_PROMOTION: f32 = 0.6;

    /// Default cap on overdue concepts injected into the system prompt
    /// per turn. Configurable via `VocabSettings::max_per_prompt` and the
    /// `--vocab-max-per-prompt` CLI flag.
    pub const DEFAULT_VOCAB_MAX_PER_PROMPT: usize = 4;
}
