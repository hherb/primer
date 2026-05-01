//! Persistence traits for session and learner-model storage.
//!
//! This crate owns the trait only. Concrete implementations live in
//! sibling crates (e.g. `primer-storage` for SQLite).

use async_trait::async_trait;
use uuid::Uuid;

use crate::classifier::EngagementAssessment;
use crate::conversation::{Session, SessionId, Turn};
use crate::error::Result;

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
}
