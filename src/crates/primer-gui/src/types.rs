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
    /// `"SuggestBreak"`). Current — not lagged. Value comes from
    /// `PedagogicalIntent::name()` — canonical, stable across releases;
    /// frontends key behaviour on these exact strings.
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
    /// The `EngagementState` variant name from `EngagementState::name()`,
    /// e.g. `"Engaged"`, `"Reflecting"`, `"FrustratedStuck"`,
    /// `"FrustratedTrying"`, `"Disengaging"`, `"Unknown"`.
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
    /// `UnderstandingDepth` variant name from `UnderstandingDepth::name()`:
    /// `"Unknown"`, `"Aware"`, `"Recall"`, `"Comprehension"`,
    /// `"Application"`, or `"Analysis"`. The frontend lowercases this for
    /// the depth-pill `data-depth` selector — keep the lowercased forms
    /// in sync with [`styles.css`](../../ui/styles.css).
    pub depth: String,
    /// In `[0.0, 1.0]`.
    pub confidence: f32,
    /// Optional short rationale — usually a phrase the child said.
    pub evidence: Option<String>,
}

/// Snapshot of the learner's longitudinal state — what the sidebar's
/// "Learner" section renders.
///
/// Refreshed at the same trigger as [`TurnSignals`] (the
/// `primer://turn_complete` event); the underlying `LearnerModel`
/// mutates across turns as the comprehension classifier promotes
/// depth, the extractor adds concepts, and the vocab scheduler moves
/// box levels. Reading it is cheap — one short DM-mutex lock for the
/// shape transform, no I/O.
#[derive(Debug, Clone, Serialize)]
pub struct LearnerSnapshot {
    pub profile: LearnerProfileView,
    /// Top-N concepts that are most overdue for passive review, picked
    /// by [`primer_core::vocab::due_concepts`]. Empty for a fresh
    /// learner with no concepts yet.
    pub vocab_due: Vec<DueConcept>,
    /// Counts of concepts at each [`primer_core::learner::UnderstandingDepth`]
    /// variant, in canonical order (Unknown → Analysis). Always six
    /// entries — depths the learner has never reached carry `count = 0`.
    pub depth_distribution: Vec<DepthCount>,
    /// Recent engagement states in chronological order (oldest first,
    /// newest last). The sidebar renders left-to-right, so the
    /// most-recent state is the rightmost dot.
    /// Variant names match [`primer_core::learner::EngagementState::name`].
    pub recent_engagement: Vec<String>,
    /// Total concept count. `depth_distribution` sums to the same
    /// number, but a direct field saves a JS reduce.
    pub concept_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct LearnerProfileView {
    pub id: Uuid,
    pub name: String,
    pub age: u8,
    /// Locale pack id ("en", "de", ...).
    pub locale: String,
}

/// One row in the vocab-due list.
#[derive(Debug, Clone, Serialize)]
pub struct DueConcept {
    pub concept_id: String,
    /// 0..=4 — number of filled dots to render. `4` is the 30-day box.
    pub box_level: u8,
    /// `UnderstandingDepth` variant name — useful when the sidebar
    /// wants to show the depth alongside the dot row.
    pub depth: String,
    /// Days until the concept next becomes due. Negative = already
    /// overdue by that many days. `chrono::Duration::num_days`
    /// truncates toward zero, so sub-day remainders on both sides
    /// round to 0 — "0.4 days" reads as "due now" rather than "due
    /// tomorrow", "-0.4 days" reads as "due now" rather than "1 day
    /// late". The asymmetric-overdue side is the deliberate forgiving
    /// choice over a true floor.
    pub days_until_due: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DepthCount {
    /// `UnderstandingDepth` variant name.
    pub depth: String,
    pub count: usize,
}

/// One row in the sidebar's "Session" turn-by-turn list.
///
/// Returned by `list_session_turns`; the frontend renders the list
/// once on session start and updates it after each `primer://turn_complete`.
/// Click-to-scroll uses [`Self::index`] as the
/// `data-turn-index` selector on the matching chat bubble.
///
/// Lightweight by design — `text_preview` is truncated server-side
/// so big sessions don't blow up the IPC payload. The full turn text
/// is still on disk via `SessionStore::load_session` for any future
/// "Inspect turn N" panel.
#[derive(Debug, Clone, Serialize)]
pub struct SessionTurnSummary {
    /// Zero-based index in the session's turn timeline. Matches
    /// [`TurnComplete::child_turn_index`] / `primer_turn_index`.
    pub index: usize,
    /// Stable lowercase identifier — `"child"` or `"primer"` — produced
    /// by `commands::session::speaker_name`. Used directly as a
    /// `[data-speaker=…]` selector hook on the frontend; do not rename.
    pub speaker: String,
    /// Truncated turn text — server-side cap matches the sidebar's
    /// visual budget for an at-a-glance scan. The full text is in the
    /// rendered chat bubble; this is just the row label.
    pub text_preview: String,
    /// `true` when the original text was truncated. Useful for the
    /// frontend's tooltip ("…") or for a future "expand" affordance.
    pub truncated: bool,
    /// `PedagogicalIntent::name()` from the turn's stored intent. `None` for
    /// child turns and for any Primer turn whose intent was never set
    /// (only `open_session`'s greeting hits that today).
    pub intent: Option<String>,
    /// Concepts that the extractor backfilled onto this turn. Stable
    /// for past turns; an in-flight current-turn entry shows what the
    /// extractor has surfaced so far (may grow on subsequent refresh).
    pub concepts: Vec<String>,
    /// Wallclock at which the turn landed. Surfaced as an ISO-8601
    /// string so the frontend can format relatively without parsing.
    pub timestamp: String,
}
