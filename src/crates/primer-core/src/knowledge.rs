//! Knowledge base trait — RAG retrieval from the offline corpus.
//!
//! The knowledge base stores factual content (Wikipedia, curated encyclopedias,
//! curriculum materials) and retrieves relevant passages given a query.
//! This grounds the LLM's responses in verified information rather than
//! relying solely on parametric knowledge.
//!
//! Implementation uses SQLite FTS5 for full-text search, optionally augmented
//! with embedding-based semantic search (via candle or a small local model).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// A passage retrieved from the knowledge base.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Passage {
    /// Unique identifier for this passage.
    pub id: String,
    /// Source identifier (e.g., "wikipedia:Water", "encyclopedia:photosynthesis").
    /// Logical foreign key into `SourceMeta::id` when the source has been registered
    /// via `KnowledgeBase::upsert_source`.
    pub source: String,
    /// The text content of the passage.
    pub text: String,
    /// Relevance score (higher = more relevant). Scale depends on retrieval method.
    pub score: f64,
}

/// Per-source attribution + licence metadata. Stored once per distinct source
/// in the KB DB so licence compliance can travel with the data: a child or
/// parent can ask "where did this come from?" and the answer is local.
///
/// `id` matches `Passage.source`. `license` is a free-form string (e.g.
/// `"CC-BY-SA-4.0"`, `"CC0-1.0"`, `"Public Domain"`) — not an enum, so a
/// new licence variant doesn't require a code change. The accept/reject
/// policy lives in the ingestion pipeline, not in this type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceMeta {
    /// Stable source identifier referenced by `Passage.source`.
    pub id: String,
    /// Licence tag, e.g. `"CC-BY-SA-4.0"`.
    pub license: String,
    /// Human-readable credit line shown to users / printed by `--print-attribution`.
    pub attribution: String,
    /// Canonical URL, if applicable. `None` for hand-authored seed content.
    pub source_url: Option<String>,
    /// When this source was last fetched / written, as Unix epoch seconds.
    pub retrieved_at: i64,
}

/// Parameters for knowledge retrieval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalParams {
    /// Maximum number of passages to return.
    pub top_k: usize,
    /// Minimum relevance score threshold (passages below this are discarded).
    pub min_score: f64,
    /// Restrict to specific sources (empty = search all).
    pub source_filter: Vec<String>,
}

impl Default for RetrievalParams {
    fn default() -> Self {
        Self {
            top_k: 5,
            min_score: 0.0,
            source_filter: vec![],
        }
    }
}

/// The knowledge base backend.
#[async_trait]
pub trait KnowledgeBase: Send + Sync {
    /// Retrieve relevant passages for a query string.
    async fn retrieve(&self, query: &str, params: &RetrievalParams) -> Result<Vec<Passage>>;

    /// Retrieve passages relevant to the current conversation context.
    /// Default implementation extracts key terms from the last few messages
    /// and calls `retrieve()`. Implementations may override with more
    /// sophisticated context-aware retrieval.
    async fn retrieve_for_context(
        &self,
        conversation_summary: &str,
        params: &RetrievalParams,
    ) -> Result<Vec<Passage>> {
        self.retrieve(conversation_summary, params).await
    }

    /// Insert or update a source's attribution + licence metadata.
    /// Called by the ingestion path before inserting passages that reference it.
    /// Default impl is a no-op so backends that don't track sources stay valid.
    async fn upsert_source(&self, _source: &SourceMeta) -> Result<()> {
        Ok(())
    }

    /// List every registered source, for `--print-attribution`-style audit.
    /// Default impl returns an empty vec.
    async fn list_sources(&self) -> Result<Vec<SourceMeta>> {
        Ok(vec![])
    }
}
