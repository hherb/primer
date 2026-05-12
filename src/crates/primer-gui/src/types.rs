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

/// Snapshot of the pedagogical signals attached to the most-recently
/// completed exchange. Returned by `get_turn_signals` and rendered in
/// the right-hand sidebar's "Current turn" section.
///
/// **One-turn lag, by design.** The classifier / extractor /
/// comprehension subsystems spawn background tasks at the END of each
/// turn and the dialogue manager drains them at the TOP of the NEXT
/// `respond_to_streaming`. So after turn N's stream completes,
/// `engagement` / `concepts` / `comprehension` reflect what the
/// background tasks produced for turn **N−1** (turn N's haven't been
/// awaited yet). `intent` is current — it's decided synchronously
/// during respond_to_streaming. This mirrors what the CLI's `--verbose`
/// flag shows and is the right trade-off for the natural inter-turn
/// pause to absorb the 3–10 s analysis wallclock.
///
/// On the very first turn of a session, every Optional field except
/// `intent` is `None`.
#[derive(Debug, Clone, Serialize)]
pub struct TurnSignals {
    /// The pedagogical intent the Primer adopted on its most recent
    /// response (e.g. `"SocraticQuestion"`, `"Encouragement"`,
    /// `"SuggestBreak"`). Current — not lagged.
    pub intent: Option<String>,

    /// The engagement assessment applied to the in-memory `LearnerModel`
    /// for the previous turn's child input. Lagged by one turn.
    pub engagement: Option<EngagementSummary>,

    /// Concepts the extractor surfaced from the previous exchange.
    /// Lagged by one turn.
    pub concepts: ConceptBreakdown,

    /// Per-concept comprehension assessments for the previous exchange.
    /// Lagged by one turn.
    pub comprehension: Vec<ComprehensionSummary>,

    /// Stable identifier of the active classifier (e.g. `"stub"`,
    /// `"llm:claude-sonnet-4-6"`). Renders as a small subtitle under
    /// the engagement section so a parent can see which model produced
    /// the rating.
    pub classifier_identifier: String,
    /// Stable identifier of the active extractor.
    pub extractor_identifier: String,
    /// Stable identifier of the active comprehension classifier.
    pub comprehension_identifier: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EngagementSummary {
    /// The `EngagementState` variant name, e.g. `"Curious"`, `"Frustrated"`.
    pub state: String,
    /// In `[0.0, 1.0]`.
    pub confidence: f32,
    /// Optional one-sentence rationale. LLM classifiers populate this;
    /// the stub does not.
    pub reasoning: Option<String>,
}

/// Two-column extractor breakdown of who introduced what.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ConceptBreakdown {
    /// Concepts the child surfaced.
    pub child: Vec<String>,
    /// Concepts the Primer introduced.
    pub primer: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComprehensionSummary {
    pub concept: String,
    /// `UnderstandingDepth` variant name: `"Aware"`, `"Familiar"`,
    /// `"Solid"`, or `"Fluent"`. The frontend renders this as a pill
    /// with depth-graded colour.
    pub depth: String,
    /// In `[0.0, 1.0]`.
    pub confidence: f32,
    /// Optional short rationale — usually a phrase the child said.
    pub evidence: Option<String>,
}
