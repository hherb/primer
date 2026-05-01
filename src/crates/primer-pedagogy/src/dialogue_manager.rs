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
//!
//! # Ownership model
//!
//! `inference` and `knowledge` are borrowed references (`&'a dyn …`):
//! they are only used synchronously inside method bodies and need no
//! cross-turn lifetime. By contrast, `storage` and `classifier` are
//! `Arc<dyn …>` because the post-response classifier task (Task 23)
//! will capture them inside a `tokio::spawn` future, which requires
//! `'static` — borrowed references cannot satisfy that bound.

use std::sync::Arc;

use chrono::Utc;
use futures::StreamExt;
use primer_classifier::{ClassifierSettings, EngagementClassifier};
use primer_core::classifier::EngagementAssessment;
use primer_core::config::PedagogyConfig;
use primer_core::conversation::{PedagogicalIntent, Session, Speaker, Turn};
use primer_core::error::{PrimerError, Result};
use primer_core::inference::{GenerationParams, InferenceBackend};
use primer_core::knowledge::{KnowledgeBase, RetrievalParams};
use primer_core::learner::LearnerModel;
use primer_core::storage::SessionStore;
use tokio::task::JoinHandle;

use crate::prompt_builder;

/// The dialogue manager for a single session.
///
/// Holds references to all the subsystems it needs, plus the mutable
/// session and learner model state. The CLI (or future GUI) drives
/// the conversation by calling `respond_to()` in a loop.
///
/// `inference` and `knowledge` are borrowed references: they are used
/// only synchronously inside method bodies. `storage` and `classifier`
/// are `Arc<dyn …>` so they can be captured by the post-response
/// classifier task (`tokio::spawn` requires `'static`).
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
    /// Arc so the classifier task can capture it across turn boundaries.
    storage: Option<Arc<dyn SessionStore>>,
    /// Engagement classifier — called after each Primer response to assess
    /// the child's engagement state. Arc for the same spawn-capture reason.
    #[allow(dead_code)]
    classifier: Arc<dyn EngagementClassifier>,
    /// Tunable parameters for the classifier (thresholds, timeouts, etc.).
    classifier_settings: ClassifierSettings,
    /// Handle to the in-flight classifier task spawned after the previous
    /// turn. `None` when no task is running.
    #[allow(dead_code)]
    classify_task: Option<JoinHandle<Option<EngagementAssessment>>>,
    /// Pedagogical configuration.
    config: PedagogyConfig,
}

/// Push an `EngagementAssessment` into the learner's history buffer and,
/// when confidence is high enough, update `current_engagement`.
///
/// History is a FIFO ring of depth `settings.history_depth`. Every
/// assessment — even low-confidence ones — is recorded so the trajectory
/// is visible to later logic. Only assessments that meet or exceed
/// `settings.confidence_threshold` update `current_engagement`; below
/// that threshold the field is left unchanged so a single noisy read
/// doesn't yank the intent-selection state.
pub(crate) fn apply_assessment(
    learner: &mut primer_core::learner::LearnerModel,
    a: primer_core::classifier::EngagementAssessment,
    settings: &primer_classifier::ClassifierSettings,
) {
    learner.recent_assessments.push(a.clone());
    while learner.recent_assessments.len() > settings.history_depth {
        learner.recent_assessments.remove(0);
    }
    if a.confidence >= settings.confidence_threshold {
        learner.current_engagement = a.state;
    }
    // Low-confidence assessments are still recorded in history (signal for
    // trajectory) but current_engagement stays unchanged.
}

impl<'a> DialogueManager<'a> {
    /// Create a new dialogue manager for a session.
    ///
    /// `storage` and `classifier` are `Arc<dyn …>` so they can be
    /// captured inside the post-response classifier task without
    /// lifetime constraints (`tokio::spawn` requires `'static`).
    pub fn new(
        learner: LearnerModel,
        inference: &'a dyn InferenceBackend,
        knowledge: &'a dyn KnowledgeBase,
        storage: Option<Arc<dyn SessionStore>>,
        classifier: Arc<dyn EngagementClassifier>,
        classifier_settings: ClassifierSettings,
        config: PedagogyConfig,
    ) -> Self {
        let session = Session::new(learner.profile.id);
        Self {
            learner,
            session,
            inference,
            knowledge,
            storage,
            classifier,
            classifier_settings,
            classify_task: None,
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

        if let Some(ref store) = self.storage {
            if let Err(e) = store.save_session(&self.session).await {
                tracing::warn!("session save failed: {e}");
            }
        }

        Ok(greeting)
    }

    /// Pick up an existing session loaded from storage. Replaces
    /// `open_session()` for resumed flows: no greeting is emitted, the
    /// loaded turns are kept in place, and `ended_at` is cleared so
    /// the session is "active again".
    ///
    /// If the loaded session has pre-window content the existing
    /// summary doesn't yet cover, this method refreshes the summary so
    /// the model has long-term memory of the conversation from turn
    /// one. A summary that already covers the current pre-window range
    /// is preserved verbatim — no point burning an LLM call to
    /// regenerate identical work.
    ///
    /// Note: the in-memory `LearnerModel` (built from CLI flags) is
    /// not reconciled with `loaded.learner_id`; they may diverge until
    /// a learner persistence layer lands. The session's `learner_id`
    /// is preserved as loaded.
    pub async fn resume_session(&mut self, loaded: Session) -> Result<()> {
        self.session = loaded;
        self.session.ended_at = None;
        self.refresh_summary_if_stale().await;
        if let Some(ref store) = self.storage {
            if let Err(e) = store.save_session(&self.session).await {
                tracing::warn!("session save failed during resume: {e}");
            }
        }
        Ok(())
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

        // 2. Decide intent, retrieve knowledge, retrieve relevant older
        //    turns from the FTS index (when there are turns outside the
        //    active window), build prompt.
        let intent = prompt_builder::decide_intent(&self.learner, &self.session);
        let knowledge_context = self.retrieve_knowledge(child_input).await;
        let (summary, retrieved_older) = self.retrieve_long_term_memory(child_input).await;
        let prompt = prompt_builder::build_prompt(
            &self.learner,
            &self.session,
            intent,
            &knowledge_context,
            &summary,
            &retrieved_older,
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
            // Refresh the rolling summary if enough turns have fallen
            // out of the window since we last summarized. Best-effort:
            // a summary failure is logged, not propagated.
            self.refresh_summary_if_due().await;
        }

        // 5. Save the session if a store is configured. Runs on both Ok
        //    and Err paths. Save failures are logged, not propagated.
        if let Some(ref store) = self.storage {
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
        if let Some(ref store) = self.storage {
            if let Err(e) = store.save_session(&self.session).await {
                tracing::warn!("session save failed during close: {e}");
            }
        }
    }

    // ─── Classifier helpers ───────────────────────────────────────────

    /// Wait (up to `blocking_timeout`) for the classifier task spawned after
    /// the previous turn, then apply its result to `learner`.
    ///
    /// Called at the start of each new turn so the prior turn's assessment
    /// is consumed before intent is decided. On timeout the task is aborted
    /// and we proceed with the existing (stale) engagement state — better
    /// than blocking the conversation indefinitely.
    async fn await_pending_classification(
        &mut self,
        learner: &mut primer_core::learner::LearnerModel,
    ) {
        let Some(task) = self.classify_task.take() else {
            return;
        };
        let abort = task.abort_handle();
        let timeout = self.classifier_settings.blocking_timeout;
        match tokio::time::timeout(timeout, task).await {
            Ok(Ok(Some(assessment))) => {
                apply_assessment(learner, assessment, &self.classifier_settings)
            }
            Ok(Ok(None)) => { /* soft failure; nothing to apply */ }
            Ok(Err(e)) => tracing::warn!(error = ?e, "classifier task panicked"),
            Err(_) => {
                abort.abort();
                tracing::debug!(
                    "classifier exceeded blocking timeout — proceeding with stale engagement state"
                );
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

    /// Pull long-term memory for the current turn: the rolling summary
    /// of pre-window turns plus the top-K older turns that the FTS index
    /// considers relevant to `child_input`.
    ///
    /// Both pieces are empty when the session is still inside its first
    /// context window, when no store is configured, or when the FTS
    /// index returns no matches. Errors from the store are logged and
    /// treated as "no retrieved turns" — long-term memory is best-effort.
    async fn retrieve_long_term_memory(&self, child_input: &str) -> (String, Vec<Turn>) {
        let total = self.session.turns.len();
        let window = self.config.context_window_turns;
        if total <= window {
            return (String::new(), vec![]);
        }
        let exclude_at_or_after = total - window;
        let retrieved = match self.storage.as_deref() {
            None => vec![],
            Some(store) => store
                .retrieve_session_turns(self.session.id, child_input, 3, exclude_at_or_after)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("session-turn retrieval failed: {e}");
                    vec![]
                }),
        };
        (self.session.summary.clone(), retrieved)
    }

    /// Active-conversation cadence. Refresh the rolling summary when at
    /// least `context_window_turns` turns have fallen out of the window
    /// since `summary_through_turn_index` was last set, so per-turn
    /// dialogue doesn't trigger an LLM call every time the boundary
    /// advances. At the default K=20, a summary is built each time 20
    /// new turns have rolled past the boundary.
    async fn refresh_summary_if_due(&mut self) {
        let window = self.config.context_window_turns;
        let total = self.session.turns.len();
        if total <= window {
            return;
        }
        let pre_window_end = total - window;
        let already_covered = self.session.summary_through_turn_index;
        if pre_window_end < already_covered.saturating_add(window) {
            return;
        }
        self.regenerate_summary_through(pre_window_end).await;
    }

    /// Resume cadence. Refresh the rolling summary when the loaded
    /// session has pre-window content the existing summary doesn't
    /// yet cover. A summary that's already current is preserved
    /// verbatim — there is no value in regenerating identical work.
    async fn refresh_summary_if_stale(&mut self) {
        let window = self.config.context_window_turns;
        let total = self.session.turns.len();
        if total <= window {
            return;
        }
        let pre_window_end = total - window;
        if self.session.summary_through_turn_index >= pre_window_end {
            return;
        }
        self.regenerate_summary_through(pre_window_end).await;
    }

    /// Common body: re-summarize `turns[..pre_window_end]` from scratch
    /// and stamp the new boundary. Replacing rather than incrementally
    /// extending keeps the summary coherent; the simplicity is fine at
    /// Phase-0 cost. Best-effort: a summary failure is logged and the
    /// previous state stays in place.
    async fn regenerate_summary_through(&mut self, pre_window_end: usize) {
        let to_summarize = &self.session.turns[..pre_window_end];
        match self.inference.summarize(to_summarize, 1500).await {
            Ok(summary) => {
                self.session.summary = summary;
                self.session.summary_through_turn_index = pre_window_end;
            }
            Err(e) => tracing::warn!("summary refresh failed: {e}"),
        }
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
    use primer_classifier::StubEngagementClassifier;
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

    fn stub_classifier() -> Arc<dyn EngagementClassifier> {
        Arc::new(StubEngagementClassifier::new())
    }

    /// Test inference backend that emits a pre-configured sequence of stream items.
    struct ScriptedBackend {
        // Wrap in Mutex<Option> so we can take ownership in `generate_stream`
        // even though the trait method takes `&self`.
        script: Mutex<Option<Vec<Result<TokenChunk>>>>,
        // Counts calls to `summarize` for tests that assert on cadence.
        summarize_calls: Mutex<u32>,
    }

    impl ScriptedBackend {
        fn new(items: Vec<Result<TokenChunk>>) -> Self {
            Self {
                script: Mutex::new(Some(items)),
                summarize_calls: Mutex::new(0),
            }
        }
        fn summary_call_count(&self) -> u32 {
            *self.summarize_calls.lock().unwrap()
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
        async fn summarize(&self, turns: &[Turn], _target_chars: usize) -> Result<String> {
            *self.summarize_calls.lock().unwrap() += 1;
            Ok(format!("[test summary covering {} turns]", turns.len()))
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
        async fn load_session(&self, _id: uuid::Uuid) -> Result<Option<Session>> {
            // Stub: tests that need real load behaviour use a different store.
            Ok(None)
        }
        async fn retrieve_session_turns(
            &self,
            _session_id: uuid::Uuid,
            _query: &str,
            _k: usize,
            _exclude_indices_at_or_after: usize,
        ) -> Result<Vec<Turn>> {
            Ok(vec![])
        }

        async fn save_classification(
            &self,
            _session_id: primer_core::conversation::SessionId,
            _turn_index: usize,
            _assessment: &primer_core::classifier::EngagementAssessment,
            _classifier_identifier: &str,
        ) -> Result<()> {
            Ok(())
        }

        async fn load_recent_assessments(
            &self,
            _session_id: primer_core::conversation::SessionId,
            _classifier_identifier: &str,
            _k: usize,
        ) -> Result<Vec<primer_core::classifier::EngagementAssessment>> {
            Ok(vec![])
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
            recent_assessments: vec![],
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
            stub_classifier(),
            ClassifierSettings::default(),
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
            stub_classifier(),
            ClassifierSettings::default(),
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
            stub_classifier(),
            ClassifierSettings::default(),
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
            stub_classifier(),
            ClassifierSettings::default(),
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
            stub_classifier(),
            ClassifierSettings::default(),
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
            stub_classifier(),
            ClassifierSettings::default(),
            PedagogyConfig::default(),
        );

        let result = dm.respond_to("hi").await.unwrap();
        assert_eq!(result, "alpha beta");
    }

    #[tokio::test]
    async fn respond_to_streaming_fires_engine_save_on_success() {
        let backend = ScriptedBackend::new(vec![Ok(chunk("Hi", false)), Ok(chunk("", true))]);
        let knowledge = EmptyKnowledge;
        let store = Arc::new(CountingStore::new());
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            Some(Arc::clone(&store) as Arc<dyn SessionStore>),
            stub_classifier(),
            ClassifierSettings::default(),
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
        let backend = ScriptedBackend::new(vec![
            Ok(chunk("partial", false)),
            Err(PrimerError::Inference("simulated drop".into())),
        ]);
        let knowledge = EmptyKnowledge;
        let store = Arc::new(CountingStore::new());
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            Some(Arc::clone(&store) as Arc<dyn SessionStore>),
            stub_classifier(),
            ClassifierSettings::default(),
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
        let backend = ScriptedBackend::new(vec![Ok(chunk("Hi", false)), Ok(chunk("", true))]);
        let knowledge = EmptyKnowledge;
        let store = Arc::new(CountingStore::new());
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            Some(Arc::clone(&store) as Arc<dyn SessionStore>),
            stub_classifier(),
            ClassifierSettings::default(),
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
        let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
        let knowledge = EmptyKnowledge;
        let store = Arc::new(CountingStore::new());
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            Some(Arc::clone(&store) as Arc<dyn SessionStore>),
            stub_classifier(),
            ClassifierSettings::default(),
            PedagogyConfig::default(),
        );

        let _ = dm.open_session().await.unwrap();

        // The greeting turn was recorded and persisted.
        assert_eq!(store.save_count(), 1);
        assert_eq!(store.last_turn_count(), 1);
    }

    // ─── resume_session and summary refresh ──────────────────────────

    fn make_test_session_with_turns(n: usize, learner_id: Uuid) -> Session {
        use primer_core::conversation::Speaker;
        let mut session = Session::new(learner_id);
        for i in 0..n {
            session.add_turn(Turn {
                speaker: if i % 2 == 0 {
                    Speaker::Child
                } else {
                    Speaker::Primer
                },
                text: format!("turn {i}"),
                timestamp: Utc::now(),
                intent: None,
                concepts: vec![],
            });
        }
        session
    }

    #[tokio::test]
    async fn resume_session_loads_turns_without_greeting() {
        // Resume picks up the loaded turns verbatim. No greeting is
        // prepended; the turn count after resume_session matches the
        // loaded session exactly.
        let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
        let knowledge = EmptyKnowledge;
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            None,
            stub_classifier(),
            ClassifierSettings::default(),
            PedagogyConfig::default(),
        );
        let learner_id = dm.learner.profile.id;
        let loaded = make_test_session_with_turns(5, learner_id);
        let loaded_id = loaded.id;
        dm.resume_session(loaded).await.unwrap();
        assert_eq!(dm.session.turns.len(), 5);
        assert_eq!(dm.session.id, loaded_id);
        // The Primer never said "Hello, ..." — turn 0 is from our test fixture.
        assert_eq!(dm.session.turns[0].text, "turn 0");
    }

    #[tokio::test]
    async fn resume_session_clears_ended_at_and_persists() {
        let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
        let knowledge = EmptyKnowledge;
        let store = Arc::new(CountingStore::new());
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            Some(Arc::clone(&store) as Arc<dyn SessionStore>),
            stub_classifier(),
            ClassifierSettings::default(),
            PedagogyConfig::default(),
        );
        let mut loaded = make_test_session_with_turns(3, dm.learner.profile.id);
        loaded.ended_at = Some(Utc::now());
        dm.resume_session(loaded).await.unwrap();
        assert!(dm.session.ended_at.is_none(), "ended_at should be cleared");
        assert_eq!(store.save_count(), 1, "resume should fire one save");
    }

    #[tokio::test]
    async fn resume_session_preserves_loaded_learner_id() {
        // The in-memory LearnerModel comes from CLI flags; the loaded
        // Session might belong to a different learner_id. Resume must
        // keep the loaded learner_id (no silent override).
        let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
        let knowledge = EmptyKnowledge;
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            None,
            stub_classifier(),
            ClassifierSettings::default(),
            PedagogyConfig::default(),
        );
        let dm_learner_id = dm.learner.profile.id;
        let other_learner = Uuid::new_v4();
        assert_ne!(dm_learner_id, other_learner);
        let loaded = make_test_session_with_turns(2, other_learner);
        dm.resume_session(loaded).await.unwrap();
        assert_eq!(
            dm.session.learner_id, other_learner,
            "session learner_id should not be overwritten by the manager's learner"
        );
    }

    #[tokio::test]
    async fn resume_session_triggers_summary_refresh_when_above_window() {
        // A loaded session with > context_window_turns should get its
        // summary refreshed unconditionally on resume so the Primer has
        // long-term memory of pre-window turns from turn one.
        let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
        let knowledge = EmptyKnowledge;
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            None,
            stub_classifier(),
            ClassifierSettings::default(),
            PedagogyConfig::default(),
        );
        // Window is 20; 25 turns gives 5 pre-window turns.
        let loaded = make_test_session_with_turns(25, dm.learner.profile.id);
        dm.resume_session(loaded).await.unwrap();
        assert_eq!(
            backend.summary_call_count(),
            1,
            "summary should refresh on resume"
        );
        assert!(
            !dm.session.summary.is_empty(),
            "summary should be populated after refresh"
        );
        assert_eq!(
            dm.session.summary_through_turn_index, 5,
            "summary boundary should land at total - window"
        );
    }

    #[tokio::test]
    async fn resume_session_skips_summary_when_inside_first_window() {
        // Sessions that fit inside the active window have nothing to
        // summarize; resume must not waste an inference call.
        let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
        let knowledge = EmptyKnowledge;
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            None,
            stub_classifier(),
            ClassifierSettings::default(),
            PedagogyConfig::default(),
        );
        // Window is 20; 5 turns is well inside.
        let loaded = make_test_session_with_turns(5, dm.learner.profile.id);
        dm.resume_session(loaded).await.unwrap();
        assert_eq!(backend.summary_call_count(), 0);
        assert_eq!(dm.session.summary, "");
    }

    #[tokio::test]
    async fn resume_session_skips_refresh_when_summary_already_current() {
        // Loaded session has 25 turns and a summary that already covers
        // turns[..5] — exactly the pre-window range. There is no new
        // pre-window content for the summary to absorb, so resume must
        // not burn an LLM call regenerating identical work.
        let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
        let knowledge = EmptyKnowledge;
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            None,
            stub_classifier(),
            ClassifierSettings::default(),
            PedagogyConfig::default(),
        );
        let mut loaded = make_test_session_with_turns(25, dm.learner.profile.id);
        loaded.summary = "Pre-existing summary covering turns 0..5.".to_string();
        loaded.summary_through_turn_index = 5;
        dm.resume_session(loaded).await.unwrap();
        assert_eq!(
            backend.summary_call_count(),
            0,
            "summary already covers the pre-window range; resume must not regenerate"
        );
        assert_eq!(
            dm.session.summary, "Pre-existing summary covering turns 0..5.",
            "existing summary must be preserved verbatim"
        );
    }

    #[tokio::test]
    async fn resume_session_refreshes_when_existing_summary_is_stale() {
        // Loaded session has 30 turns and a summary that only covers
        // turns[..3]. The current pre-window range is turns[..10], so
        // there are 7 pre-window turns the summary doesn't yet know
        // about. Resume must refresh.
        let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
        let knowledge = EmptyKnowledge;
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            None,
            stub_classifier(),
            ClassifierSettings::default(),
            PedagogyConfig::default(),
        );
        let mut loaded = make_test_session_with_turns(30, dm.learner.profile.id);
        loaded.summary = "Stale summary covering only turns 0..3.".to_string();
        loaded.summary_through_turn_index = 3;
        dm.resume_session(loaded).await.unwrap();
        assert_eq!(backend.summary_call_count(), 1);
        assert_eq!(dm.session.summary_through_turn_index, 10);
    }

    #[tokio::test]
    async fn summary_does_not_refresh_when_below_threshold_during_active_session() {
        // First respond_to_streaming fires only when there are turns to
        // process. With turn count below window+window, no refresh.
        let backend = ScriptedBackend::new(vec![Ok(chunk("ok", false)), Ok(chunk("", true))]);
        let knowledge = EmptyKnowledge;
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            None,
            stub_classifier(),
            ClassifierSettings::default(),
            PedagogyConfig::default(),
        );
        // Pre-load with 21 turns (1 turn pre-window). Far below the
        // 2*window threshold.
        dm.session.turns = make_test_session_with_turns(21, dm.learner.profile.id).turns;
        let _ = dm.respond_to_streaming("hi", |_| {}).await.unwrap();
        assert_eq!(backend.summary_call_count(), 0);
    }

    // ─── apply_assessment ─────────────────────────────────────────────

    #[test]
    fn apply_assessment_pushes_to_recent_assessments() {
        let mut learner = test_learner();
        let settings = ClassifierSettings::default();
        let a = primer_core::classifier::EngagementAssessment {
            state: EngagementState::Reflecting,
            confidence: 0.9,
            reasoning: None,
        };
        apply_assessment(&mut learner, a.clone(), &settings);
        assert_eq!(learner.recent_assessments.len(), 1);
        assert_eq!(learner.recent_assessments[0].state, EngagementState::Reflecting);
    }

    #[test]
    fn apply_assessment_evicts_oldest_when_buffer_full() {
        let mut learner = test_learner();
        let settings = ClassifierSettings {
            history_depth: 2,
            ..Default::default()
        };
        for state in [
            EngagementState::Engaged,
            EngagementState::Reflecting,
            EngagementState::FrustratedStuck,
        ] {
            apply_assessment(
                &mut learner,
                primer_core::classifier::EngagementAssessment {
                    state,
                    confidence: 0.9,
                    reasoning: None,
                },
                &settings,
            );
        }
        assert_eq!(learner.recent_assessments.len(), 2);
        assert_eq!(learner.recent_assessments[0].state, EngagementState::Reflecting);
        assert_eq!(learner.recent_assessments[1].state, EngagementState::FrustratedStuck);
    }

    #[test]
    fn apply_assessment_updates_current_engagement_when_confident() {
        let mut learner = test_learner();
        let settings = ClassifierSettings {
            confidence_threshold: 0.6,
            ..Default::default()
        };
        apply_assessment(
            &mut learner,
            primer_core::classifier::EngagementAssessment {
                state: EngagementState::FrustratedTrying,
                confidence: 0.8,
                reasoning: None,
            },
            &settings,
        );
        assert_eq!(learner.current_engagement, EngagementState::FrustratedTrying);
    }

    #[test]
    fn apply_assessment_keeps_current_engagement_when_low_confidence() {
        let mut learner = test_learner();
        let initial = learner.current_engagement;
        let settings = ClassifierSettings {
            confidence_threshold: 0.6,
            ..Default::default()
        };
        apply_assessment(
            &mut learner,
            primer_core::classifier::EngagementAssessment {
                state: EngagementState::FrustratedTrying,
                confidence: 0.3,
                reasoning: None,
            },
            &settings,
        );
        assert_eq!(
            learner.current_engagement, initial,
            "low-confidence assessment must NOT change current_engagement"
        );
        assert_eq!(
            learner.recent_assessments.len(),
            1,
            "low-confidence assessment IS still recorded in history"
        );
    }
}
