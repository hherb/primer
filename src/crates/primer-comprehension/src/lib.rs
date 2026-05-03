//! Comprehension classifier — trait + implementations.
//!
//! Assesses the depth of understanding a child has demonstrated for
//! each concept they engaged with in one completed exchange.
//! `LlmComprehensionClassifier` wraps an `Arc<dyn InferenceBackend>`
//! so the crate stays free of `primer-inference` as a build
//! dependency. A stub implementation supports tests.
//!
//! Soft-fail policy: classifier errors must NEVER propagate up and
//! abort the conversation. Implementations log via `tracing::warn!`
//! and return `ComprehensionResult::empty()` on any failure.

use async_trait::async_trait;

use primer_core::comprehension::{ComprehensionContext, ComprehensionResult};
use primer_core::error::Result;

pub mod consts;
pub mod llm;
pub mod settings;
pub mod stub;

pub use llm::LlmComprehensionClassifier;
pub use settings::ComprehensionSettings;
pub use stub::StubComprehensionClassifier;

/// Per-concept comprehension classifier.
///
/// Implementations may be LLM-backed (via `Arc<dyn InferenceBackend>`),
/// rule-based (future), or a stub for tests. Soft-fail policy applies
/// — never propagate errors.
#[async_trait]
pub trait ComprehensionClassifier: Send + Sync {
    /// Stable identifier carrying impl + model. Persisted in the
    /// `comprehension_classifiers` lookup table; lets us re-classify
    /// the archive with a different classifier later without
    /// overwriting old labels. Examples: `"llm:cloud-anthropic:claude-haiku-4-5"`,
    /// `"stub"`.
    fn identifier(&self) -> &str;

    async fn classify(&self, ctx: ComprehensionContext<'_>) -> Result<ComprehensionResult>;
}
