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
}

/// A complete conversation session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub learner_id: LearnerId,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub turns: Vec<Turn>,
}

impl Session {
    pub fn new(learner_id: LearnerId) -> Self {
        Self {
            id: Uuid::new_v4(),
            learner_id,
            started_at: Utc::now(),
            ended_at: None,
            turns: vec![],
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
