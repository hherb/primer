//! Vocabulary spaced-repetition settings consumed by `DialogueManager`.
//!
//! The pure scheduling functions live in [`primer_core::vocab`]; this
//! module only owns the runtime tunables and re-exports the default cap
//! constant for downstream callers (CLI, integration tests).

use primer_core::consts::vocab::DEFAULT_VOCAB_MAX_PER_PROMPT;

/// Tunables for the vocabulary review feature.
///
/// `max_per_prompt` caps how many overdue concepts are injected into
/// the system prompt per turn. Higher = more review pressure, more
/// prompt bloat. Configurable via the `--vocab-max-per-prompt` CLI flag;
/// default is `DEFAULT_VOCAB_MAX_PER_PROMPT` (4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VocabSettings {
    pub max_per_prompt: usize,
}

impl Default for VocabSettings {
    fn default() -> Self {
        Self {
            max_per_prompt: DEFAULT_VOCAB_MAX_PER_PROMPT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_max_per_prompt_is_four() {
        assert_eq!(VocabSettings::default().max_per_prompt, 4);
    }

    #[test]
    fn vocab_settings_is_copy_for_cheap_threading() {
        let a = VocabSettings::default();
        let b = a;
        assert_eq!(a, b);
    }
}
