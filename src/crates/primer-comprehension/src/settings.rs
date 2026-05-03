//! Tunable settings for the comprehension subsystem.

use std::time::Duration;

use crate::consts;

#[derive(Debug, Clone)]
pub struct ComprehensionSettings {
    /// Maximum time to block awaiting the previous turn's
    /// extractor→comprehension chain before the next intent decision.
    /// Defaults to 5000ms — combined with `extractor.blocking_timeout`
    /// the chain has a 10s budget, comfortably covering even small-model
    /// two-call latency. On timeout, the task is detached so DB
    /// persistence still completes; we just don't wait for it.
    pub blocking_timeout: Duration,
    /// Hard cap on the LLM's raw output length before parsing.
    /// Char-boundary-safe.
    pub max_output_chars: usize,
    /// Hard cap on candidate concepts handed to one classifier call.
    /// Bounds prompt size; over-cap concepts are simply not assessed
    /// this turn.
    pub max_concepts_per_call: usize,
    /// Below this confidence, an assessment is persisted but NOT
    /// applied to in-memory `LearnerModel.concepts.depth`. Above it
    /// (>=), `apply_comprehension` promotes depth via monotonic max.
    pub confidence_threshold: f32,
    pub generation_max_tokens: u32,
    pub generation_temperature: f32,
    pub generation_top_p: f32,
    /// Per-assessment evidence-text cap (chars). Truncated
    /// char-boundary-safe at the parser layer.
    pub evidence_max_chars: usize,
}

impl Default for ComprehensionSettings {
    fn default() -> Self {
        Self {
            blocking_timeout: Duration::from_millis(consts::DEFAULT_BLOCKING_TIMEOUT_MS),
            max_output_chars: consts::DEFAULT_MAX_COMPREHENSION_OUTPUT_CHARS,
            max_concepts_per_call: consts::DEFAULT_MAX_CONCEPTS_PER_CALL,
            confidence_threshold: consts::DEFAULT_CONFIDENCE_THRESHOLD,
            generation_max_tokens: consts::DEFAULT_COMPREHENSION_MAX_TOKENS,
            generation_temperature: consts::DEFAULT_COMPREHENSION_TEMPERATURE,
            generation_top_p: consts::DEFAULT_COMPREHENSION_TOP_P,
            evidence_max_chars: consts::DEFAULT_EVIDENCE_MAX_CHARS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_consts() {
        let s = ComprehensionSettings::default();
        assert_eq!(
            s.blocking_timeout,
            Duration::from_millis(consts::DEFAULT_BLOCKING_TIMEOUT_MS)
        );
        assert_eq!(
            s.max_output_chars,
            consts::DEFAULT_MAX_COMPREHENSION_OUTPUT_CHARS
        );
        assert_eq!(
            s.max_concepts_per_call,
            consts::DEFAULT_MAX_CONCEPTS_PER_CALL
        );
        assert!((s.confidence_threshold - consts::DEFAULT_CONFIDENCE_THRESHOLD).abs() < 1e-6);
        assert_eq!(
            s.generation_max_tokens,
            consts::DEFAULT_COMPREHENSION_MAX_TOKENS
        );
        assert!(
            (s.generation_temperature - consts::DEFAULT_COMPREHENSION_TEMPERATURE).abs() < 1e-6
        );
        assert!((s.generation_top_p - consts::DEFAULT_COMPREHENSION_TOP_P).abs() < 1e-6);
        assert_eq!(s.evidence_max_chars, consts::DEFAULT_EVIDENCE_MAX_CHARS);
    }
}
