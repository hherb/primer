//! Engagement classifier — trait + implementations.
//!
//! Classifies a child's engagement state given recent turns and the
//! prior assessment trajectory. One LLM-backed implementation
//! (`LlmEngagementClassifier`) wraps an `Arc<dyn InferenceBackend>` so
//! the crate stays free of `primer-inference` as a build dependency.
//! A stub implementation supports tests.

use async_trait::async_trait;

use primer_core::classifier::{EngagementAssessment, EngagementContext};
use primer_core::error::Result;

pub mod consts;
pub mod settings;
pub mod stub;
pub mod llm;

pub use settings::ClassifierSettings;
pub use stub::StubEngagementClassifier;

/// Classifier of the child's engagement state.
///
/// Implementations may be LLM-backed (via `Arc<dyn InferenceBackend>`),
/// a fine-tuned local model, or a stub for tests. Soft-fail policy:
/// classifier errors must NEVER propagate up and abort the conversation.
/// Implementations log via `tracing::warn!` and return
/// `EngagementAssessment::unknown_low_confidence(...)` on any failure.
#[async_trait]
pub trait EngagementClassifier: Send + Sync {
    /// Stable identifier carrying impl + model-version. Stored in the
    /// classifiers lookup table; lets us re-classify the archive with a
    /// different classifier later without overwriting old labels.
    /// Examples: "llm:claude-haiku-4-5", "stub", "bert-engagement-v1".
    fn identifier(&self) -> &str;

    async fn classify(&self, ctx: EngagementContext<'_>) -> Result<EngagementAssessment>;
}
