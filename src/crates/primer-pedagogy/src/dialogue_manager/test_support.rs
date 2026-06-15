//! Shared test infrastructure for the dialogue_manager module.
//!
//! Houses the mocks and helper builders that almost every test in this
//! module reaches for — counting / spy implementations of the storage
//! traits, a scripted inference backend, an empty knowledge base, and
//! the `test_learner` / `chunk` / `make_test_session_with_turns`
//! builders. Per-test mocks (e.g. `RepeatingBackend`, `FailingLearnerStore`,
//! `ConceptCapturingStore`) stay alongside the tests that need them.
//!
//! Visibility is `pub(super)` so the dialogue manager's `mod tests`
//! can use these via `super::test_support::*`. Nothing in this file
//! is reachable outside `crate::dialogue_manager`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Utc;
use futures::stream;
use primer_classifier::{ClassifierSettings, EngagementClassifier, StubEngagementClassifier};
use primer_comprehension::{ComprehensionClassifier, StubComprehensionClassifier};
use primer_core::conversation::{Session, Speaker, Turn};
use primer_core::error::Result;
use primer_core::inference::{GenerationParams, InferenceBackend, Prompt, TokenChunk, TokenStream};
use primer_core::knowledge::{KnowledgeBase, Passage, RetrievalParams};
use primer_core::learner::{EngagementState, LearnerModel, LearnerProfile, LearningPreferences};
use primer_extractor::{ConceptExtractor, ExtractorSettings};
use uuid::Uuid;

use super::DialogueManagerSubsystems;

// ─── Stub builders ───────────────────────────────────────────────────

pub(super) fn stub_classifier() -> Arc<dyn EngagementClassifier> {
    Arc::new(StubEngagementClassifier::new())
}

pub(super) fn stub_extractor() -> Arc<dyn ConceptExtractor> {
    Arc::new(primer_extractor::StubConceptExtractor::new())
}

pub(super) fn stub_comprehension() -> Arc<dyn ComprehensionClassifier> {
    Arc::new(StubComprehensionClassifier::new())
}

/// Default-everything subsystems bundle for tests that don't care
/// about the specifics of the classifier/extractor/comprehension.
pub(super) fn default_subsystems() -> DialogueManagerSubsystems {
    DialogueManagerSubsystems {
        classifier: stub_classifier(),
        classifier_settings: ClassifierSettings::default(),
        extractor: stub_extractor(),
        extractor_settings: ExtractorSettings::default(),
        comprehension: stub_comprehension(),
        comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
        vocab_settings: crate::vocab::VocabSettings::default(),
        embedder: None,
    }
}

/// Subsystems bundle for tests that need a specific extractor
/// (e.g. scripted concepts) but otherwise default classifier/settings.
pub(super) fn subsystems_with_extractor(
    extractor: Arc<dyn ConceptExtractor>,
) -> DialogueManagerSubsystems {
    DialogueManagerSubsystems {
        classifier: stub_classifier(),
        classifier_settings: ClassifierSettings::default(),
        extractor,
        extractor_settings: ExtractorSettings::default(),
        comprehension: stub_comprehension(),
        comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
        vocab_settings: crate::vocab::VocabSettings::default(),
        embedder: None,
    }
}

/// Subsystems bundle for tests that need a specific comprehension
/// classifier but otherwise default classifier/extractor/settings.
#[allow(dead_code)]
pub(super) fn subsystems_with_comprehension(
    comprehension: Arc<dyn ComprehensionClassifier>,
) -> DialogueManagerSubsystems {
    DialogueManagerSubsystems {
        classifier: stub_classifier(),
        classifier_settings: ClassifierSettings::default(),
        extractor: stub_extractor(),
        extractor_settings: ExtractorSettings::default(),
        comprehension,
        comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
        vocab_settings: crate::vocab::VocabSettings::default(),
        embedder: None,
    }
}

// ─── ScriptedBackend ─────────────────────────────────────────────────

/// Test inference backend that emits a pre-configured sequence of stream items.
pub(super) struct ScriptedBackend {
    // Backend `name()`. Defaults to "scripted-test"; the per-backend
    // context-budget tests override it (e.g. "qnn:test-model") to exercise
    // `is_small_context_backend` detection through the dialogue manager.
    name: String,
    // Wrap in Mutex<Option> so we can take ownership in `generate_stream`
    // even though the trait method takes `&self`.
    script: Mutex<Option<Vec<Result<TokenChunk>>>>,
    // Counts calls to `summarize` for tests that assert on cadence.
    summarize_calls: Mutex<u32>,
}

impl ScriptedBackend {
    pub(super) fn new(items: Vec<Result<TokenChunk>>) -> Self {
        Self {
            name: "scripted-test".to_string(),
            script: Mutex::new(Some(items)),
            summarize_calls: Mutex::new(0),
        }
    }
    /// Override the reported `name()`. Used by the per-backend
    /// context-budget tests to simulate a small-context backend
    /// (`"qnn:…"`).
    pub(super) fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
    pub(super) fn summary_call_count(&self) -> u32 {
        *self.summarize_calls.lock().unwrap()
    }
    pub(super) fn set_script(&self, items: Vec<Result<TokenChunk>>) {
        *self.script.lock().unwrap() = Some(items);
    }
}

#[async_trait]
impl InferenceBackend for ScriptedBackend {
    fn name(&self) -> &str {
        &self.name
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

// ─── EmptyKnowledge ──────────────────────────────────────────────────

/// Empty knowledge base for tests — never returns any passages.
pub(super) struct EmptyKnowledge;

#[async_trait]
impl KnowledgeBase for EmptyKnowledge {
    async fn retrieve(&self, _query: &str, _params: &RetrievalParams) -> Result<Vec<Passage>> {
        Ok(vec![])
    }
}

// ─── BigPassageKnowledge ─────────────────────────────────────────────

/// Knowledge base that always returns `count` long passages (each body is
/// `words` repetitions of a distinctive marker), so a test can prove the
/// dialogue manager truncates and budgets them on a small-context backend.
pub(super) struct BigPassageKnowledge {
    count: usize,
    words: usize,
}

impl BigPassageKnowledge {
    pub(super) fn new(count: usize, words: usize) -> Self {
        Self { count, words }
    }
    /// The repeated marker token used in every passage body.
    pub(super) const MARKER: &'static str = "KNOWLEDGEMARKER";
}

#[async_trait]
impl KnowledgeBase for BigPassageKnowledge {
    async fn retrieve(&self, _query: &str, params: &RetrievalParams) -> Result<Vec<Passage>> {
        let body = vec![Self::MARKER; self.words].join(" ");
        Ok((0..self.count.min(params.top_k))
            .map(|i| Passage {
                id: format!("kb:big:{i}"),
                source: format!("wiki:big:{i}"),
                text: body.clone(),
                score: 1.0 - (i as f64) * 0.01,
            })
            .collect())
    }
}

// ─── TopKRecordingKnowledge ──────────────────────────────────────────

/// Knowledge base that records the `top_k` of the most recent BM25-only
/// `retrieve` call so a test can prove the dialogue manager applied the
/// per-backend KB budget. Returns no passages — only the requested
/// `top_k` matters here.
pub(super) struct TopKRecordingKnowledge {
    last_top_k: Mutex<Option<usize>>,
}

impl TopKRecordingKnowledge {
    pub(super) fn new() -> Self {
        Self {
            last_top_k: Mutex::new(None),
        }
    }
    pub(super) fn last_top_k(&self) -> Option<usize> {
        *self.last_top_k.lock().unwrap()
    }
}

#[async_trait]
impl KnowledgeBase for TopKRecordingKnowledge {
    async fn retrieve(&self, _query: &str, params: &RetrievalParams) -> Result<Vec<Passage>> {
        *self.last_top_k.lock().unwrap() = Some(params.top_k);
        Ok(vec![])
    }
}

// ─── CountingStore ───────────────────────────────────────────────────

/// Session-store spy: counts `save_session` calls and records the turn
/// count of the most recent save. Lets the dialogue-manager tests prove
/// the engine actually fired a save (rather than relying on idempotence
/// of a manual save after the fact).
pub(super) struct CountingStore {
    saves: Mutex<u32>,
    last_turn_count: Mutex<usize>,
}

impl CountingStore {
    pub(super) fn new() -> Self {
        Self {
            saves: Mutex::new(0),
            last_turn_count: Mutex::new(0),
        }
    }
    pub(super) fn save_count(&self) -> u32 {
        *self.saves.lock().unwrap()
    }
    pub(super) fn last_turn_count(&self) -> usize {
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

    async fn most_recent_session_learner_id(&self) -> Result<Option<uuid::Uuid>> {
        Ok(None)
    }

    async fn list_sessions(&self) -> Result<Vec<primer_core::conversation::SessionListing>> {
        Ok(vec![])
    }

    async fn update_turn_concepts(
        &self,
        _session_id: primer_core::conversation::SessionId,
        _turn_index: usize,
        _concepts: &[String],
    ) -> Result<()> {
        Ok(())
    }

    async fn update_exchange_concepts(
        &self,
        _session_id: primer_core::conversation::SessionId,
        _child_turn_index: usize,
        _child_concepts: &[String],
        _primer_turn_index: usize,
        _primer_concepts: &[String],
    ) -> Result<()> {
        Ok(())
    }

    async fn save_comprehensions(
        &self,
        _session_id: primer_core::conversation::SessionId,
        _primer_turn_index: usize,
        _assessments: &[primer_core::comprehension::ComprehensionAssessment],
        _classifier_identifier: &str,
    ) -> Result<()> {
        Ok(())
    }
}

// ─── CountingLearnerStore ────────────────────────────────────────────

/// Learner-store spy: counts `save_learner` calls. Used to prove that
/// the per-turn save site fires (or doesn't) per the dirty-flag policy.
pub(super) struct CountingLearnerStore {
    saves: Mutex<u32>,
}

impl CountingLearnerStore {
    pub(super) fn new() -> Self {
        Self {
            saves: Mutex::new(0),
        }
    }
    pub(super) fn save_count(&self) -> u32 {
        *self.saves.lock().unwrap()
    }
}

#[async_trait]
impl primer_core::storage::LearnerStore for CountingLearnerStore {
    async fn save_learner(&self, _learner: &LearnerModel) -> Result<()> {
        *self.saves.lock().unwrap() += 1;
        Ok(())
    }
    async fn load_learner(&self) -> Result<Option<LearnerModel>> {
        Ok(None)
    }
}

// ─── Builders ────────────────────────────────────────────────────────

pub(super) fn test_learner() -> LearnerModel {
    LearnerModel {
        profile: LearnerProfile {
            id: Uuid::new_v4(),
            name: "Tester".to_string(),
            age: 8,
            languages: vec!["en".to_string()],
            locale: primer_core::i18n::Locale::English,
            created_at: Utc::now(),
            last_active: Utc::now(),
        },
        concepts: vec![],
        preferences: LearningPreferences::default(),
        current_engagement: EngagementState::Engaged,
        recent_assessments: vec![],
    }
}

pub(super) fn chunk(text: &str, done: bool) -> TokenChunk {
    TokenChunk {
        text: text.to_string(),
        done,
        ..Default::default()
    }
}

pub(super) fn make_test_session_with_turns(n: usize, learner_id: Uuid) -> Session {
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

/// Build a `(LearnerModel, Arc<CountingLearnerStore>)` pair for the
/// per-turn save dirty-flag tests. Isolated here so multiple test
/// files can drive the same setup.
pub(super) fn dirty_flag_test_setup(
    starting: EngagementState,
) -> (LearnerModel, Arc<CountingLearnerStore>) {
    let mut learner = test_learner();
    learner.current_engagement = starting;
    let store = Arc::new(CountingLearnerStore::new());
    (learner, store)
}

// ─── RepeatingBackend ────────────────────────────────────────────────

/// Backend that serves the same single-chunk response on every
/// `generate_stream` call. Used by multi-turn tests where the exact
/// content of the Primer response does not matter.
pub(super) struct RepeatingBackend;

#[async_trait]
impl InferenceBackend for RepeatingBackend {
    fn name(&self) -> &str {
        "repeating-test"
    }
    async fn is_available(&self) -> bool {
        true
    }
    async fn generate_stream(
        &self,
        _prompt: &Prompt,
        _params: &GenerationParams,
    ) -> Result<TokenStream> {
        let items: Vec<Result<TokenChunk>> = vec![Ok(chunk("ok.", false)), Ok(chunk("", true))];
        Ok(Box::pin(stream::iter(items)))
    }
    async fn summarize(&self, turns: &[Turn], _target_chars: usize) -> Result<String> {
        Ok(format!(
            "[repeating-backend summary covering {} turns]",
            turns.len()
        ))
    }
}

// ─── SequenceBackend ─────────────────────────────────────────────────

/// Test inference backend returning a DIFFERENT scripted stream per
/// `generate_stream` call — for retry-loop tests that invoke the backend
/// multiple times.
pub(super) struct SequenceBackend {
    name: String,
    scripts: Mutex<VecDeque<Vec<Result<TokenChunk>>>>,
}

impl SequenceBackend {
    pub(super) fn new(scripts: Vec<Vec<Result<TokenChunk>>>) -> Self {
        Self {
            name: "scripted-test".to_string(),
            scripts: Mutex::new(scripts.into()),
        }
    }

    /// Number of scripts not yet consumed by a `generate_stream` call.
    /// Retry-loop tests assert this is `0` to prove the expected number
    /// of inference attempts ran (catches a loop that stops early and
    /// leaves a script unused).
    pub(super) fn remaining(&self) -> usize {
        self.scripts.lock().unwrap().len()
    }
}

#[async_trait]
impl InferenceBackend for SequenceBackend {
    fn name(&self) -> &str {
        &self.name
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
            .scripts
            .lock()
            .unwrap()
            .pop_front()
            .expect("SequenceBackend ran out of scripts");
        Ok(Box::pin(stream::iter(items)))
    }
    async fn summarize(&self, turns: &[Turn], _target_chars: usize) -> Result<String> {
        Ok(format!("[test summary covering {} turns]", turns.len()))
    }
}

/// Terminal stream chunk carrying an explicit `FinishReason` (the `chunk`
/// helper always produces `Stop`). Used by context-limit recovery tests.
pub(super) fn finished(finish_reason: primer_core::inference::FinishReason) -> TokenChunk {
    TokenChunk {
        text: String::new(),
        done: true,
        finish_reason,
    }
}

// ─── ConceptCapturingStore ───────────────────────────────────────────

/// Session-store spy that captures `update_turn_concepts`,
/// `update_exchange_concepts`, and `save_comprehensions` calls so
/// extractor / comprehension tests can assert on what landed.
pub(super) struct ConceptCapturingStore {
    inner: CountingStore,
    captures: Mutex<Vec<(usize, Vec<String>)>>,
    comprehensions: Mutex<
        Vec<(
            usize,
            Vec<primer_core::comprehension::ComprehensionAssessment>,
            String,
        )>,
    >,
}

impl ConceptCapturingStore {
    pub(super) fn new() -> Self {
        Self {
            inner: CountingStore::new(),
            captures: Mutex::new(vec![]),
            comprehensions: Mutex::new(vec![]),
        }
    }
    pub(super) fn captured(&self) -> Vec<(usize, Vec<String>)> {
        self.captures.lock().unwrap().clone()
    }
    pub(super) fn captured_comprehensions(
        &self,
    ) -> Vec<(
        usize,
        Vec<primer_core::comprehension::ComprehensionAssessment>,
        String,
    )> {
        self.comprehensions.lock().unwrap().clone()
    }
}

#[async_trait]
impl primer_core::storage::SessionStore for ConceptCapturingStore {
    async fn save_session(&self, session: &Session) -> Result<()> {
        self.inner.save_session(session).await
    }
    async fn load_session(&self, id: uuid::Uuid) -> Result<Option<Session>> {
        self.inner.load_session(id).await
    }
    async fn retrieve_session_turns(
        &self,
        session_id: uuid::Uuid,
        query: &str,
        k: usize,
        exclude_indices_at_or_after: usize,
    ) -> Result<Vec<Turn>> {
        self.inner
            .retrieve_session_turns(session_id, query, k, exclude_indices_at_or_after)
            .await
    }
    async fn save_classification(
        &self,
        session_id: primer_core::conversation::SessionId,
        turn_index: usize,
        assessment: &primer_core::classifier::EngagementAssessment,
        classifier_identifier: &str,
    ) -> Result<()> {
        self.inner
            .save_classification(session_id, turn_index, assessment, classifier_identifier)
            .await
    }
    async fn load_recent_assessments(
        &self,
        session_id: primer_core::conversation::SessionId,
        classifier_identifier: &str,
        k: usize,
    ) -> Result<Vec<primer_core::classifier::EngagementAssessment>> {
        self.inner
            .load_recent_assessments(session_id, classifier_identifier, k)
            .await
    }
    async fn most_recent_session_learner_id(&self) -> Result<Option<uuid::Uuid>> {
        self.inner.most_recent_session_learner_id().await
    }
    async fn list_sessions(&self) -> Result<Vec<primer_core::conversation::SessionListing>> {
        self.inner.list_sessions().await
    }
    async fn update_turn_concepts(
        &self,
        _session_id: primer_core::conversation::SessionId,
        turn_index: usize,
        concepts: &[String],
    ) -> Result<()> {
        self.captures
            .lock()
            .unwrap()
            .push((turn_index, concepts.to_vec()));
        Ok(())
    }

    async fn update_exchange_concepts(
        &self,
        _session_id: primer_core::conversation::SessionId,
        child_turn_index: usize,
        child_concepts: &[String],
        primer_turn_index: usize,
        primer_concepts: &[String],
    ) -> Result<()> {
        // Mirror the storage impl: push only the speaker(s) that
        // actually have concepts, so tests that scripted an
        // empty-on-one-side extraction don't see a phantom capture.
        let mut captures = self.captures.lock().unwrap();
        if !child_concepts.is_empty() {
            captures.push((child_turn_index, child_concepts.to_vec()));
        }
        if !primer_concepts.is_empty() {
            captures.push((primer_turn_index, primer_concepts.to_vec()));
        }
        Ok(())
    }

    async fn save_comprehensions(
        &self,
        _session_id: primer_core::conversation::SessionId,
        primer_turn_index: usize,
        assessments: &[primer_core::comprehension::ComprehensionAssessment],
        classifier_identifier: &str,
    ) -> Result<()> {
        self.comprehensions.lock().unwrap().push((
            primer_turn_index,
            assessments.to_vec(),
            classifier_identifier.to_string(),
        ));
        Ok(())
    }
}
