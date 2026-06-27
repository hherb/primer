//! Conversation types — the shared vocabulary for dialogue state.
//!
//! A conversation is a sequence of turns between the child and the Primer.
//! Each turn carries metadata used by the pedagogical engine to track
//! comprehension, engagement, and topic flow.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::learner::LearnerId;

/// Unique identifier for a conversation session.
pub type SessionId = Uuid;

/// A single turn in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    /// Who spoke.
    pub speaker: Speaker,
    /// What was said (text, post-STT for the child).
    pub text: String,
    /// When this turn occurred.
    pub timestamp: DateTime<Utc>,
    /// The pedagogical intent behind this turn (for Primer turns).
    pub intent: Option<PedagogicalIntent>,
    /// Concepts touched in this turn.
    pub concepts: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Speaker {
    Child,
    Primer,
}

impl Speaker {
    /// Every variant, in declaration order. Source for the storage layer's
    /// validate-and-seed pass.
    pub const ALL: &'static [Self] = &[Self::Child, Self::Primer];
}

/// What the Primer was trying to accomplish with a given response.
/// This is metadata for the pedagogical engine's own use — the child
/// never sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PedagogicalIntent {
    /// Asking a guiding question to lead the child toward discovery.
    SocraticQuestion,
    /// Probing whether the child's understanding is genuine vs. parroting.
    ComprehensionCheck,
    /// Providing a concrete example or analogy to aid understanding.
    Scaffolding,
    /// Offering encouragement or reducing frustration.
    Encouragement,
    /// Extending a concept the child has grasped — "now what if...?"
    Extension,
    /// Providing a direct factual answer (appropriate for pure fact queries).
    DirectAnswer,
    /// Pivoting from a factual answer to a Socratic follow-up.
    AnswerThenPivot,
    /// Suggesting the session end (the Primer never tries to maximise engagement).
    SessionClose,
    /// Gentle nudge to take a break — the child can keep going. Fired
    /// by the wallclock-based break-suggestion gate; never a forced halt.
    SuggestBreak,
    /// The child asserted a claim; ask how she knows or how she could
    /// check, rather than confirming or correcting it outright.
    ProbeReasoning,
}

impl PedagogicalIntent {
    /// Every variant, in declaration order. Source for the storage layer's
    /// validate-and-seed pass.
    pub const ALL: &'static [Self] = &[
        Self::SocraticQuestion,
        Self::ComprehensionCheck,
        Self::Scaffolding,
        Self::Encouragement,
        Self::Extension,
        Self::DirectAnswer,
        Self::AnswerThenPivot,
        Self::SessionClose,
        Self::SuggestBreak,
        Self::ProbeReasoning,
    ];

    /// Canonical machine-readable name. Stable identifier exposed across
    /// the FFI surface (Tauri `TurnSignals`, future verbose-CLI tracing).
    /// Don't rename — frontends key behaviour (e.g. badge colour, log
    /// filters) on these exact strings.
    pub fn name(self) -> &'static str {
        match self {
            Self::SocraticQuestion => "SocraticQuestion",
            Self::ComprehensionCheck => "ComprehensionCheck",
            Self::Scaffolding => "Scaffolding",
            Self::Encouragement => "Encouragement",
            Self::Extension => "Extension",
            Self::DirectAnswer => "DirectAnswer",
            Self::AnswerThenPivot => "AnswerThenPivot",
            Self::SessionClose => "SessionClose",
            Self::SuggestBreak => "SuggestBreak",
            Self::ProbeReasoning => "ProbeReasoning",
        }
    }
}

impl std::fmt::Display for PedagogicalIntent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// A complete conversation session.
///
/// `summary` is a rolling LLM-generated condensation of the turns that
/// have fallen out of the active context window. It exists so the model
/// retains long-range memory across hours of conversation without
/// blowing the context budget. Empty until enough turns have accumulated
/// for the dialogue manager to trigger a summarization pass.
///
/// `summary_through_turn_index` records the *exclusive* upper bound of
/// turns covered by `summary`: turns at indices `0..summary_through_turn_index`
/// have been summarized. Defaults to 0 (nothing covered yet).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub learner_id: LearnerId,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub turns: Vec<Turn>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub summary_through_turn_index: usize,
}

impl Session {
    pub fn new(learner_id: LearnerId) -> Self {
        Self {
            id: Uuid::new_v4(),
            learner_id,
            started_at: Utc::now(),
            ended_at: None,
            turns: vec![],
            summary: String::new(),
            summary_through_turn_index: 0,
        }
    }

    pub fn add_turn(&mut self, turn: Turn) {
        self.turns.push(turn);
    }

    /// Returns the last N turns (for context window construction).
    pub fn recent_turns(&self, n: usize) -> &[Turn] {
        let start = self.turns.len().saturating_sub(n);
        &self.turns[start..]
    }
}

/// Lightweight session metadata for picker / index views.
///
/// Held outside `Session` because consumers (the GUI's session picker
/// today; potentially a CLI `--list-sessions` flag tomorrow) want
/// per-row aggregates — turn count, last activity — without paying the
/// cost of materializing every `Turn`. `SessionStore::list_sessions`
/// returns a `Vec<SessionListing>` ordered most-recent-activity first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionListing {
    pub id: SessionId,
    pub learner_id: LearnerId,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    /// Max(turns.timestamp) or `started_at` if no turns exist yet.
    pub last_activity: DateTime<Utc>,
    pub turn_count: usize,
    /// Rolling LLM summary; may be empty for short / fresh sessions.
    pub summary: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speaker_all_lists_every_variant() {
        assert_eq!(Speaker::ALL.len(), 2);
        assert!(Speaker::ALL.contains(&Speaker::Child));
        assert!(Speaker::ALL.contains(&Speaker::Primer));
    }

    #[test]
    fn pedagogical_intent_all_lists_every_variant() {
        assert_eq!(PedagogicalIntent::ALL.len(), 10);
        // Spot-check a few representatives.
        assert!(PedagogicalIntent::ALL.contains(&PedagogicalIntent::SocraticQuestion));
        assert!(PedagogicalIntent::ALL.contains(&PedagogicalIntent::SessionClose));
        assert!(PedagogicalIntent::ALL.contains(&PedagogicalIntent::SuggestBreak));
    }

    #[test]
    fn pedagogical_intent_name_matches_canonical_strings() {
        assert_eq!(
            PedagogicalIntent::SocraticQuestion.name(),
            "SocraticQuestion"
        );
        assert_eq!(
            PedagogicalIntent::ComprehensionCheck.name(),
            "ComprehensionCheck"
        );
        assert_eq!(PedagogicalIntent::Scaffolding.name(), "Scaffolding");
        assert_eq!(PedagogicalIntent::Encouragement.name(), "Encouragement");
        assert_eq!(PedagogicalIntent::Extension.name(), "Extension");
        assert_eq!(PedagogicalIntent::DirectAnswer.name(), "DirectAnswer");
        assert_eq!(PedagogicalIntent::AnswerThenPivot.name(), "AnswerThenPivot");
        assert_eq!(PedagogicalIntent::SessionClose.name(), "SessionClose");
        assert_eq!(PedagogicalIntent::SuggestBreak.name(), "SuggestBreak");
        assert_eq!(PedagogicalIntent::ProbeReasoning.name(), "ProbeReasoning");
    }

    #[test]
    fn pedagogical_intent_display_uses_name() {
        assert_eq!(
            format!("{}", PedagogicalIntent::SuggestBreak),
            "SuggestBreak"
        );
    }
}
