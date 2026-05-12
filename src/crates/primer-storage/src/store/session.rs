//! `SessionStore` trait impl for `SqliteSessionStore`.
//!
//! Every method body is a one-line delegation to the matching
//! `*_inner` inherent method on `SqliteSessionStore`. Those methods
//! live in `session_save`, `session_load`, and `session_search` —
//! see those files for the actual SQL and transactional logic.
//! Keeping the trait surface small here makes the dispatch table
//! easy to scan and audit.

use async_trait::async_trait;
use primer_core::classifier::EngagementAssessment;
use primer_core::comprehension::ComprehensionAssessment;
use primer_core::conversation::{Session, SessionId, SessionListing, Turn};
use primer_core::embedder::Embedder;
use primer_core::error::Result;
use primer_core::knowledge::HybridParams;
use primer_core::storage::SessionStore;
use uuid::Uuid;

use super::SqliteSessionStore;

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn save_session(&self, session: &Session) -> Result<()> {
        self.save_session_inner(session).await
    }

    async fn load_session(&self, id: Uuid) -> Result<Option<Session>> {
        self.load_session_inner(id).await
    }

    async fn retrieve_session_turns(
        &self,
        session_id: Uuid,
        query: &str,
        k: usize,
        exclude_indices_at_or_after: usize,
    ) -> Result<Vec<Turn>> {
        self.retrieve_session_turns_inner(session_id, query, k, exclude_indices_at_or_after)
            .await
    }

    async fn save_classification(
        &self,
        session_id: SessionId,
        turn_index: usize,
        assessment: &EngagementAssessment,
        classifier_identifier: &str,
    ) -> Result<()> {
        self.save_classification_inner(session_id, turn_index, assessment, classifier_identifier)
            .await
    }

    async fn load_recent_assessments(
        &self,
        session_id: SessionId,
        classifier_identifier: &str,
        k: usize,
    ) -> Result<Vec<EngagementAssessment>> {
        self.load_recent_assessments_inner(session_id, classifier_identifier, k)
            .await
    }

    async fn most_recent_session_learner_id(&self) -> Result<Option<Uuid>> {
        self.most_recent_session_learner_id_inner().await
    }

    async fn list_sessions(&self) -> Result<Vec<SessionListing>> {
        self.list_sessions_inner().await
    }

    async fn update_turn_concepts(
        &self,
        session_id: SessionId,
        turn_index: usize,
        concepts: &[String],
    ) -> Result<()> {
        self.update_turn_concepts_inner(session_id, turn_index, concepts)
            .await
    }

    async fn update_exchange_concepts(
        &self,
        session_id: SessionId,
        child_turn_index: usize,
        child_concepts: &[String],
        primer_turn_index: usize,
        primer_concepts: &[String],
    ) -> Result<()> {
        self.update_exchange_concepts_inner(
            session_id,
            child_turn_index,
            child_concepts,
            primer_turn_index,
            primer_concepts,
        )
        .await
    }

    async fn save_comprehensions(
        &self,
        session_id: SessionId,
        primer_turn_index: usize,
        assessments: &[ComprehensionAssessment],
        classifier_identifier: &str,
    ) -> Result<()> {
        self.save_comprehensions_inner(
            session_id,
            primer_turn_index,
            assessments,
            classifier_identifier,
        )
        .await
    }

    async fn save_turn_embedding(
        &self,
        session_id: SessionId,
        turn_index: usize,
        model_id: &str,
        dim: usize,
        vec: &[f32],
    ) -> Result<()> {
        self.save_turn_embedding_inner(session_id, turn_index, model_id, dim, vec)
            .await
    }

    async fn retrieve_session_turns_hybrid(
        &self,
        session_id: Uuid,
        query: &str,
        embedder: &dyn Embedder,
        params: &HybridParams,
        exclude_indices_at_or_after: usize,
    ) -> Result<Vec<Turn>> {
        self.retrieve_session_turns_hybrid_inner(
            session_id,
            query,
            embedder,
            params,
            exclude_indices_at_or_after,
        )
        .await
    }
}
