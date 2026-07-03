//! Defaults for the vocabulary spaced-repetition feature.
//!
//! See [`crate::vocab`] and the design spec at
//! `docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md`.

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
