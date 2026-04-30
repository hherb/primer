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
use futures::StreamExt;
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
    /// Optional session persistence. When set, the session is saved after
    /// every `respond_to_streaming` call (success or mid-stream error).
    storage: Option<&'a dyn primer_core::storage::SessionStore>,
    /// Pedagogical configuration.
    config: PedagogyConfig,
}

impl<'a> DialogueManager<'a> {
    /// Create a new dialogue manager for a session.
    pub fn new(
        learner: LearnerModel,
        inference: &'a dyn InferenceBackend,
        knowledge: &'a dyn KnowledgeBase,
        storage: Option<&'a dyn primer_core::storage::SessionStore>,
        config: PedagogyConfig,
    ) -> Self {
        let session = Session::new(learner.profile.id);
        Self {
            learner,
            session,
            inference,
            knowledge,
            storage,
            config,
        }
    }

    /// The opening move — the Primer greets the child and invites
    /// a topic. This is the very first turn in a session.
    pub async fn open_session(&mut self) -> Result<String> {
        let name = &self.learner.profile.name;
        let greeting = format!("Hello, {name}. What are you curious about today?");

        self.session.add_turn(Turn {
            speaker: Speaker::Primer,
            text: greeting.clone(),
            timestamp: Utc::now(),
            intent: Some(PedagogicalIntent::SocraticQuestion),
            concepts: vec![],
        });

        if let Some(store) = self.storage {
            if let Err(e) = store.save_session(&self.session).await {
                tracing::warn!("session save failed: {e}");
            }
        }

        Ok(greeting)
    }

    /// Process the child's input and generate the Primer's response.
    /// Convenience wrapper around `respond_to_streaming` that discards
    /// per-chunk callbacks. See that method for the full contract.
    pub async fn respond_to(&mut self, child_input: &str) -> Result<String> {
        self.respond_to_streaming(child_input, |_| {}).await
    }

    /// Streaming variant of `respond_to`: invokes `on_chunk` for every
    /// non-empty token chunk emitted by the inference backend, in order.
    ///
    /// On a clean stream the closure receives chunks like
    /// `["Hel", "lo", " there"]`; the returned `String` is the full
    /// accumulation (`"Hello there"`).
    ///
    /// On a mid-stream error, the partial accumulation is discarded:
    /// the Primer turn is **not** recorded, the learner model is not
    /// updated, and the error is returned. The child's turn (recorded
    /// at step 1) stays in the session.
    pub async fn respond_to_streaming<F>(
        &mut self,
        child_input: &str,
        mut on_chunk: F,
    ) -> Result<String>
    where
        F: FnMut(&str),
    {
        // 1. Record the child's turn.
        let child_turn = Turn {
            speaker: Speaker::Child,
            text: child_input.to_string(),
            timestamp: Utc::now(),
            intent: None,
            concepts: vec![],
        };
        self.session.add_turn(child_turn);

        // 2. Decide intent + retrieve knowledge + build prompt.
        let intent = prompt_builder::decide_intent(&self.learner, &self.session);
        let knowledge_context = self.retrieve_knowledge(child_input).await;
        let prompt = prompt_builder::build_prompt(
            &self.learner,
            &self.session,
            intent,
            &knowledge_context,
            self.config.context_window_turns,
        );

        // 3. Stream the response, accumulating into a single String.
        // The result is captured in `result` so we can run the save call
        // exactly once afterwards, regardless of which path we took.
        let params = GenerationParams::default();
        let result: Result<String> = async {
            let mut stream = self
                .inference
                .generate_stream(&prompt, &params)
                .await
                .map_err(|e| PrimerError::Inference(format!("Generation failed: {e}")))?;

            let mut accumulated = String::new();
            while let Some(item) = stream.next().await {
                let chunk = item.inspect_err(|e| {
                    tracing::warn!("Stream error mid-generation: {e}");
                })?;
                if !chunk.text.is_empty() {
                    on_chunk(&chunk.text);
                    accumulated.push_str(&chunk.text);
                }
                if chunk.done {
                    break;
                }
            }
            Ok(accumulated)
        }
        .await;

        // 4. On success, record the Primer turn and update the learner.
        if let Ok(accumulated) = &result {
            if accumulated.is_empty() {
                tracing::warn!("Inference stream produced no text");
            }
            let active_concepts = prompt_builder::extract_active_concepts(&self.session, 4);
            let primer_turn = Turn {
                speaker: Speaker::Primer,
                text: accumulated.clone(),
                timestamp: Utc::now(),
                intent: Some(intent),
                concepts: active_concepts,
            };
            self.session.add_turn(primer_turn);
            self.update_learner_model(child_input, &intent);
        }

        // 5. Save the session if a store is configured. Runs on both Ok
        //    and Err paths. Save failures are logged, not propagated.
        if let Some(store) = self.storage {
            if let Err(e) = store.save_session(&self.session).await {
                tracing::warn!("session save failed: {e}");
            }
        }

        result
    }

    /// Check whether the session has run long enough that the Primer
    /// should suggest a break.
    pub fn should_suggest_break(&self) -> bool {
        let elapsed = Utc::now()
            .signed_duration_since(self.session.started_at)
            .num_minutes();
        elapsed >= self.config.max_session_minutes as i64
    }

    /// End the session gracefully. Records `ended_at` and, if storage is
    /// configured, fires a final save so the timestamp lands on disk. Save
    /// failures are logged via `tracing::warn!` rather than propagated —
    /// matching `respond_to_streaming`'s save-failure semantics.
    pub async fn close_session(&mut self) {
        self.session.ended_at = Some(Utc::now());
        if let Some(store) = self.storage {
            if let Err(e) = store.save_session(&self.session).await {
                tracing::warn!("session save failed during close: {e}");
            }
        }
    }

    // ─── Private helpers ─────────────────────────────────────────────

    /// Retrieve knowledge passages relevant to the child's input.
    /// Falls back gracefully if the knowledge base is empty or errors.
    async fn retrieve_knowledge(&self, query: &str) -> Vec<primer_core::knowledge::Passage> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use futures::stream;
    use primer_core::config::PedagogyConfig;
    use primer_core::inference::{
        GenerationParams, InferenceBackend, Prompt, TokenChunk, TokenStream,
    };
    use primer_core::knowledge::{KnowledgeBase, Passage, RetrievalParams};
    use primer_core::learner::{
        EngagementState, LearnerModel, LearnerProfile, LearningPreferences,
    };
    use std::sync::Mutex;
    use uuid::Uuid;

    /// Test inference backend that emits a pre-configured sequence of stream items.
    struct ScriptedBackend {
        // Wrap in Mutex<Option> so we can take ownership in `generate_stream`
        // even though the trait method takes `&self`.
        script: Mutex<Option<Vec<Result<TokenChunk>>>>,
    }

    impl ScriptedBackend {
        fn new(items: Vec<Result<TokenChunk>>) -> Self {
            Self {
                script: Mutex::new(Some(items)),
            }
        }
    }

    #[async_trait]
    impl InferenceBackend for ScriptedBackend {
        fn name(&self) -> &str {
            "scripted-test"
        }
        async fn is_available(&self) -> bool {
            true
        }
        async fn generate_stream(
            &self,
            _prompt: &Prompt,
            _params: &GenerationParams,
        ) -> Result<TokenStream> {
            let items = self
                .script
                .lock()
                .unwrap()
                .take()
                .expect("ScriptedBackend script already consumed");
            Ok(Box::pin(stream::iter(items)))
        }
    }

    /// Empty knowledge base for tests — never returns any passages.
    struct EmptyKnowledge;
    #[async_trait]
    impl KnowledgeBase for EmptyKnowledge {
        async fn retrieve(&self, _query: &str, _params: &RetrievalParams) -> Result<Vec<Passage>> {
            Ok(vec![])
        }
    }

    /// Session-store spy: counts `save_session` calls and records the turn
    /// count of the most recent save. Lets the dialogue-manager tests prove
    /// the engine actually fired a save (rather than relying on idempotence
    /// of a manual save after the fact).
    struct CountingStore {
        saves: Mutex<u32>,
        last_turn_count: Mutex<usize>,
    }

    impl CountingStore {
        fn new() -> Self {
            Self {
                saves: Mutex::new(0),
                last_turn_count: Mutex::new(0),
            }
        }
        fn save_count(&self) -> u32 {
            *self.saves.lock().unwrap()
        }
        fn last_turn_count(&self) -> usize {
            *self.last_turn_count.lock().unwrap()
        }
    }

    #[async_trait]
    impl primer_core::storage::SessionStore for CountingStore {
        async fn save_session(&self, session: &Session) -> Result<()> {
            *self.saves.lock().unwrap() += 1;
            *self.last_turn_count.lock().unwrap() = session.turns.len();
            Ok(())
        }
    }

    fn test_learner() -> LearnerModel {
        LearnerModel {
            profile: LearnerProfile {
                id: Uuid::new_v4(),
                name: "Tester".to_string(),
                age: 8,
                languages: vec!["en".to_string()],
                created_at: Utc::now(),
                last_active: Utc::now(),
            },
            concepts: vec![],
            preferences: LearningPreferences::default(),
            current_engagement: EngagementState::Engaged,
        }
    }

    fn chunk(text: &str, done: bool) -> TokenChunk {
        TokenChunk {
            text: text.to_string(),
            done,
        }
    }

    #[tokio::test]
    async fn respond_to_streaming_invokes_callback_per_chunk() {
        let backend = ScriptedBackend::new(vec![
            Ok(chunk("Hel", false)),
            Ok(chunk("lo", false)),
            Ok(chunk(" there", false)),
            Ok(chunk("", true)),
        ]);
        let knowledge = EmptyKnowledge;
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            None,
            PedagogyConfig::default(),
        );

        let received: Mutex<Vec<String>> = Mutex::new(vec![]);
        let _ = dm
            .respond_to_streaming("why is the sky blue", |c| {
                received.lock().unwrap().push(c.to_string());
            })
            .await
            .unwrap();

        let joined: String = received.lock().unwrap().join("");
        assert_eq!(joined, "Hello there");
    }

    #[tokio::test]
    async fn respond_to_streaming_returns_full_accumulated_text() {
        let backend = ScriptedBackend::new(vec![
            Ok(chunk("Hel", false)),
            Ok(chunk("lo", false)),
            Ok(chunk(" there", false)),
            Ok(chunk("", true)),
        ]);
        let knowledge = EmptyKnowledge;
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            None,
            PedagogyConfig::default(),
        );

        let result = dm.respond_to_streaming("hi", |_| {}).await.unwrap();
        assert_eq!(result, "Hello there");
    }

    #[tokio::test]
    async fn respond_to_streaming_records_full_primer_turn() {
        let backend = ScriptedBackend::new(vec![
            Ok(chunk("part one ", false)),
            Ok(chunk("part two", false)),
            Ok(chunk("", true)),
        ]);
        let knowledge = EmptyKnowledge;
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            None,
            PedagogyConfig::default(),
        );

        let _ = dm.respond_to_streaming("question", |_| {}).await.unwrap();
        let last = dm.session.turns.last().unwrap();
        assert_eq!(last.speaker, Speaker::Primer);
        assert_eq!(last.text, "part one part two");
    }

    #[tokio::test]
    async fn respond_to_streaming_does_not_record_primer_turn_on_stream_error() {
        let backend = ScriptedBackend::new(vec![
            Ok(chunk("partial", false)),
            Err(PrimerError::Inference("simulated network drop".into())),
        ]);
        let knowledge = EmptyKnowledge;
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            None,
            PedagogyConfig::default(),
        );

        let result = dm.respond_to_streaming("question", |_| {}).await;
        assert!(result.is_err(), "expected Err on mid-stream failure");

        // Child turn should be recorded; Primer turn should NOT be.
        assert_eq!(dm.session.turns.len(), 1);
        assert_eq!(dm.session.turns[0].speaker, Speaker::Child);
    }

    #[tokio::test]
    async fn respond_to_streaming_returns_empty_string_when_stream_yields_no_text() {
        // Backend completes cleanly with only an empty done-chunk. The call
        // should succeed with an empty accumulated string and still record
        // the (empty) Primer turn — the consumer is responsible for noticing
        // and surfacing this as a user-facing problem if they care.
        let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
        let knowledge = EmptyKnowledge;
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            None,
            PedagogyConfig::default(),
        );

        let result = dm.respond_to_streaming("hi", |_| {}).await.unwrap();
        assert_eq!(result, "");
        let last = dm.session.turns.last().unwrap();
        assert_eq!(last.speaker, Speaker::Primer);
        assert_eq!(last.text, "");
    }

    #[tokio::test]
    async fn respond_to_thin_wrapper_still_works() {
        let backend = ScriptedBackend::new(vec![
            Ok(chunk("alpha ", false)),
            Ok(chunk("beta", false)),
            Ok(chunk("", true)),
        ]);
        let knowledge = EmptyKnowledge;
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            None,
            PedagogyConfig::default(),
        );

        let result = dm.respond_to("hi").await.unwrap();
        assert_eq!(result, "alpha beta");
    }

    #[tokio::test]
    async fn respond_to_streaming_fires_engine_save_on_success() {
        use primer_core::storage::SessionStore;

        let backend = ScriptedBackend::new(vec![Ok(chunk("Hi", false)), Ok(chunk("", true))]);
        let knowledge = EmptyKnowledge;
        let store = CountingStore::new();
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            Some(&store as &dyn SessionStore),
            PedagogyConfig::default(),
        );

        let _ = dm.respond_to_streaming("hello", |_| {}).await.unwrap();

        // Engine fired exactly one save. Persisted session has both the
        // child input and the Primer response.
        assert_eq!(store.save_count(), 1);
        assert_eq!(store.last_turn_count(), 2);
    }

    #[tokio::test]
    async fn respond_to_streaming_fires_engine_save_on_stream_error() {
        use primer_core::storage::SessionStore;

        let backend = ScriptedBackend::new(vec![
            Ok(chunk("partial", false)),
            Err(PrimerError::Inference("simulated drop".into())),
        ]);
        let knowledge = EmptyKnowledge;
        let store = CountingStore::new();
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            Some(&store as &dyn SessionStore),
            PedagogyConfig::default(),
        );

        let result = dm.respond_to_streaming("question", |_| {}).await;
        assert!(result.is_err());

        // Engine fired the save even though the stream errored. Persisted
        // session has only the child turn (Primer turn was dropped).
        assert_eq!(store.save_count(), 1);
        assert_eq!(store.last_turn_count(), 1);
        assert_eq!(dm.session.turns.len(), 1);
        assert_eq!(dm.session.turns[0].speaker, Speaker::Child);
    }

    #[tokio::test]
    async fn close_session_fires_engine_save_with_ended_at() {
        use primer_core::storage::SessionStore;

        let backend = ScriptedBackend::new(vec![Ok(chunk("Hi", false)), Ok(chunk("", true))]);
        let knowledge = EmptyKnowledge;
        let store = CountingStore::new();
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            Some(&store as &dyn SessionStore),
            PedagogyConfig::default(),
        );

        let _ = dm.respond_to_streaming("hello", |_| {}).await.unwrap();
        // First save fired during respond_to_streaming.
        let saves_after_response = store.save_count();

        dm.close_session().await;

        // close_session also fires a save, this time with ended_at populated.
        assert_eq!(store.save_count(), saves_after_response + 1);
        assert!(dm.session.ended_at.is_some());
    }

    #[tokio::test]
    async fn open_session_fires_engine_save() {
        use primer_core::storage::SessionStore;

        let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
        let knowledge = EmptyKnowledge;
        let store = CountingStore::new();
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            Some(&store as &dyn SessionStore),
            PedagogyConfig::default(),
        );

        let _ = dm.open_session().await.unwrap();

        // The greeting turn was recorded and persisted.
        assert_eq!(store.save_count(), 1);
        assert_eq!(store.last_turn_count(), 1);
    }
}
