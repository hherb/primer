//! Tunable settings for the classifier subsystem.

use std::time::Duration;

use crate::consts;

#[derive(Debug, Clone)]
pub struct ClassifierSettings {
    pub history_depth: usize,
    pub blocking_timeout: Duration,
    pub confidence_threshold: f32,
    pub recent_child_turns: usize,
    pub max_output_chars: usize,
}

impl Default for ClassifierSettings {
    fn default() -> Self {
        Self {
            history_depth: consts::DEFAULT_HISTORY_DEPTH,
            blocking_timeout: Duration::from_millis(consts::DEFAULT_BLOCKING_TIMEOUT_MS),
            confidence_threshold: consts::DEFAULT_CONFIDENCE_THRESHOLD,
            recent_child_turns: consts::DEFAULT_RECENT_CHILD_TURNS_FOR_CLASSIFICATION,
            max_output_chars: consts::DEFAULT_MAX_CLASSIFIER_OUTPUT_CHARS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_consts() {
        let s = ClassifierSettings::default();
        assert_eq!(s.history_depth, consts::DEFAULT_HISTORY_DEPTH);
        assert_eq!(
            s.blocking_timeout,
            Duration::from_millis(consts::DEFAULT_BLOCKING_TIMEOUT_MS)
        );
        assert!((s.confidence_threshold - consts::DEFAULT_CONFIDENCE_THRESHOLD).abs() < 1e-6);
        assert_eq!(
            s.recent_child_turns,
            consts::DEFAULT_RECENT_CHILD_TURNS_FOR_CLASSIFICATION
        );
        assert_eq!(s.max_output_chars, consts::DEFAULT_MAX_CLASSIFIER_OUTPUT_CHARS);
    }
}
