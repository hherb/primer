//! Tunables for the Phase 1.3 inference router (see
//! docs/superpowers/specs/2026-06-07-inference-router-design.md).
//!
//! These weights and the threshold are starting values; they need
//! calibration against real usage data (like the bench numbers) and are
//! deliberately gathered here so that tuning never touches logic.

/// Route to the secondary (strong) leg when `complexity_score` reaches
/// this value, in `hybrid` mode. Set deliberately above the heaviest
/// single-intent weight (`Scaffolding`, 0.45) so no intent alone routes to
/// the cloud: a heavy intent must combine with at least one more signal (a
/// retrieved passage or a long/multi-question message). Privacy-preferring.
pub const ROUTE_SECONDARY_THRESHOLD: f32 = 0.5;

/// Retrieved-passage count is clamped to this before scoring, so a large
/// retrieval cannot dominate the score.
pub const ROUTE_PASSAGE_CAP: usize = 3;
/// Per-passage score weight (after the cap).
pub const W_PASSAGE: f32 = 0.15;

/// A child message with more than this many words contributes the long-
/// message weight.
pub const MSG_LONG_WORDS: usize = 30;
/// Weight added for a long child message.
pub const W_MSG_LONG: f32 = 0.20;
/// Weight added per question mark beyond the first, in the child message.
pub const W_MSG_QUESTION: f32 = 0.10;
/// Question marks beyond the first are counted up to this cap.
pub const MSG_QUESTION_CAP: usize = 2;

/// Score added to a turn's complexity when the primary leg's recent
/// time-to-first-token EMA exceeds the configured budget, in `hybrid`
/// mode. A *weight*, not a threshold — it only contributes when a budget
/// is configured (`--primary-ttft-budget-ms` / the GUI field), and it is
/// deliberately BELOW `ROUTE_SECONDARY_THRESHOLD` (0.5): latency is a
/// NUDGE that tips a borderline-complex turn over the line, not a sole
/// trigger. A trivial turn (base score 0) therefore stays local even when
/// the local leg is slow — which keeps routine turns sampling the local
/// TTFT so the EMA self-heals when local speeds back up. Starting value;
/// the real budget is owner-calibrated from bench numbers.
pub const W_LATENCY: f32 = 0.30;

/// Exponential-moving-average smoothing factor for the rolling primary-leg
/// TTFT. Device-independent (a standard EMA alpha in `0..=1`), NOT a
/// routing threshold: higher = more weight on the latest sample.
pub const TTFT_EMA_ALPHA: f32 = 0.3;
