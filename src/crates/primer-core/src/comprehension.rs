//! Types produced and consumed by the comprehension classifier.
//!
//! `ComprehensionAssessment` is one classifier output for one concept
//! on one turn. `ComprehensionResult` packages the per-concept
//! assessments. `ComprehensionContext` is the input — newtype-wrapped
//! so future fields can be added without breaking the trait signature.

use serde::{Deserialize, Serialize};

use crate::conversation::Turn;
use crate::learner::UnderstandingDepth;

/// One per-concept assessment of comprehension depth.
///
/// `evidence` is optional and populated only by classifier impls that
/// produce one (LLM impls can generate it cheaply; stub does not).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComprehensionAssessment {
    pub concept: String,
    pub depth: UnderstandingDepth,
    /// Confidence in the assessment, in [0.0, 1.0].
    pub confidence: f32,
    pub evidence: Option<String>,
}

/// One classifier output: zero-or-more per-concept assessments.
///
/// Empty when extraction yielded no concepts, when the LLM saw no
/// evidence for any candidate, or when the classifier soft-failed
/// (use `::empty()` for the soft-fail return).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComprehensionResult {
    pub assessments: Vec<ComprehensionAssessment>,
}

impl ComprehensionResult {
    /// Empty result. Used as the soft-fail return on classifier errors
    /// (parse failure, network error, invalid output). Never shaped as
    /// `Err` — failures must not propagate.
    pub fn empty() -> Self {
        Self::default()
    }
}

/// Input to the comprehension classifier. Newtype'd so future fields
/// can be added without breaking the trait signature.
pub struct ComprehensionContext<'a> {
    pub child_turn: &'a Turn,
    pub primer_turn: &'a Turn,
    /// A small slice of preceding turns for context (oldest-first).
    /// Helps the LLM disambiguate pronoun references across turns.
    pub recent_turns: &'a [Turn],
    /// Concepts to assess: the dedup'd union of `child_concepts ∪
    /// primer_concepts` from the just-completed exchange's extraction,
    /// already capped to `ComprehensionSettings::max_concepts_per_call`.
    pub candidate_concepts: &'a [String],
    // Reserved for extension. Adding fields here is non-breaking
    // because the trait method takes ComprehensionContext by value.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_factory_returns_empty_assessments() {
        let r = ComprehensionResult::empty();
        assert!(r.assessments.is_empty());
    }

    #[test]
    fn assessment_round_trips_through_serde() {
        let a = ComprehensionAssessment {
            concept: "photosynthesis".into(),
            depth: UnderstandingDepth::Comprehension,
            confidence: 0.85,
            evidence: Some("explained in own words".into()),
        };
        let json = serde_json::to_string(&a).unwrap();
        let back: ComprehensionAssessment = serde_json::from_str(&json).unwrap();
        assert_eq!(back.concept, a.concept);
        assert_eq!(back.depth, a.depth);
        assert!((back.confidence - a.confidence).abs() < 1e-6);
        assert_eq!(back.evidence, a.evidence);
    }

    #[test]
    fn result_round_trips_through_serde() {
        let r = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "gravity".into(),
                depth: UnderstandingDepth::Recall,
                confidence: 0.7,
                evidence: None,
            }],
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: ComprehensionResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.assessments.len(), 1);
        assert_eq!(back.assessments[0].concept, "gravity");
    }
}
