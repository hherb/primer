//! Learner model — persistent representation of a child's knowledge state.
//!
//! The learner model tracks what a child knows, how deeply they understand it,
//! how they prefer to learn, and where the gaps are. It is the pedagogical
//! engine's memory — the thing that makes the Primer a companion rather than
//! a chatbot.
//!
//! Design principles:
//! - All data is local. Nothing leaves the device without explicit parental consent.
//! - The model is updated in real-time during conversation.
//! - It is queryable by the dialogue manager to construct appropriate prompts.
//! - It persists across sessions (SQLite-backed).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a learner (child).
pub type LearnerId = Uuid;

/// A learner's profile — identity and preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnerProfile {
    pub id: LearnerId,
    pub name: String,
    /// Age in years (used for developmental-stage adaptation).
    pub age: u8,
    /// Preferred language(s) — ISO 639-1 codes, ordered by preference.
    pub languages: Vec<String>,
    /// When the profile was created.
    pub created_at: DateTime<Utc>,
    /// When the profile was last active.
    pub last_active: DateTime<Utc>,
}

/// How deeply a child understands a concept.
///
/// Based on Bloom's taxonomy (revised), but simplified for
/// real-time assessment during conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum UnderstandingDepth {
    /// Has not encountered this concept.
    Unknown,
    /// Has heard of it / can recognise the term.
    Aware,
    /// Can recall and repeat the definition or fact.
    Recall,
    /// Can explain it in their own words.
    Comprehension,
    /// Can apply it in a new context they haven't seen before.
    Application,
    /// Can break it down, compare, and reason about it.
    Analysis,
}

/// A node in the learner's concept graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptState {
    /// The concept identifier (e.g., "physics:gravity", "biology:photosynthesis").
    pub concept_id: String,
    /// Current assessed understanding depth.
    pub depth: UnderstandingDepth,
    /// Confidence in this assessment (0.0 – 1.0).
    /// Low confidence means we should re-probe before assuming.
    pub confidence: f32,
    /// Number of times this concept has been discussed.
    pub encounter_count: u32,
    /// When the concept was last discussed.
    pub last_encountered: Option<DateTime<Utc>>,
    /// Notes from the dialogue manager (e.g., "struggled with the
    /// distinction between mass and weight").
    pub notes: Vec<String>,
}

/// Observed learning style preferences — updated over time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningPreferences {
    /// Relative preference for learning through stories/narrative (0.0 – 1.0).
    pub narrative: f32,
    /// Relative preference for learning through questions/Socratic dialogue.
    pub socratic: f32,
    /// Relative preference for visual/spatial explanations.
    pub visual: f32,
    /// Relative preference for hands-on / experimental approaches.
    pub kinesthetic: f32,
    /// Average session length before disengagement (minutes).
    pub typical_session_minutes: f32,
    /// Topics that sustain attention longest.
    pub high_engagement_topics: Vec<String>,
}

impl Default for LearningPreferences {
    fn default() -> Self {
        Self {
            narrative: 0.5,
            socratic: 0.5,
            visual: 0.5,
            kinesthetic: 0.5,
            typical_session_minutes: 20.0,
            high_engagement_topics: vec![],
        }
    }
}

/// A snapshot of a child's emotional/engagement state during a session.
/// Inferred cautiously from voice tone and response patterns.
/// Used ONLY to detect frustration or disengagement — never to manipulate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngagementState {
    /// Actively engaged, responding readily.
    Engaged,
    /// Thinking — longer pauses, but still present.
    Reflecting,
    /// Frustrated and stuck — no progress, gives up. Routes to Scaffolding.
    FrustratedStuck,
    /// Frustrated but still articulating an attempt. Routes to Encouragement.
    FrustratedTrying,
    /// Losing interest (long pauses, off-topic responses).
    Disengaging,
    /// State cannot be determined.
    Unknown,
}

impl EngagementState {
    pub const ALL: &'static [Self] = &[
        Self::Engaged,
        Self::Reflecting,
        Self::FrustratedStuck,
        Self::FrustratedTrying,
        Self::Disengaging,
        Self::Unknown,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engagement_state_all_lists_every_variant() {
        assert_eq!(EngagementState::ALL.len(), 6);
        assert!(EngagementState::ALL.contains(&EngagementState::FrustratedStuck));
        assert!(EngagementState::ALL.contains(&EngagementState::FrustratedTrying));
    }
}

/// The complete learner model for one child.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnerModel {
    pub profile: LearnerProfile,
    pub concepts: Vec<ConceptState>,
    pub preferences: LearningPreferences,
    pub current_engagement: EngagementState,
}
