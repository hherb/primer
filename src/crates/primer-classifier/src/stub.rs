//! Stub classifier — deterministic, test-controllable.

use std::collections::VecDeque;

use async_trait::async_trait;
use tokio::sync::Mutex;

use primer_core::classifier::{EngagementAssessment, EngagementContext};
use primer_core::error::Result;
use primer_core::learner::EngagementState;

use crate::EngagementClassifier;

pub struct StubEngagementClassifier {
    fallback: EngagementAssessment,
    script: Mutex<Option<VecDeque<EngagementAssessment>>>,
}

impl StubEngagementClassifier {
    pub fn new() -> Self {
        Self {
            fallback: EngagementAssessment {
                state: EngagementState::Engaged,
                confidence: 1.0,
                reasoning: None,
            },
            script: Mutex::new(None),
        }
    }

    pub fn with_response(response: EngagementAssessment) -> Self {
        Self { fallback: response, script: Mutex::new(None) }
    }

    pub fn with_script(script: Vec<EngagementAssessment>) -> Self {
        Self {
            fallback: EngagementAssessment {
                state: EngagementState::Engaged,
                confidence: 1.0,
                reasoning: None,
            },
            script: Mutex::new(Some(script.into())),
        }
    }
}

impl Default for StubEngagementClassifier {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl EngagementClassifier for StubEngagementClassifier {
    fn identifier(&self) -> &str { "stub" }

    async fn classify(&self, _ctx: EngagementContext<'_>) -> Result<EngagementAssessment> {
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

    fn ctx<'a>() -> EngagementContext<'a> {
        EngagementContext { recent_child_turns: &[], prior_assessments: &[] }
    }

    #[tokio::test]
    async fn default_returns_engaged_with_full_confidence() {
        let c = StubEngagementClassifier::new();
        let a = c.classify(ctx()).await.unwrap();
        assert_eq!(a.state, EngagementState::Engaged);
        assert!((a.confidence - 1.0).abs() < 1e-6);
        assert!(a.reasoning.is_none());
    }

    #[tokio::test]
    async fn with_response_returns_configured() {
        let configured = EngagementAssessment {
            state: EngagementState::FrustratedTrying,
            confidence: 0.7,
            reasoning: Some("test".into()),
        };
        let c = StubEngagementClassifier::with_response(configured.clone());
        let a = c.classify(ctx()).await.unwrap();
        assert_eq!(a.state, configured.state);
    }

    #[tokio::test]
    async fn with_script_returns_in_order_then_falls_back() {
        let c = StubEngagementClassifier::with_script(vec![
            EngagementAssessment {
                state: EngagementState::Reflecting, confidence: 0.5, reasoning: None,
            },
            EngagementAssessment {
                state: EngagementState::FrustratedStuck, confidence: 0.9, reasoning: None,
            },
        ]);
        let a1 = c.classify(ctx()).await.unwrap();
        assert_eq!(a1.state, EngagementState::Reflecting);
        let a2 = c.classify(ctx()).await.unwrap();
        assert_eq!(a2.state, EngagementState::FrustratedStuck);
        // Exhausted — falls back to default Engaged
        let a3 = c.classify(ctx()).await.unwrap();
        assert_eq!(a3.state, EngagementState::Engaged);
    }

    #[tokio::test]
    async fn identifier_is_stub() {
        let c = StubEngagementClassifier::new();
        assert_eq!(c.identifier(), "stub");
    }
}
