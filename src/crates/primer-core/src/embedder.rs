//! Dense-vector embedding backend trait.
//!
//! The [`Embedder`] turns text into fixed-dimension `f32` vectors so
//! [`KnowledgeBase`] and [`SessionStore`] retrieval can blend semantic
//! similarity (cosine over these vectors) with the existing BM25
//! lexical path via Reciprocal Rank Fusion.
//!
//! Like the inference and speech traits, `Embedder` lives in
//! `primer-core` so the storage / knowledge crates can depend on the
//! trait without a build-time dep on any concrete backend
//! (`primer-embedding`'s `StubEmbedder`, `FastEmbedBackend`, etc.).
//!
//! # Determinism
//!
//! Implementations are not required to be deterministic in general —
//! a real embedding model run twice on the same input will produce the
//! same vector by construction, but model implementations are
//! free to use non-deterministic batching, mixed precision, etc.
//! `StubEmbedder` (in `primer-embedding`) IS deterministic, so the
//! workspace's hermetic tests can pin retrieval behaviour without any
//! model download or runtime feature flag.
//!
//! # Model identity
//!
//! `model_id()` is persisted alongside every stored vector in both
//! `primer-knowledge` and `primer-storage`. On open, both crates check
//! that the configured embedder's `model_id` matches what's already in
//! the DB; mismatch is a hard error rather than a silent fallback,
//! because invisible quality regressions from cross-model mixing are
//! exactly the kind of trust-eroding bug we want to avoid.
//!
//! [`KnowledgeBase`]: crate::knowledge::KnowledgeBase
//! [`SessionStore`]: crate::storage::SessionStore

use async_trait::async_trait;

use crate::error::Result;
use crate::speech::Named;

/// Backend that produces dense `f32` vectors from text.
///
/// Implementations live in `primer-embedding`. This trait is the only
/// surface `primer-knowledge` and `primer-storage` see — they never
/// know whether the backend is the stub, fastembed, Ollama, or
/// something else.
#[async_trait]
pub trait Embedder: Named + Send + Sync {
    /// Embed a batch of texts. Returns one vector per input, each of
    /// `dim()` length.
    ///
    /// Implementations should batch under the hood for throughput;
    /// callers can pass either a single string slice or many at once.
    /// An empty input slice returns an empty output.
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;

    /// Vector dimensionality. Constant for the lifetime of the embedder
    /// (model is fixed at construction time).
    fn dim(&self) -> usize;

    /// Stable identifier for the underlying model, persisted in the
    /// per-DB `embedding_models` lookup table so cross-model mixing is
    /// detectable at open time. Must match across processes and across
    /// reopens of the same backend (e.g. `"bge-m3"`, `"stub-fxhash-v1"`).
    fn model_id(&self) -> &str;
}
