//! The dialogue manager — the Primer's conversational brain.
//!
//! The `DialogueManager` orchestrates a single conversation session:
//!
//! 1. Receives the child's input (text, post-STT).
//! 2. Decides what pedagogical intent to pursue next.
//! 3. Retrieves relevant knowledge passages for grounding.
//! 4. Constructs a prompt and sends it to the inference backend.
//! 5. Records the exchange and updates the learner model.
//!
//! It does NOT own the inference backend or knowledge base — those are
//! injected as trait objects, keeping this module testable with stubs.

use chrono::Utc;
use primer_core::config::PedagogyConfig;
use primer_core::conversation::{PedagogicalIntent, Session, Speaker, Turn};
use primer_core::error::{PrimerError, Result};
use primer_core::inference::{GenerationParams, InferenceBackend};
use primer_core::knowledge::{KnowledgeBase, RetrievalParams};
use primer_core::learner::LearnerModel;

use crate::prompt_builder;

/// The dialogue manager for a single session.
///
/// Holds references to all the subsystems it needs, plus the mutable
/// session and learner model state. The CLI (or future GUI) drives
/// the conversation by calling `respond_to()` in a loop.
pub struct DialogueManager<'a> {
    /// The learner model — updated in place as we learn about the child.
    pub learner: LearnerModel,
    /// The current conversation session.
    pub session: Session,
    /// Inference backend (local model or cloud API).
    inference: &'a dyn InferenceBackend,
    /// Knowledge base for RAG retrieval.
    knowledge: &'a dyn KnowledgeBase,
    /// Pedagogical configuration.
    config: PedagogyConfig,
}

impl<'a> DialogueManager<'a> {
    /// Create a new dialogue manager for a session.
    pub fn new(
        learner: LearnerModel,
        inference: &'a dyn InferenceBackend,
        knowledge: &'a dyn KnowledgeBase,
        config: PedagogyConfig,
    ) -> Self {
        let session = Session::new(learner.profile.id);
        Self {
            learner,
            session,
            inference,
            knowledge,
            config,
        }
    }

    /// The opening move — the Primer greets the child and invites
    /// a topic. This is the very first turn in a session.
    pub async fn open_session(&mut self) -> Result<String> {
        let name = &self.learner.profile.name;
        let greeting = format!(
            "Hello, {name}. What are you curious about today?"
        );

        self.session.add_turn(Turn {
            speaker: Speaker::Primer,
            text: greeting.clone(),
            timestamp: Utc::now(),
            intent: Some(PedagogicalIntent::SocraticQuestion),
            concepts: vec![],
        });

        Ok(greeting)
    }

    /// Process the child's input and generate the Primer's response.
    ///
    /// This is the core conversation loop step:
    /// 1. Record the child's turn.
    /// 2. Decide pedagogical intent.
    /// 3. Retrieve relevant knowledge.
    /// 4. Build the prompt.
    /// 5. Generate the response.
    /// 6. Record the Primer's turn.
    /// 7. Update the learner model.
    pub async fn respond_to(&mut self, child_input: &str) -> Result<String> {
        // 1. Record the child's turn.
        let child_turn = Turn {
            speaker: Speaker::Child,
            text: child_input.to_string(),
            timestamp: Utc::now(),
            intent: None,
            concepts: vec![], // TODO: extract concepts from input
        };
        self.session.add_turn(child_turn);

        // 2. Decide what the Primer should do next.
        let intent = prompt_builder::decide_intent(&self.learner, &self.session);

        // 3. Retrieve relevant knowledge passages.
        let knowledge_context = self.retrieve_knowledge(child_input).await;

        // 4. Build the complete prompt.
        let prompt = prompt_builder::build_prompt(
            &self.learner,
            &self.session,
            intent,
            &knowledge_context,
            self.config.context_window_turns,
        );

        // 5. Generate the response.
        let params = GenerationParams::default();
        let response = self
            .inference
            .generate(&prompt, &params)
            .await
            .map_err(|e| PrimerError::Inference(format!("Generation failed: {e}")))?;

        // 6. Record the Primer's turn.
        let active_concepts = prompt_builder::extract_active_concepts(&self.session, 4);
        let primer_turn = Turn {
            speaker: Speaker::Primer,
            text: response.clone(),
            timestamp: Utc::now(),
            intent: Some(intent),
            concepts: active_concepts,
        };
        self.session.add_turn(primer_turn);

        // 7. Update the learner model (placeholder — a production version
        //    would assess comprehension from the child's response).
        self.update_learner_model(child_input, &intent);

        Ok(response)
    }

    /// Check whether the session has run long enough that the Primer
    /// should suggest a break.
    pub fn should_suggest_break(&self) -> bool {
        let elapsed = Utc::now()
            .signed_duration_since(self.session.started_at)
            .num_minutes();
        elapsed >= self.config.max_session_minutes as i64
    }

    /// End the session gracefully.
    pub fn close_session(&mut self) {
        self.session.ended_at = Some(Utc::now());
    }

    // ─── Private helpers ─────────────────────────────────────────────

    /// Retrieve knowledge passages relevant to the child's input.
    /// Falls back gracefully if the knowledge base is empty or errors.
    async fn retrieve_knowledge(
        &self,
        query: &str,
    ) -> Vec<primer_core::knowledge::Passage> {
        let params = RetrievalParams {
            top_k: 3,
            min_score: 0.5,
            source_filter: vec![],
        };

        self.knowledge
            .retrieve(query, &params)
            .await
            .unwrap_or_default()
    }

    /// Update the learner model based on the conversation evidence.
    ///
    /// This is deliberately minimal for the scaffold. A production version
    /// would:
    /// - Parse the child's response for comprehension signals
    /// - Use the LLM to classify understanding depth
    /// - Update concept graph confidence scores
    /// - Detect engagement state from response patterns
    fn update_learner_model(&mut self, child_input: &str, _intent: &PedagogicalIntent) {
        // Simple engagement heuristic: very short responses may indicate
        // frustration or disengagement.
        let word_count = child_input.split_whitespace().count();

        use primer_core::learner::EngagementState;
        self.learner.current_engagement = if word_count == 0 {
            EngagementState::Disengaging
        } else if word_count < 3 {
            // Could be frustration ("I don't know") or just a short answer.
            // Don't over-interpret — keep previous state unless it was Engaged.
            match self.learner.current_engagement {
                EngagementState::Engaged => EngagementState::Reflecting,
                other => other,
            }
        } else {
            EngagementState::Engaged
        };
    }
}
