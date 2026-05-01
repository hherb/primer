//! Classifier types — output and input for the engagement classifier.
//!
//! `EngagementAssessment` is one classifier output. `EngagementContext`
//! is its input — newtype-wrapped so future fields (learner_age,
//! session_started_at, etc.) can be added without breaking the trait
//! signature.

use serde::{Deserialize, Serialize};

use crate::conversation::Turn;
use crate::learner::EngagementState;

/// One classifier output. State + confidence; `reasoning` is optional and
/// populated only by classifier impls that produce one (LLM impls can
/// generate it cheaply; BERT/stub do not).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngagementAssessment {
    pub state: EngagementState,
    /// Confidence in the assessment, in [0.0, 1.0].
    pub confidence: f32,
    /// Optional one-sentence rationale. Populated by LLM impls; None
    /// for stub and (future) BERT classifiers.
    pub reasoning: Option<String>,
}

impl EngagementAssessment {
    /// Construct a low-confidence Unknown assessment, used as the
    /// soft-fail return on classifier errors (parse failure, network
    /// error, invalid output).
    pub fn unknown_low_confidence(reason: impl Into<String>) -> Self {
        Self {
            state: EngagementState::Unknown,
            confidence: 0.0,
            reasoning: Some(reason.into()),
        }
    }
}

/// Input to the classifier. Newtype'd so future fields can be added
/// without breaking the trait signature.
pub struct EngagementContext<'a> {
    pub recent_child_turns: &'a [Turn],
    pub prior_assessments: &'a [EngagementAssessment],
    // Reserved for extension. Adding fields here is non-breaking
    // because the trait method takes EngagementContext by value.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_low_confidence_factory_returns_unknown_state() {
        let a = EngagementAssessment::unknown_low_confidence("parse failure");
        assert_eq!(a.state, EngagementState::Unknown);
        assert_eq!(a.confidence, 0.0);
        assert_eq!(a.reasoning.as_deref(), Some("parse failure"));
    }
}
