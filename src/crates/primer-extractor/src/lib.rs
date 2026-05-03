//! Concept extractor — trait + implementations.
//!
//! Extracts the concepts a child surfaced and the concepts the Primer
//! introduced, given one completed exchange. One LLM-backed
//! implementation (`LlmConceptExtractor`) wraps an
//! `Arc<dyn InferenceBackend>` so the crate stays free of
//! `primer-inference` as a build dependency. A stub implementation
//! supports tests.

use async_trait::async_trait;

use primer_core::error::Result;
use primer_core::extractor::{ConceptExtraction, ExtractionContext};

pub mod consts;
pub mod llm;
pub mod settings;
pub mod stub;

// Re-exports activated as Tasks 5–7 land:
// pub use llm::LlmConceptExtractor;
pub use settings::ExtractorSettings;
// pub use stub::StubConceptExtractor;

/// Extractor of concepts from one completed exchange.
///
/// Implementations may be LLM-backed (via `Arc<dyn InferenceBackend>`),
/// rule-based (future), or a stub for tests. Soft-fail policy:
/// extractor errors must NEVER propagate up and abort the conversation.
/// Implementations log via `tracing::warn!` and return
/// `ConceptExtraction::empty()` on any failure.
#[async_trait]
pub trait ConceptExtractor: Send + Sync {
    /// Stable identifier carrying impl + model-version. Future schema
    /// may persist this alongside extracted concepts so we can re-extract
    /// the archive with a different model later. Examples: "llm:claude-haiku-4-5",
    /// "stub", "rule-based-v1".
    fn identifier(&self) -> &str;

    async fn extract(&self, ctx: ExtractionContext<'_>) -> Result<ConceptExtraction>;
}
