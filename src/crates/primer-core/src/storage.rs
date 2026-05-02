//! Persistence traits for session and learner-model storage.
//!
//! This crate owns the trait only. Concrete implementations live in
//! sibling crates (e.g. `primer-storage` for SQLite).

use async_trait::async_trait;
use uuid::Uuid;

use crate::classifier::EngagementAssessment;
use crate::conversation::{Session, SessionId, Turn};
use crate::error::Result;
use crate::learner::LearnerModel;

/// Persists conversation sessions.
///
/// Implementations must make `save_session` idempotent — the dialogue
/// manager calls it after every turn, repeatedly, with the same session
/// growing over time. Repeated calls with no in-memory changes must
/// leave the store unchanged.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Persist the full current state of `session`. Append-only on
    /// turns: implementations must not delete turns that exist on disk
    /// but no longer exist in `session.turns` (the in-memory `Session`
    /// type is itself append-only).
    async fn save_session(&self, session: &Session) -> Result<()>;

    /// Load a session by its UUID, including all turns (with their
    /// concepts) and the rolling summary fields.
    ///
    /// Returns `Ok(None)` when no session with that id exists; reserves
    /// `Err` for genuine I/O / decoding failures. The `Option` shape
    /// keeps "miss" out of the error vocabulary so the CLI can format
    /// its own user-facing message.
    async fn load_session(&self, id: Uuid) -> Result<Option<Session>>;

    /// Retrieve up to `k` turns from the given session whose text
    /// matches `query` (full-text search), excluding turns at index
    /// `>= exclude_indices_at_or_after`. Used by the dialogue manager
    /// to surface relevant pre-window turns alongside the rolling
    /// summary.
    ///
    /// Implementations should treat `query` as a literal phrase rather
    /// than as a backend-specific search expression — the caller passes
    /// raw child input. Returned turns carry empty `concepts` vectors;
    /// retrieval is read-only and concept tags are not needed by the
    /// caller. Returns an empty Vec on no matches; reserves `Err` for
    /// genuine I/O failures.
    ///
    /// **Index-lag invariant.** The dialogue manager calls this *after*
    /// appending the current child turn to its in-memory `Session` but
    /// *before* the next `save_session` flushes that turn to disk, so
    /// the search index may be one save behind the in-memory session.
    /// Callers MUST set `exclude_indices_at_or_after = total - window`
    /// so the still-unsaved tail of the conversation is excluded by
    /// index regardless of whether it has reached the index yet. Lower
    /// bounds (e.g. `total - window - 1`) would silently return stale
    /// — or duplicate — results.
    async fn retrieve_session_turns(
        &self,
        session_id: Uuid,
        query: &str,
        k: usize,
        exclude_indices_at_or_after: usize,
    ) -> Result<Vec<Turn>>;

    /// Persist one classification of one child turn. Resolves
    /// `(session_id, turn_index)` → `turn_id` internally; lazily creates
    /// the `classifiers` lookup row if `classifier_identifier` is new.
    ///
    /// A UNIQUE constraint on `(turn_id, classifier_id)` means calling
    /// this twice for the same turn and the same classifier is a hard
    /// error — the caller has a logic bug if it tries.
    async fn save_classification(
        &self,
        session_id: SessionId,
        turn_index: usize,
        assessment: &EngagementAssessment,
        classifier_identifier: &str,
    ) -> Result<()>;

    /// Load the most recent `k` classifications for this session, filtered
    /// by `classifier_identifier`. Ordered oldest-first within the result
    /// so callers can use the slice directly as a trajectory buffer.
    ///
    /// Returns an empty `Vec` if the classifier has never produced output
    /// for this session. Reserves `Err` for genuine I/O failures.
    async fn load_recent_assessments(
        &self,
        session_id: SessionId,
        classifier_identifier: &str,
        k: usize,
    ) -> Result<Vec<EngagementAssessment>>;

    /// Return the `learner_id` of the most-recent session in this DB,
    /// if any. Used by the CLI on first-run after a v3 → v4 upgrade to
    /// adopt the existing session-id as the new learner's persistent
    /// UUID, eliminating the otherwise-orphan-session class.
    ///
    /// Returns `Ok(None)` for a DB with no sessions. Reserves `Err` for
    /// genuine I/O / decoding failures.
    async fn most_recent_session_learner_id(&self) -> Result<Option<Uuid>>;
}

/// Persists the per-child `LearnerModel` to disk.
///
/// One implementation lives per DB file (the application invariant: at
/// most one `learners` row per file). `load_learner` returns `Ok(None)`
/// if the file has never had a learner row created — first-run signal.
///
/// `save_learner` is monotonic on concepts: it upserts every concept
/// in `learner.concepts` but never deletes `learner_concepts` rows.
/// Concept state is monotonic across a child's lifetime — knowing-then-
/// not-knowing is a separate event ("forgetting") that should be an
/// explicit operation, not a side effect.
#[async_trait]
pub trait LearnerStore: Send + Sync {
    /// Look up the (single) learner row in this DB. Returns Ok(None) if
    /// the file has never had a learner row created.
    ///
    /// **Returned `recent_assessments` is always empty.** That field is
    /// the per-session classifier trajectory buffer; it lives in
    /// `turn_classifications`, not in the `learners` table. Callers that
    /// need it populated MUST follow this call with
    /// `SessionStore::load_recent_assessments(session_id, classifier, k)`
    /// — typically as part of resuming a session via
    /// `DialogueManager::resume_session`. Code paths that load a learner
    /// without resuming a session will see an empty buffer; if that
    /// matters to them, they need to call `load_recent_assessments`
    /// themselves.
    async fn load_learner(&self) -> Result<Option<LearnerModel>>;

    /// Persist the full current state of `learner`. Idempotent at the
    /// row level: the `learners` row is upserted (`INSERT … ON CONFLICT
    /// DO UPDATE`), and `learner_concepts` rows are upserted by
    /// `(learner_id, concept_id)` PRIMARY KEY. Concepts dropped from
    /// `learner.concepts` are NOT removed from the DB — concept state is
    /// monotonic across a child's lifetime.
    async fn save_learner(&self, learner: &LearnerModel) -> Result<()>;
}
