//! Stub comprehension classifier — deterministic, test-controllable.
//!
//! Mirrors the shape of `primer_classifier::StubEngagementClassifier`
//! and `primer_extractor::StubConceptExtractor`.

use std::collections::VecDeque;

use async_trait::async_trait;
use tokio::sync::Mutex;

use primer_core::comprehension::{ComprehensionContext, ComprehensionResult};
use primer_core::error::Result;

use crate::ComprehensionClassifier;

pub struct StubComprehensionClassifier {
    fallback: ComprehensionResult,
    script: Mutex<Option<VecDeque<ComprehensionResult>>>,
}

impl StubComprehensionClassifier {
    pub fn new() -> Self {
        Self {
            fallback: ComprehensionResult::empty(),
            script: Mutex::new(None),
        }
    }

    pub fn with_response(response: ComprehensionResult) -> Self {
        Self {
            fallback: response,
            script: Mutex::new(None),
        }
    }

    pub fn with_script(script: Vec<ComprehensionResult>) -> Self {
        Self {
            fallback: ComprehensionResult::empty(),
            script: Mutex::new(Some(script.into())),
        }
    }
}

impl Default for StubComprehensionClassifier {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ComprehensionClassifier for StubComprehensionClassifier {
    fn identifier(&self) -> &str {
        "stub"
    }

    async fn classify(&self, _ctx: ComprehensionContext<'_>) -> Result<ComprehensionResult> {
        let mut script = self.script.lock().await;
        if let Some(q) = script.as_mut() {
            if let Some(next) = q.pop_front() {
                return Ok(next);
            }
        }
        Ok(self.fallback.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use primer_core::comprehension::ComprehensionAssessment;
    use primer_core::conversation::{Speaker, Turn};
    use primer_core::learner::UnderstandingDepth;

    fn t(speaker: Speaker, text: &str) -> Turn {
        Turn {
            speaker,
            text: text.into(),
            timestamp: Utc::now(),
            intent: None,
            concepts: vec![],
        }
    }

    fn ctx<'a>(
        child: &'a Turn,
        primer: &'a Turn,
        candidates: &'a [String],
    ) -> ComprehensionContext<'a> {
        ComprehensionContext {
            child_turn: child,
            primer_turn: primer,
            recent_turns: &[],
            candidate_concepts: candidates,
        }
    }

    #[tokio::test]
    async fn default_returns_empty() {
        let c = StubComprehensionClassifier::new();
        let child = t(Speaker::Child, "hi");
        let primer = t(Speaker::Primer, "hello");
        let candidates: Vec<String> = vec![];
        let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert!(r.assessments.is_empty());
    }

    #[tokio::test]
    async fn with_response_returns_configured() {
        let configured = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "gravity".into(),
                depth: UnderstandingDepth::Recall,
                confidence: 0.7,
                evidence: None,
            }],
        };
        let c = StubComprehensionClassifier::with_response(configured.clone());
        let child = t(Speaker::Child, "?");
        let primer = t(Speaker::Primer, "?");
        let candidates: Vec<String> = vec!["gravity".into()];
        let r = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert_eq!(r.assessments.len(), 1);
        assert_eq!(r.assessments[0].concept, "gravity");
    }

    #[tokio::test]
    async fn with_script_returns_in_order_then_falls_back() {
        let mk = |concept: &str| ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: concept.into(),
                depth: UnderstandingDepth::Aware,
                confidence: 0.8,
                evidence: None,
            }],
        };
        let c = StubComprehensionClassifier::with_script(vec![mk("a"), mk("b")]);
        let child = t(Speaker::Child, "?");
        let primer = t(Speaker::Primer, "?");
        let candidates: Vec<String> = vec![];
        let r1 = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert_eq!(r1.assessments[0].concept, "a");
        let r2 = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert_eq!(r2.assessments[0].concept, "b");
        let r3 = c.classify(ctx(&child, &primer, &candidates)).await.unwrap();
        assert!(r3.assessments.is_empty()); // falls back
    }

    #[tokio::test]
    async fn identifier_is_stub() {
        let c = StubComprehensionClassifier::new();
        assert_eq!(c.identifier(), "stub");
    }
}
