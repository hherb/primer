//! # primer-embedding
//!
//! Concrete [`Embedder`] backends for the Primer's hybrid retrieval.
//!
//! This first cut ships only [`StubEmbedder`]. Real-model backends —
//! `FastEmbedBackend` (BGE-M3 via fastembed-rs) and `OllamaEmbedder`
//! (`/api/embeddings`) — land in later steps of the hybrid-retrieval
//! design and gate behind the `fastembed` and `ollama` feature flags.
//! Keeping the trait + stub on default features means every workspace
//! test (and CI) runs against a deterministic, in-process embedder
//! with zero network or model-download dependency.
//!
//! [`Embedder`]: primer_core::embedder::Embedder

pub mod stub;

pub use stub::StubEmbedder;
