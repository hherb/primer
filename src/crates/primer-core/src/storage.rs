//! Persistence traits for session and learner-model storage.
//!
//! This crate owns the trait only. Concrete implementations live in
//! sibling crates (e.g. `primer-storage` for SQLite).

use async_trait::async_trait;
use uuid::Uuid;

use crate::conversation::{Session, Turn};
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
    async fn retrieve_session_turns(
        &self,
        session_id: Uuid,
        query: &str,
        k: usize,
        exclude_indices_at_or_after: usize,
    ) -> Result<Vec<Turn>>;
}
