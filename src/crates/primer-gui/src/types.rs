//! DTOs serialised across the Tauri IPC boundary.
//!
//! Step 3 surfaces a single [`SessionInfo`] DTO carrying the bare
//! minimum the frontend needs to render its header after
//! `start_session` lands (learner profile + backend identity). Richer
//! sidebar payloads (`TurnSignals`, `LearnerSnapshot`, etc.) arrive in
//! steps 5 / 6 alongside the sidebar implementation.

use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    /// `None` until the first `send_message` opens the underlying
    /// `Session`. See [`ActiveSession::session_id`](crate::state::ActiveSession::session_id).
    pub session_id: Option<Uuid>,
    pub learner: LearnerSummary,
    /// Backend kind: "stub" | "cloud" | "ollama".
    pub backend_kind: String,
    /// Main model id (e.g. "claude-sonnet-4-6", "llama3.2", "stub").
    pub main_model: String,
    /// Locale pack id ("en", "de", ...).
    pub locale: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LearnerSummary {
    pub id: Uuid,
    pub name: String,
    pub age: u8,
    /// Number of concepts the learner has encountered so far (across
    /// all sessions). Useful for the header to show "Binti · 9 · 42
    /// concepts" without pulling the full vocab list.
    pub concept_count: usize,
}
