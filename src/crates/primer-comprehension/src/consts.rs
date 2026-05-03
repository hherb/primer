//! Default values for `ComprehensionSettings`. Per the no-magic-numbers
//! convention, every numeric used by the comprehension subsystem is
//! defined here (or in a sibling settings struct field).

pub const DEFAULT_BLOCKING_TIMEOUT_MS: u64 = 1500;

/// Hard cap on the LLM's raw output length before parsing. Sized for
/// comprehension's per-concept JSON output: ~10 entries × ~60 chars
/// structured ≈ ~600 chars before envelope, plus evidence text up to
/// evidence_max_chars per assessment. Currently equal to the extractor's
/// cap (4096); independent constants kept so the two can diverge if
/// future tuning calls for it.
pub const DEFAULT_MAX_COMPREHENSION_OUTPUT_CHARS: usize = 4096;

/// Hard cap on candidate concepts handed to one classifier call.
/// Bounds prompt size on pathological turns; concepts beyond the cap
/// are simply not assessed this turn (they retain their existing
/// learner.concepts state and may be assessed in a future turn).
pub const DEFAULT_MAX_CONCEPTS_PER_CALL: usize = 10;

/// Confidence threshold below which an assessment is persisted to
/// `turn_comprehensions` (raw signal preserved) but NOT applied to
/// in-memory `LearnerModel.concepts.depth`. Same value the engagement
/// classifier uses, intentionally — keeps two LLM-confidence-gated
/// classifiers from getting out of step.
pub const DEFAULT_CONFIDENCE_THRESHOLD: f32 = 0.6;

pub const DEFAULT_COMPREHENSION_MAX_TOKENS: u32 = 512;
pub const DEFAULT_COMPREHENSION_TEMPERATURE: f32 = 0.1;
pub const DEFAULT_COMPREHENSION_TOP_P: f32 = 0.9;

/// Per-assessment evidence-text cap. Truncated char-boundary-safe at
/// the parser layer so a verbose LLM doesn't produce 1KB evidence
/// blobs that bloat `turn_comprehensions.evidence`.
pub const DEFAULT_EVIDENCE_MAX_CHARS: usize = 240;

/// Char cap on the snippet of unparseable LLM output included in the
/// `tracing::warn!` log line. Tracing concern only — not user-tunable.
pub const LLM_DEBUG_SNIPPET_CHARS: usize = 256;
