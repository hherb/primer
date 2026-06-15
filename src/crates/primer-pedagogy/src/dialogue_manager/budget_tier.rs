//! The progressive-shrink prompt-budget ladder for context-limit recovery.
//!
//! Each tier selects which optional prompt sections are assembled. The
//! ladder is a pure, backend-agnostic state machine so the dialogue
//! manager's retry loop is unit-testable without an inference backend.

/// Which optional prompt sections to include on a given attempt.
// Consumed by the retry loop wired in Task 7; dead_code until then.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptBudgetTier {
    /// Today's behaviour: KB passages + long-term memory + full window.
    Full,
    /// First retry: drop KB passages, keep long-term memory.
    NoKnowledge,
    /// Second retry: drop KB + long-term memory, shrink the recent-turn
    /// window to a floor.
    Minimal,
}

// Methods consumed by the retry loop wired in Task 7; dead_code until then.
#[allow(dead_code)]
impl PromptBudgetTier {
    /// The full ladder, tightest-last. Length − 1 == number of retries.
    pub(crate) const LADDER: [PromptBudgetTier; 3] = [Self::Full, Self::NoKnowledge, Self::Minimal];

    /// The next tighter tier, or `None` if already at the tightest.
    pub(crate) fn next_tighter(self) -> Option<Self> {
        match self {
            Self::Full => Some(Self::NoKnowledge),
            Self::NoKnowledge => Some(Self::Minimal),
            Self::Minimal => None,
        }
    }

    /// Whether KB passages are retrieved at this tier.
    pub(crate) fn includes_knowledge(self) -> bool {
        matches!(self, Self::Full)
    }

    /// Whether long-term memory (summary + retrieved older turns) is
    /// retrieved at this tier.
    pub(crate) fn includes_long_term_memory(self) -> bool {
        matches!(self, Self::Full | Self::NoKnowledge)
    }

    /// Effective recent-turn window: `base` for Full/NoKnowledge; capped
    /// at the Minimal floor for Minimal.
    pub(crate) fn context_window_turns(self, base: usize) -> usize {
        match self {
            Self::Full | Self::NoKnowledge => base,
            Self::Minimal => base.min(crate::consts::MINIMAL_TIER_CONTEXT_WINDOW_TURNS),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_length_matches_retry_budget() {
        assert_eq!(
            PromptBudgetTier::LADDER.len(),
            crate::consts::MAX_TRUNCATION_RETRIES + 1
        );
    }

    #[test]
    fn next_tighter_walks_the_ladder_then_stops() {
        assert_eq!(
            PromptBudgetTier::Full.next_tighter(),
            Some(PromptBudgetTier::NoKnowledge)
        );
        assert_eq!(
            PromptBudgetTier::NoKnowledge.next_tighter(),
            Some(PromptBudgetTier::Minimal)
        );
        assert_eq!(PromptBudgetTier::Minimal.next_tighter(), None);
    }

    #[test]
    fn full_includes_everything_minimal_includes_nothing_optional() {
        assert!(PromptBudgetTier::Full.includes_knowledge());
        assert!(PromptBudgetTier::Full.includes_long_term_memory());
        assert!(!PromptBudgetTier::NoKnowledge.includes_knowledge());
        assert!(PromptBudgetTier::NoKnowledge.includes_long_term_memory());
        assert!(!PromptBudgetTier::Minimal.includes_knowledge());
        assert!(!PromptBudgetTier::Minimal.includes_long_term_memory());
    }

    #[test]
    fn minimal_caps_window_at_floor_others_pass_through() {
        let base = 12;
        assert_eq!(PromptBudgetTier::Full.context_window_turns(base), base);
        assert_eq!(
            PromptBudgetTier::NoKnowledge.context_window_turns(base),
            base
        );
        assert_eq!(
            PromptBudgetTier::Minimal.context_window_turns(base),
            crate::consts::MINIMAL_TIER_CONTEXT_WINDOW_TURNS
        );
    }
}
