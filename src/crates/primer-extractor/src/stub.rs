//! Stub extractor — deterministic, test-controllable.

use std::collections::VecDeque;

use async_trait::async_trait;
use tokio::sync::Mutex;

use primer_core::error::Result;
use primer_core::extractor::{ConceptExtraction, ExtractionContext};

use crate::ConceptExtractor;

pub struct StubConceptExtractor {
    fallback: ConceptExtraction,
    script: Mutex<Option<VecDeque<ConceptExtraction>>>,
}

impl StubConceptExtractor {
    pub fn new() -> Self {
        Self {
            fallback: ConceptExtraction::empty(),
            script: Mutex::new(None),
        }
    }

    pub fn with_response(response: ConceptExtraction) -> Self {
        Self {
            fallback: response,
            script: Mutex::new(None),
        }
    }

    pub fn with_script(script: Vec<ConceptExtraction>) -> Self {
        Self {
            fallback: ConceptExtraction::empty(),
            script: Mutex::new(Some(script.into())),
        }
    }
}

impl Default for StubConceptExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ConceptExtractor for StubConceptExtractor {
    fn identifier(&self) -> &str {
        "stub"
    }

    async fn extract(&self, _ctx: ExtractionContext<'_>) -> Result<ConceptExtraction> {
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
    use primer_core::conversation::{Speaker, Turn};

    fn t(speaker: Speaker, text: &str) -> Turn {
        Turn {
            speaker,
            text: text.into(),
            timestamp: Utc::now(),
            intent: None,
            concepts: vec![],
        }
    }

    fn ctx<'a>(child: &'a Turn, primer: &'a Turn) -> ExtractionContext<'a> {
        ExtractionContext {
            child_turn: child,
            primer_turn: primer,
            recent_turns: &[],
        }
    }

    #[tokio::test]
    async fn default_returns_empty() {
        let e = StubConceptExtractor::new();
        let child = t(Speaker::Child, "hi");
        let primer = t(Speaker::Primer, "hello");
        let r = e.extract(ctx(&child, &primer)).await.unwrap();
        assert!(r.child_concepts.is_empty());
        assert!(r.primer_concepts.is_empty());
    }

    #[tokio::test]
    async fn with_response_returns_configured() {
        let configured = ConceptExtraction {
            child_concepts: vec!["gravity".into()],
            primer_concepts: vec!["mass".into()],
        };
        let e = StubConceptExtractor::with_response(configured.clone());
        let child = t(Speaker::Child, "?");
        let primer = t(Speaker::Primer, "?");
        let r = e.extract(ctx(&child, &primer)).await.unwrap();
        assert_eq!(r.child_concepts, configured.child_concepts);
        assert_eq!(r.primer_concepts, configured.primer_concepts);
    }

    #[tokio::test]
    async fn with_script_returns_in_order_then_falls_back() {
        let e = StubConceptExtractor::with_script(vec![
            ConceptExtraction {
                child_concepts: vec!["a".into()],
                primer_concepts: vec![],
            },
            ConceptExtraction {
                child_concepts: vec!["b".into()],
                primer_concepts: vec![],
            },
        ]);
        let child = t(Speaker::Child, "?");
        let primer = t(Speaker::Primer, "?");
        let r1 = e.extract(ctx(&child, &primer)).await.unwrap();
        assert_eq!(r1.child_concepts, vec!["a".to_string()]);
        let r2 = e.extract(ctx(&child, &primer)).await.unwrap();
        assert_eq!(r2.child_concepts, vec!["b".to_string()]);
        // Script exhausted — falls back to empty.
        let r3 = e.extract(ctx(&child, &primer)).await.unwrap();
        assert!(r3.child_concepts.is_empty());
    }

    #[tokio::test]
    async fn identifier_is_stub() {
        let e = StubConceptExtractor::new();
        assert_eq!(e.identifier(), "stub");
    }
}
