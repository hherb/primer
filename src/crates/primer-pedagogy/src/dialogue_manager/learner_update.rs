//! Per-turn learner-model update from the child's response.
//!
//! Currently a simple word-count heuristic for engagement state, kept as
//! a placeholder while the LLM-based engagement classifier (in
//! `primer-classifier`, dispatched via the spawn-and-await pattern in
//! `background.rs`) provides the real signal. The classifier writes
//! into `learner.recent_assessments` and conditionally
//! `learner.current_engagement` via `apply_assessment`; this heuristic
//! is the best-effort fallback for the very first turn (before any
//! classifier output exists) and for backends where the classifier is
//! disabled.

use primer_core::conversation::PedagogicalIntent;
use primer_core::learner::EngagementState;

use super::DialogueManager;

impl DialogueManager {
    /// Update the learner model based on the conversation evidence.
    ///
    /// This is deliberately minimal for the scaffold. A production version
    /// would:
    /// - Parse the child's response for comprehension signals
    /// - Use the LLM to classify understanding depth
    /// - Update concept graph confidence scores
    /// - Detect engagement state from response patterns
    pub(super) fn update_learner_model(&mut self, child_input: &str, _intent: &PedagogicalIntent) {
        // Simple engagement heuristic: very short responses may indicate
        // frustration or disengagement.
        let word_count = child_input.split_whitespace().count();

        let new_engagement = if word_count == 0 {
            EngagementState::Disengaging
        } else if word_count < 3 {
            // Could be frustration ("I don't know") or just a short answer.
            // Don't over-interpret — keep previous state unless it was Engaged.
            match self.learner.current_engagement {
                EngagementState::Engaged => EngagementState::Reflecting,
                other => other,
            }
        } else {
            EngagementState::Engaged
        };
        // Only mark dirty if the persisted field actually changed —
        // assigning the same value back is a no-op for the on-disk row,
        // and the per-turn save site uses the dirty flag to decide whether
        // to issue a write transaction.
        if self.learner.current_engagement != new_engagement {
            self.learner.current_engagement = new_engagement;
            self.learner_dirty = true;
        }
    }
}
