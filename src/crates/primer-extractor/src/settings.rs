//! Tunable settings for the extractor subsystem.

use std::time::Duration;

use crate::consts;

#[derive(Debug, Clone)]
pub struct ExtractorSettings {
    /// Maximum time to block awaiting the previous turn's extraction
    /// before the next intent decision. Defaults to 1500ms — extraction
    /// is allowed to be slower than classification because no in-turn
    /// decision currently depends on extracted concepts. If the timeout
    /// fires, the task is detached (extraction may still complete and
    /// persist asynchronously); we just don't wait for it.
    pub blocking_timeout: Duration,
    /// How many surrounding turns to include as context in the
    /// extractor prompt. Helps disambiguate pronouns ("it", "that").
    pub recent_context_turns: usize,
    /// Hard cap on the LLM's raw output length before parsing, to
    /// guard against runaway responses. Char-boundary-safe.
    pub max_output_chars: usize,
    /// Hard cap on extracted concepts per speaker. Truncated post-parse
    /// so a runaway list doesn't bloat `concepts` table cardinality.
    pub max_concepts_per_speaker: usize,
    pub generation_max_tokens: u32,
    pub generation_temperature: f32,
    pub generation_top_p: f32,
}

impl Default for ExtractorSettings {
    fn default() -> Self {
        Self {
            blocking_timeout: Duration::from_millis(consts::DEFAULT_BLOCKING_TIMEOUT_MS),
            recent_context_turns: consts::DEFAULT_RECENT_CONTEXT_TURNS,
            max_output_chars: consts::DEFAULT_MAX_EXTRACTOR_OUTPUT_CHARS,
            max_concepts_per_speaker: consts::DEFAULT_MAX_CONCEPTS_PER_SPEAKER,
            generation_max_tokens: consts::DEFAULT_EXTRACTOR_MAX_TOKENS,
            generation_temperature: consts::DEFAULT_EXTRACTOR_TEMPERATURE,
            generation_top_p: consts::DEFAULT_EXTRACTOR_TOP_P,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_consts() {
        let s = ExtractorSettings::default();
        assert_eq!(
            s.blocking_timeout,
            Duration::from_millis(consts::DEFAULT_BLOCKING_TIMEOUT_MS)
        );
        assert_eq!(s.recent_context_turns, consts::DEFAULT_RECENT_CONTEXT_TURNS);
        assert_eq!(
            s.max_output_chars,
            consts::DEFAULT_MAX_EXTRACTOR_OUTPUT_CHARS
        );
        assert_eq!(
            s.max_concepts_per_speaker,
            consts::DEFAULT_MAX_CONCEPTS_PER_SPEAKER
        );
        assert_eq!(
            s.generation_max_tokens,
            consts::DEFAULT_EXTRACTOR_MAX_TOKENS
        );
        assert!((s.generation_temperature - consts::DEFAULT_EXTRACTOR_TEMPERATURE).abs() < 1e-6);
        assert!((s.generation_top_p - consts::DEFAULT_EXTRACTOR_TOP_P).abs() < 1e-6);
    }
}
