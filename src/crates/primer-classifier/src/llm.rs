//! LLM-backed engagement classifier. Wraps any `InferenceBackend`
//! (via Arc<dyn ...>) so memory-constrained devices can reuse the chat
//! backend, while bigger machines can use a separate small model.

use std::sync::Arc;

use async_trait::async_trait;

use primer_core::classifier::{EngagementAssessment, EngagementContext};
use primer_core::error::Result;
use primer_core::inference::InferenceBackend;

use crate::settings::ClassifierSettings;
use crate::EngagementClassifier;

pub struct LlmEngagementClassifier {
    #[allow(dead_code)] // used by classify() in Task 14
    backend: Arc<dyn InferenceBackend>,
    #[allow(dead_code)]
    model: String,
    #[allow(dead_code)]
    settings: ClassifierSettings,
    identifier: String,
}

impl LlmEngagementClassifier {
    pub fn new(
        backend: Arc<dyn InferenceBackend>,
        model: String,
        settings: ClassifierSettings,
    ) -> Self {
        let identifier = format!("llm:{model}");
        Self { backend, model, settings, identifier }
    }
}

#[async_trait]
impl EngagementClassifier for LlmEngagementClassifier {
    fn identifier(&self) -> &str { &self.identifier }

    async fn classify(&self, _ctx: EngagementContext<'_>) -> Result<EngagementAssessment> {
        // Real implementation in Task 14.
        Ok(EngagementAssessment::unknown_low_confidence("not yet implemented"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use primer_core::inference::{GenerationParams, Prompt, TokenChunk, TokenStream};

    /// Test backend that returns canned text from `generate_stream` and stubs
    /// `name` / `is_available`. `generate` and `summarize` use the default
    /// impls that delegate to `generate_stream`.
    struct CannedBackend(String);

    #[async_trait]
    impl InferenceBackend for CannedBackend {
        fn name(&self) -> &str { "canned" }

        async fn is_available(&self) -> bool { true }

        async fn generate_stream(
            &self,
            _prompt: &Prompt,
            _params: &GenerationParams,
        ) -> Result<TokenStream> {
            let text = self.0.clone();
            let chunk = TokenChunk { text, done: true };
            Ok(Box::pin(stream::once(async move { Ok(chunk) })))
        }
    }

    #[test]
    fn identifier_includes_model_name() {
        let backend = Arc::new(CannedBackend("{}".into())) as Arc<dyn InferenceBackend>;
        let c = LlmEngagementClassifier::new(
            backend,
            "claude-haiku-4-5".into(),
            ClassifierSettings::default(),
        );
        assert_eq!(c.identifier(), "llm:claude-haiku-4-5");
    }
}
