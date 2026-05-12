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

/// Payload of the `primer://turn_complete` event, emitted by
/// `send_message` once a streaming response has fully landed.
///
/// Step 4 carries only the bare essentials the chat surface needs.
/// Sidebar-shaped signals (intent badge, engagement confidence,
/// extracted concepts, comprehension depth, retrieved passages) land
/// in steps 5–6 by extending this struct — the event name stays the
/// same so the frontend listener doesn't need to know which fields
/// are present until it tries to render them.
#[derive(Debug, Clone, Serialize)]
pub struct TurnComplete {
    /// UUID of the underlying `Session`. Useful for the frontend to
    /// confirm subsequent `send_message` calls are addressed to the
    /// session it thinks they are (no per-call session-id field is
    /// needed because the GUI is single-session).
    pub session_id: Uuid,

    /// Zero-based index of the child's turn in the session timeline.
    pub child_turn_index: usize,

    /// Zero-based index of the Primer's response turn (always
    /// `child_turn_index + 1`).
    pub primer_turn_index: usize,
}

/// Payload of the `primer://chunk` event — a single token (or short
/// run of tokens) emitted by the streaming inference backend.
///
/// Carries the chunk text plus the index of the response bubble it
/// belongs to so the frontend can append to the correct DOM node
/// even if a future feature multiplexes streams.
#[derive(Debug, Clone, Serialize)]
pub struct ChunkEvent {
    /// Zero-based index of the Primer's response turn this chunk
    /// belongs to. Matches [`TurnComplete::primer_turn_index`].
    pub primer_turn_index: usize,
    /// The token text — append directly to the bubble.
    pub text: String,
}
