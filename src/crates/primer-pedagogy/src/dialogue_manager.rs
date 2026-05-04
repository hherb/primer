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
use primer_core::error::Result;
use primer_core::extractor::ConceptExtraction;
use primer_core::inference::{GenerationParams, InferenceBackend};
use primer_core::knowledge::{KnowledgeBase, RetrievalParams};
use primer_core::learner::LearnerModel;
use primer_core::storage::{LearnerStore, SessionStore};
use primer_extractor::{ConceptExtractor, ExtractorSettings};
use tokio::task::JoinHandle;

use crate::prompt_builder;
use crate::prompt_pack::{self, PromptPack};

/// Optional persistence stores for a `DialogueManager`.
///
/// Both fields default to `None` — useful for tests that don't care
/// about persistence. When set, the manager saves to each store at
/// the points its docstring describes (open / resume / per-turn / close).
///
/// Bundled into one struct rather than passed as two arguments because
/// `DialogueManager::new` was already at the clippy `too_many_arguments`
/// threshold; keeping a pair of optional `Arc<dyn …>` together is also
/// the right grouping conceptually — both are "where do I write changes
/// to disk".
#[derive(Default, Clone)]
pub struct DialogueManagerStores {
    pub session: Option<Arc<dyn SessionStore>>,
    pub learner: Option<Arc<dyn LearnerStore>>,
}

/// Per-turn analysis subsystems wired into the dialogue manager.
///
/// Mirrors `DialogueManagerStores`: groups closely-related parameters
/// (the trait object plus its tunable settings) so `DialogueManager::new`
/// stays under the clippy `too_many_arguments` threshold without
/// suppressions. Both classifier and extractor follow the same
/// "spawn after the response, await with bounded timeout at the start
/// of the next turn" pattern, so co-locating them here is the right
/// grouping conceptually as well.
pub struct DialogueManagerSubsystems {
    pub classifier: Arc<dyn EngagementClassifier>,
    pub classifier_settings: ClassifierSettings,
    pub extractor: Arc<dyn ConceptExtractor>,
    pub extractor_settings: ExtractorSettings,
    pub comprehension: Arc<dyn primer_comprehension::ComprehensionClassifier>,
    pub comprehension_settings: primer_comprehension::ComprehensionSettings,
}

/// The dialogue manager for a single session.
///
/// Holds references to all the subsystems it needs, plus the mutable
/// session and learner model state. The CLI (or future GUI) drives
/// the conversation by calling `respond_to()` in a loop.
///
/// `inference` and `knowledge` are borrowed references: they are used
/// only synchronously inside method bodies. `storage`, `learner_store`,
/// and `classifier` are `Arc<dyn …>` so they can be captured by the
/// post-response classifier task (`tokio::spawn` requires `'static`).
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
    /// Optional learner-model persistence. When set, the learner model is
    /// saved at the same four points as the session (open, resume, per-turn,
    /// close). Save failures are logged, not propagated.
    learner_store: Option<Arc<dyn LearnerStore>>,
    /// Engagement classifier — called after each Primer response to assess
    /// the child's engagement state. Arc for the same spawn-capture reason.
    classifier: Arc<dyn EngagementClassifier>,
    /// Tunable parameters for the classifier (thresholds, timeouts, etc.).
    classifier_settings: ClassifierSettings,
    /// Handle to the in-flight classifier task spawned after the previous
    /// turn. `None` when no task is running.
    classify_task: Option<JoinHandle<Option<EngagementAssessment>>>,
    /// Concept extractor — called after each Primer response to extract
    /// concepts from the just-completed exchange. Arc for the same
    /// spawn-capture reason as `classifier`.
    extractor: Arc<dyn ConceptExtractor>,
    /// Tunable parameters for the extractor.
    extractor_settings: ExtractorSettings,
    /// Handle to the in-flight post-response chained task (extractor →
    /// comprehension) spawned after the previous turn. `None` when no
    /// task is running. The result carries the (child, primer) turn
    /// indices and both extraction + comprehension outputs so
    /// `apply_post_response_outcome` can sync state back into
    /// in-memory `Session.turns` and `LearnerModel`.
    post_response_task: Option<JoinHandle<Option<PostResponseResult>>>,
    /// Comprehension classifier — invoked at the tail of each
    /// post-response chained task (after extraction). Arc for the same
    /// spawn-capture reason as `classifier`.
    comprehension: Arc<dyn primer_comprehension::ComprehensionClassifier>,
    /// Tunable parameters for the comprehension classifier.
    comprehension_settings: primer_comprehension::ComprehensionSettings,
    /// Most recent comprehension result applied to the learner. Cleared
    /// on session lifecycle events. Used by `--verbose`.
    last_comprehension: Option<primer_core::comprehension::ComprehensionResult>,
    /// Pedagogical configuration.
    config: PedagogyConfig,
    /// Most recent extractor output applied to the learner. Cleared on
    /// session lifecycle events. Used by `--verbose`.
    last_extraction: Option<primer_core::extractor::ConceptExtraction>,
    /// Tracks whether `learner` has fields-that-map-to-the-`learners`-table
    /// changes that haven't been flushed yet. The per-turn save site is
    /// gated by this flag (lifecycle events at open / resume / close
    /// always save, regardless). Set to `true` whenever any persisted
    /// field is mutated; cleared after a successful save.
    ///
    /// Future-proofing: today only `current_engagement` is mutated per-turn
    /// (via `update_learner_model` and `apply_assessment`). When concept
    /// extraction lands and starts populating `learner.concepts` per-turn,
    /// it just sets the flag — no save-site changes needed.
    learner_dirty: bool,
    /// Locale-specific prompt pack used to render every system-prompt
    /// section, intent instruction, engagement note, and section intro.
    /// Selected from `learner.profile.locale` at construction time —
    /// the locale is bound for the lifetime of this manager (no
    /// in-session locale switching today).
    prompt_pack: Arc<dyn PromptPack>,
}

/// Output of the spawned post-response task: the extracted concepts
/// (and their turn indices for syncing back into in-memory
/// `Session.turns`) plus the comprehension assessments. Returned
/// through the `JoinHandle` so `apply_post_response_outcome` can apply
/// both to in-memory state at the next-turn boundary.
struct PostResponseResult {
    extraction: ExtractionPart,
    comprehension: primer_core::comprehension::ComprehensionResult,
}

/// The extraction portion of the post-response result.
struct ExtractionPart {
    child_turn_index: usize,
    primer_turn_index: usize,
    extraction: ConceptExtraction,
}

/// Outcome of `drain_classification`. `Some((abort, result))` when a task
/// was pending; `None` when not. The abort handle lets the apply step
/// abort on timeout. Aliased so the parallel-await path can name the
/// cross-future result type without spelling out the full nested Result.
type ClassificationOutcome = Option<(
    tokio::task::AbortHandle,
    std::result::Result<
        std::result::Result<Option<EngagementAssessment>, tokio::task::JoinError>,
        tokio::time::error::Elapsed,
    >,
)>;

/// Outcome of `drain_post_response`. `Some(result)` when a task was
/// pending; `None` when not. No abort handle — post-response tasks are
/// detached on timeout (the spawned DB writes still complete in the
/// background) rather than aborted.
type PostResponseOutcome = Option<
    std::result::Result<
        std::result::Result<Option<PostResponseResult>, tokio::task::JoinError>,
        tokio::time::error::Elapsed,
    >,
>;

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

/// Merge a `ConceptExtraction` into the in-memory `LearnerModel.concepts`.
///
/// Adds new `ConceptState` rows (depth = `Aware`, confidence =
/// `consts::INITIAL_CONCEPT_CONFIDENCE`) for concepts not yet seen;
/// for concepts already in the learner model, increments
/// `encounter_count` and refreshes `last_encountered`. The updated
/// state is what `LearnerStore::save_learner` will persist on the
/// next save (idempotent upsert into `learner_concepts` — monotonic
/// across the child's lifetime).
///
/// Both `child_concepts` and `primer_concepts` feed into the same
/// `learner.concepts` store, but a concept appearing in BOTH lists
/// counts as a single encounter — one exchange in which this concept
/// was mentioned, regardless of which speaker(s) used it. Today the
/// model doesn't distinguish "a concept the child surfaced" from "a
/// concept the Primer introduced"; future work could add a per-side
/// `encounter_count_by_speaker`.
pub(crate) fn apply_extraction(
    learner: &mut primer_core::learner::LearnerModel,
    extraction: &primer_core::extractor::ConceptExtraction,
) -> bool {
    use primer_core::learner::{ConceptState, UnderstandingDepth};
    use std::collections::HashSet;

    let now = Utc::now();
    let mut changed = false;

    // Dedupe across (child, primer) — one exchange = at most one
    // encounter per concept name, even if both speakers used it.
    let mut seen: HashSet<&str> = HashSet::new();
    let unique_names = extraction
        .child_concepts
        .iter()
        .chain(extraction.primer_concepts.iter())
        .filter(|name| seen.insert(name.as_str()));

    for name in unique_names {
        if let Some(existing) = learner.concepts.iter_mut().find(|c| c.concept_id == *name) {
            existing.encounter_count = existing.encounter_count.saturating_add(1);
            existing.last_encountered = Some(now);
            changed = true;
        } else {
            learner.concepts.push(ConceptState {
                concept_id: name.clone(),
                depth: UnderstandingDepth::Aware,
                confidence: crate::consts::INITIAL_CONCEPT_CONFIDENCE,
                encounter_count: 1,
                last_encountered: Some(now),
                notes: vec![],
            });
            changed = true;
        }
    }
    changed
}

/// Apply a `ComprehensionResult` to the in-memory `LearnerModel`.
///
/// For each assessment whose `confidence >= settings.confidence_threshold`,
/// promote `learner.concepts[concept].depth` via monotonic max
/// (never demote — that's an explicit forgetting event handled
/// algorithmically over `turn_comprehensions`, not here).
///
/// Sub-threshold assessments are still persisted to disk by the
/// caller (full longitudinal record) but don't update in-memory state.
///
/// Concepts not already in `learner.concepts` are skipped — insertion
/// is the responsibility of `apply_extraction` (which always runs
/// before this function in the await sequence). The corner case of
/// an assessment for an unknown concept is tolerated (parser-layer
/// drops them) but documented here as defensive.
///
/// Returns `true` if any depth or confidence was updated; the caller
/// uses this to set `learner_dirty` so the per-turn save flushes.
pub(crate) fn apply_comprehension(
    learner: &mut primer_core::learner::LearnerModel,
    result: &primer_core::comprehension::ComprehensionResult,
    settings: &primer_comprehension::ComprehensionSettings,
) -> bool {
    let mut changed = false;
    for a in &result.assessments {
        if a.confidence < settings.confidence_threshold {
            continue;
        }
        if let Some(c) = learner
            .concepts
            .iter_mut()
            .find(|c| c.concept_id == a.concept)
        {
            if a.depth > c.depth {
                c.depth = a.depth;
                // Confidence reflects belief in the *current* depth label.
                // When depth promotes, adopt this turn's confidence rather
                // than max'ing with the prior — the prior measured belief
                // in a different (lower) depth and is no longer applicable.
                c.confidence = a.confidence;
                changed = true;
            }
        }
    }
    changed
}

/// Append `new_concepts` to `turns[index].concepts`, preserving order
/// and skipping names already present. Used by `apply_post_response_outcome`
/// to keep the in-memory `Session.turns` in sync with what the spawned
/// extractor task wrote to disk via `update_exchange_concepts`. A
/// silently-out-of-bounds index is treated as a no-op since the
/// session is append-only and the index was captured at spawn time;
/// pruning would only happen after the session is reset, in which case
/// the in-memory update has no consumer anyway.
pub(crate) fn merge_concepts_into_turn(turns: &mut [Turn], index: usize, new_concepts: &[String]) {
    let Some(turn) = turns.get_mut(index) else {
        return;
    };
    for name in new_concepts {
        if !turn.concepts.iter().any(|existing| existing == name) {
            turn.concepts.push(name.clone());
        }
    }
}

impl<'a> DialogueManager<'a> {
    /// Create a new dialogue manager for a session.
    ///
    /// `stores` bundles the optional `SessionStore` and `LearnerStore`;
    /// `subsystems` bundles the classifier and extractor along with
    /// their settings. Both bundles use `Arc<dyn …>` for the trait
    /// objects so the post-response spawned tasks can capture them
    /// without lifetime constraints — `tokio::spawn` requires `'static`.
    pub fn new(
        learner: LearnerModel,
        inference: &'a dyn InferenceBackend,
        knowledge: &'a dyn KnowledgeBase,
        stores: DialogueManagerStores,
        subsystems: DialogueManagerSubsystems,
        config: PedagogyConfig,
    ) -> Self {
        let session = Session::new(learner.profile.id);
        // `load_cached` returns a process-wide shared `Arc<dyn PromptPack>`
        // so successive `DialogueManager::new` calls in the same process
        // (tests, future multi-session flows) don't re-parse the embedded
        // TOML. PRIMER_PROMPTS_DIR bypasses the cache for translator
        // iteration.
        let prompt_pack = prompt_pack::load_cached(learner.profile.locale)
            .expect("prompt pack load failed; this should be impossible at runtime");
        Self {
            learner,
            session,
            inference,
            knowledge,
            storage: stores.session,
            learner_store: stores.learner,
            classifier: subsystems.classifier,
            classifier_settings: subsystems.classifier_settings,
            classify_task: None,
            extractor: subsystems.extractor,
            extractor_settings: subsystems.extractor_settings,
            post_response_task: None,
            comprehension: subsystems.comprehension,
            comprehension_settings: subsystems.comprehension_settings,
            last_comprehension: None,
            config,
            last_extraction: None,
            learner_dirty: false,
            prompt_pack,
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
        if let Some(ref ls) = self.learner_store {
            // Lifecycle event: save unconditionally to materialise the row,
            // then reset the dirty flag — disk now reflects in-memory state.
            if let Err(e) = ls.save_learner(&self.learner).await {
                tracing::warn!("learner save failed (open_session): {e}");
            } else {
                self.learner_dirty = false;
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

        // Rehydrate recent_assessments + current_engagement from persisted
        // classifications. Filtered by the current classifier's identifier so
        // resuming with a different classifier starts a fresh trajectory rather
        // than mixing outputs from different classifiers.
        if let Some(store) = self.storage.as_ref() {
            let recent = store
                .load_recent_assessments(
                    self.session.id,
                    self.classifier.identifier(),
                    self.classifier_settings.history_depth,
                )
                .await?;
            if let Some(latest) = recent.last() {
                self.learner.current_engagement = latest.state;
            }
            self.learner.recent_assessments = recent;
        }

        if let Some(ref store) = self.storage {
            if let Err(e) = store.save_session(&self.session).await {
                tracing::warn!("session save failed during resume: {e}");
            }
        }
        if let Some(ref ls) = self.learner_store {
            // Lifecycle event: save unconditionally, reset dirty.
            if let Err(e) = ls.save_learner(&self.learner).await {
                tracing::warn!("learner save failed (resume_session): {e}");
            } else {
                self.learner_dirty = false;
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
        // 0. Wait for the previous turn's classification + extraction (if any)
        //    to complete with bounded timeouts, then apply their results so
        //    decide_intent sees the updated engagement state and the system
        //    prompt sees freshly-extracted learner concepts. Awaited in
        //    parallel — the two tasks are independent and write to disjoint
        //    fields of self.learner.
        self.await_pending_background().await;

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
        let intent = prompt_builder::decide_intent_with_pack(
            &*self.prompt_pack,
            &self.learner,
            &self.session,
        );
        let knowledge_context = self.retrieve_knowledge(child_input).await;
        let (summary, retrieved_older) = self.retrieve_long_term_memory(child_input).await;
        let prompt = prompt_builder::build_prompt_with_pack(
            &*self.prompt_pack,
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
            let mut stream = self.inference.generate_stream(&prompt, &params).await?;

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
            let active_concepts = prompt_builder::extract_active_concepts(
                &self.session,
                crate::consts::ACTIVE_CONCEPT_LOOKBACK,
            );
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

        // 5. Save the session and learner if stores are configured. Runs on both
        //    Ok and Err paths. Save failures are logged, not propagated.
        if let Some(ref store) = self.storage {
            if let Err(e) = store.save_session(&self.session).await {
                tracing::warn!("session save failed: {e}");
            }
        }
        // Per-turn learner save is gated by `learner_dirty` so we don't
        // burn a SQLite write transaction every turn when nothing
        // persisted has changed. Lifecycle events (open / resume / close)
        // still save unconditionally; this gate is only the per-turn path.
        if self.learner_dirty {
            if let Some(ref ls) = self.learner_store {
                if let Err(e) = ls.save_learner(&self.learner).await {
                    tracing::warn!("learner save failed (per-turn): {e}");
                } else {
                    self.learner_dirty = false;
                }
            }
        }

        // 6. Spawn a classification task for the child turn that just completed.
        //    Skipped on error paths — without a completed Primer response there
        //    is no exchange to assess, and the partial Primer turn was dropped.
        if result.is_ok() {
            let child_turn_index = self
                .session
                .turns
                .iter()
                .enumerate()
                .rev()
                .find(|(_, t)| t.speaker == Speaker::Child)
                .map(|(i, _)| i);

            if let Some(child_idx) = child_turn_index {
                let store = self.storage.clone();
                let classifier = Arc::clone(&self.classifier);
                let session_id = self.session.id;

                // Build owned copies of the context inputs — the spawned task
                // needs 'static, so we cannot pass slices that borrow self.
                let recent_child_turns: Vec<Turn> = self
                    .session
                    .turns
                    .iter()
                    .filter(|t| t.speaker == Speaker::Child)
                    .rev()
                    .take(self.classifier_settings.recent_child_turns)
                    .cloned()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                let prior_assessments: Vec<EngagementAssessment> =
                    self.learner.recent_assessments.clone();

                // Latency instrumentation (Phase 1, see
                // docs/TODO/OPTIMIZING_LLM_REQUEST_ORDER.md). Owned identifier
                // string and pre-spawn instant captured before tokio::spawn so
                // the closure can compute queued_ms without borrowing self.
                let classifier_id = classifier.identifier().to_string();
                let classifier_pre_spawn = std::time::Instant::now();

                let task = tokio::spawn(async move {
                    let task_start = std::time::Instant::now();
                    let queued_ms =
                        task_start.duration_since(classifier_pre_spawn).as_millis() as u64;
                    let ctx = primer_core::classifier::EngagementContext {
                        recent_child_turns: &recent_child_turns,
                        prior_assessments: &prior_assessments,
                    };
                    let outcome = match classifier.classify(ctx).await {
                        Ok(a) => {
                            if let Some(store) = store {
                                if let Err(e) = store
                                    .save_classification(
                                        session_id,
                                        child_idx,
                                        &a,
                                        classifier.identifier(),
                                    )
                                    .await
                                {
                                    tracing::warn!(error = ?e, "save_classification failed");
                                }
                            }
                            Some(a)
                        }
                        Err(e) => {
                            tracing::warn!(error = ?e, "classifier returned error");
                            None
                        }
                    };
                    let work_ms = task_start.elapsed().as_millis() as u64;
                    tracing::info!(
                        target: "primer::latency",
                        task = "classifier",
                        identifier = %classifier_id,
                        queued_ms,
                        work_ms,
                        succeeded = outcome.is_some(),
                    );
                    outcome
                });
                self.classify_task = Some(task);
            }

            // 7. Spawn an extraction task for the just-completed exchange.
            //    Same skip-on-error policy as the classifier. The task
            //    self-persists `turn_concepts` for both turns atomically
            //    via `update_exchange_concepts`; the JoinHandle output is
            //    drained by `await_pending_background` at the start of the
            //    next turn so the in-memory `learner.concepts` AND
            //    `session.turns[child/primer].concepts` can be updated.
            let total_turns = self.session.turns.len();
            if total_turns >= 2
                && self.session.turns[total_turns - 1].speaker == Speaker::Primer
                && self.session.turns[total_turns - 2].speaker == Speaker::Child
            {
                let child_idx = total_turns - 2;
                let primer_idx = total_turns - 1;
                let child_turn = self.session.turns[child_idx].clone();
                let primer_turn = self.session.turns[primer_idx].clone();
                let recent_turns: Vec<Turn> = self
                    .session
                    .turns
                    .iter()
                    .rev()
                    .skip(2) // skip the just-added child + primer turns
                    .take(self.extractor_settings.recent_context_turns)
                    .cloned()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                let extractor = Arc::clone(&self.extractor);
                let comprehension = Arc::clone(&self.comprehension);
                let comp_settings = self.comprehension_settings.clone();
                let store = self.storage.clone();
                let session_id = self.session.id;
                let comp_classifier_id = comprehension.identifier().to_string();

                // Latency instrumentation (Phase 1, see
                // docs/TODO/OPTIMIZING_LLM_REQUEST_ORDER.md). Owned identifier
                // string and pre-spawn instant captured before tokio::spawn.
                let extractor_id = extractor.identifier().to_string();
                let chain_pre_spawn = std::time::Instant::now();

                let task = tokio::spawn(async move {
                    let task_start = std::time::Instant::now();
                    let queued_ms = task_start.duration_since(chain_pre_spawn).as_millis() as u64;

                    // ── Step 1: Extract concepts ──
                    let extract_start = std::time::Instant::now();
                    let extraction_ctx = primer_core::extractor::ExtractionContext {
                        child_turn: &child_turn,
                        primer_turn: &primer_turn,
                        recent_turns: &recent_turns,
                    };
                    let extraction = match extractor.extract(extraction_ctx).await {
                        Ok(e) => e,
                        Err(e) => {
                            tracing::warn!(error = ?e, "extractor returned error");
                            tracing::info!(
                                target: "primer::latency",
                                task = "chain",
                                extractor_id = %extractor_id,
                                comprehension_id = %comp_classifier_id,
                                queued_ms,
                                extract_ms = extract_start.elapsed().as_millis() as u64,
                                comprehension_ms = 0u64,
                                work_ms = task_start.elapsed().as_millis() as u64,
                                outcome_label = "extractor_error",
                            );
                            return None;
                        }
                    };
                    let extract_ms = extract_start.elapsed().as_millis() as u64;

                    // ── Step 2: Persist concepts ──
                    if let Some(ref store) = store {
                        if let Err(e) = store
                            .update_exchange_concepts(
                                session_id,
                                child_idx,
                                &extraction.child_concepts,
                                primer_idx,
                                &extraction.primer_concepts,
                            )
                            .await
                        {
                            tracing::warn!(error = ?e, "update_exchange_concepts failed");
                        }
                    }

                    // ── Step 3: Build candidate concepts (child ∪ primer, dedup, capped) ──
                    let mut candidates: Vec<String> = Vec::with_capacity(
                        extraction.child_concepts.len() + extraction.primer_concepts.len(),
                    );
                    let mut seen = std::collections::HashSet::new();
                    for c in extraction
                        .child_concepts
                        .iter()
                        .chain(extraction.primer_concepts.iter())
                    {
                        if seen.insert(c.clone()) {
                            candidates.push(c.clone());
                            if candidates.len() >= comp_settings.max_concepts_per_call {
                                break;
                            }
                        }
                    }

                    // ── Step 4: Run comprehension ──
                    // `comprehension_ms = 0` when candidates is empty — the
                    // classifier was never invoked, not a "0ms call".
                    let (comp_result, comprehension_ms) = if candidates.is_empty() {
                        (
                            primer_core::comprehension::ComprehensionResult::empty(),
                            0u64,
                        )
                    } else {
                        let comp_ctx = primer_core::comprehension::ComprehensionContext {
                            child_turn: &child_turn,
                            primer_turn: &primer_turn,
                            recent_turns: &recent_turns,
                            candidate_concepts: &candidates,
                        };
                        let comp_start = std::time::Instant::now();
                        let r = match comprehension.classify(comp_ctx).await {
                            Ok(r) => r,
                            Err(e) => {
                                tracing::warn!(error = ?e, "comprehension returned error");
                                primer_core::comprehension::ComprehensionResult::empty()
                            }
                        };
                        (r, comp_start.elapsed().as_millis() as u64)
                    };

                    // ── Step 5: Persist comprehensions ──
                    if !comp_result.assessments.is_empty() {
                        if let Some(ref store) = store {
                            if let Err(e) = store
                                .save_comprehensions(
                                    session_id,
                                    primer_idx,
                                    &comp_result.assessments,
                                    &comp_classifier_id,
                                )
                                .await
                            {
                                tracing::warn!(error = ?e, "save_comprehensions failed");
                            }
                        }
                    }

                    let work_ms = task_start.elapsed().as_millis() as u64;
                    tracing::info!(
                        target: "primer::latency",
                        task = "chain",
                        extractor_id = %extractor_id,
                        comprehension_id = %comp_classifier_id,
                        queued_ms,
                        extract_ms,
                        comprehension_ms,
                        work_ms,
                        outcome_label = "ok",
                    );

                    Some(PostResponseResult {
                        extraction: ExtractionPart {
                            child_turn_index: child_idx,
                            primer_turn_index: primer_idx,
                            extraction,
                        },
                        comprehension: comp_result,
                    })
                });
                self.post_response_task = Some(task);
            }
        }

        result
    }

    /// Last `PedagogicalIntent` selected by `decide_intent` (used by `--verbose`).
    /// Returns `None` until at least one turn has been processed.
    pub fn last_intent(&self) -> Option<PedagogicalIntent> {
        self.session
            .turns
            .iter()
            .rev()
            .find(|t| t.speaker == Speaker::Primer)
            .and_then(|t| t.intent)
    }

    /// Most recent classifier output applied to the learner (used by `--verbose`).
    /// Returns `None` until at least one classification has completed.
    pub fn last_assessment(&self) -> Option<&primer_core::classifier::EngagementAssessment> {
        self.learner.recent_assessments.last()
    }

    /// Stable identifier of the active engagement classifier (used by `--verbose`).
    pub fn classifier_identifier(&self) -> &str {
        self.classifier.identifier()
    }

    /// Most recent extractor output applied to the learner (used by `--verbose`).
    /// Returns `None` until at least one extraction has completed.
    pub fn last_extraction(&self) -> Option<&primer_core::extractor::ConceptExtraction> {
        self.last_extraction.as_ref()
    }

    /// Stable identifier of the active concept extractor (used by `--verbose`).
    pub fn extractor_identifier(&self) -> &str {
        self.extractor.identifier()
    }

    /// Most recent comprehension result applied to the learner (used by `--verbose`).
    /// Cleared on session lifecycle events. Returns `None` until the first
    /// completed exchange whose comprehension has been awaited.
    pub fn last_comprehension(&self) -> Option<&primer_core::comprehension::ComprehensionResult> {
        self.last_comprehension.as_ref()
    }

    /// Stable identifier of the active comprehension classifier (used by `--verbose`).
    pub fn comprehension_identifier(&self) -> &str {
        self.comprehension.identifier()
    }

    /// Check whether the session has run long enough that the Primer
    /// should suggest a break.
    pub fn should_suggest_break(&self) -> bool {
        let elapsed = Utc::now()
            .signed_duration_since(self.session.started_at)
            .num_minutes();
        elapsed >= self.config.max_session_minutes as i64
    }

    /// End the session gracefully. Drains any in-flight classifier task so
    /// the final turn's assessment lands on disk, records `ended_at`, and
    /// (if storage is configured) fires a final save so the timestamp
    /// lands on disk. Save failures are logged via `tracing::warn!` rather
    /// than propagated — matching `respond_to_streaming`'s save-failure
    /// semantics.
    pub async fn close_session(&mut self) {
        // Drain the post-response classifier + extractor tasks spawned
        // after the most recent turn. Without this, a quick exit
        // ("respond_to_streaming" immediately followed by "close_session")
        // races the runtime shutdown and the last turn_classifications /
        // turn_concepts rows may never be persisted. Drained in parallel
        // — see await_pending_background for the wallclock argument.
        self.await_pending_background().await;

        self.session.ended_at = Some(Utc::now());
        if let Some(ref store) = self.storage {
            if let Err(e) = store.save_session(&self.session).await {
                tracing::warn!("session save failed during close: {e}");
            }
        }
        if let Some(ref ls) = self.learner_store {
            // Lifecycle event: final flush, save unconditionally and
            // reset dirty (the manager is going away but be tidy).
            if let Err(e) = ls.save_learner(&self.learner).await {
                tracing::warn!("learner save failed (close_session): {e}");
            } else {
                self.learner_dirty = false;
            }
        }
    }

    // ─── Classifier helpers ───────────────────────────────────────────

    /// Drain only the classifier task and apply its outcome.
    ///
    /// Production paths use `await_pending_background` (which drains both
    /// background tasks in parallel); this focused variant exists for unit
    /// tests that exercise classifier behaviour in isolation without
    /// setting up a post-response chain.
    #[cfg(test)]
    async fn await_pending_classification(&mut self) {
        let outcome = self.drain_classification().await;
        self.apply_classification_outcome(outcome);
    }

    /// Drain both the classifier task and the post-response chain
    /// concurrently, then apply both outcomes to `self`.
    ///
    /// The two tasks were spawned independently after the previous turn
    /// and write to disjoint fields of `self.learner` (engagement vs
    /// concepts/comprehension). Awaiting them with `tokio::join!` caps
    /// wallclock at `max(classifier_timeout, extractor_timeout +
    /// comprehension_timeout)` — at default settings, 5s instead of
    /// `3 + 5 + 5 = 13s` on a worst-case full-timeout exchange.
    async fn await_pending_background(&mut self) {
        let post_response_timeout =
            self.extractor_settings.blocking_timeout + self.comprehension_settings.blocking_timeout;
        let classify_fut = self.drain_classification();
        let post_response_fut = self.drain_post_response();
        let (classify_outcome, post_response_outcome) =
            tokio::join!(classify_fut, post_response_fut);
        self.apply_classification_outcome(classify_outcome);
        self.apply_post_response_outcome(post_response_outcome, post_response_timeout);
    }

    /// Take the classifier handle out of `self` and return a `'static`
    /// future that awaits it with timeout. Captures the abort handle so
    /// the apply step can abort on timeout. Returns `None` immediately
    /// if no task is pending.
    fn drain_classification(
        &mut self,
    ) -> impl std::future::Future<Output = ClassificationOutcome> + use<> {
        let task = self.classify_task.take();
        let timeout = self.classifier_settings.blocking_timeout;
        async move {
            let task = task?;
            let abort = task.abort_handle();
            let result = tokio::time::timeout(timeout, task).await;
            Some((abort, result))
        }
    }

    /// Take the post-response handle out of `self` and return a `'static`
    /// future that awaits it with the combined extractor + comprehension
    /// timeout. Returns `None` immediately if no task is pending.
    fn drain_post_response(
        &mut self,
    ) -> impl std::future::Future<Output = PostResponseOutcome> + use<> {
        let task = self.post_response_task.take();
        let timeout =
            self.extractor_settings.blocking_timeout + self.comprehension_settings.blocking_timeout;
        async move {
            let task = task?;
            Some(tokio::time::timeout(timeout, task).await)
        }
    }

    /// Apply a classifier outcome (from `drain_classification`) to `self`.
    fn apply_classification_outcome(&mut self, outcome: ClassificationOutcome) {
        let Some((abort, result)) = outcome else {
            return;
        };
        match result {
            Ok(Ok(Some(assessment))) => {
                // Capture the persisted-field state, apply, and dirty
                // only if the persisted field actually changed.
                // `recent_assessments` is rehydrated from
                // `turn_classifications` on resume, so it does NOT need
                // to dirty the `learners` row.
                let before = self.learner.current_engagement;
                apply_assessment(&mut self.learner, assessment, &self.classifier_settings);
                if self.learner.current_engagement != before {
                    self.learner_dirty = true;
                }
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

    /// Apply a post-response outcome (from `drain_post_response`) to `self`.
    /// `timeout` is only used for the timeout-warning log line.
    fn apply_post_response_outcome(
        &mut self,
        outcome: PostResponseOutcome,
        timeout: std::time::Duration,
    ) {
        let Some(result) = outcome else {
            return;
        };
        match result {
            Ok(Ok(Some(result))) => {
                // Apply extraction first so any new concepts are in
                // learner.concepts before comprehension promotes their
                // depths.
                if apply_extraction(&mut self.learner, &result.extraction.extraction) {
                    self.learner_dirty = true;
                }
                merge_concepts_into_turn(
                    &mut self.session.turns,
                    result.extraction.child_turn_index,
                    &result.extraction.extraction.child_concepts,
                );
                merge_concepts_into_turn(
                    &mut self.session.turns,
                    result.extraction.primer_turn_index,
                    &result.extraction.extraction.primer_concepts,
                );
                self.last_extraction = Some(result.extraction.extraction);

                // Apply comprehension — promotes depths via monotonic
                // max for assessments meeting the confidence threshold.
                if apply_comprehension(
                    &mut self.learner,
                    &result.comprehension,
                    &self.comprehension_settings,
                ) {
                    self.learner_dirty = true;
                }
                self.last_comprehension = Some(result.comprehension);
            }
            Ok(Ok(None)) => {
                // Task completed but returned None (extractor errored).
                // No state to apply.
            }
            Ok(Err(e)) => tracing::warn!(error = ?e, "post-response task panicked"),
            Err(_) => {
                tracing::warn!(
                    timeout_ms = timeout.as_millis() as u64,
                    "post-response chain exceeded blocking timeout — proceeding with stale state"
                );
                // task is dropped here, but tokio::spawn'd futures
                // continue to run to completion in the background.
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
        let new_engagement = if word_count == 0 {
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
        // Only mark dirty if the persisted field actually changed —
        // assigning the same value back is a no-op for the on-disk row,
        // and the per-turn save site uses the dirty flag to decide whether
        // to issue a write transaction.
        if self.learner.current_engagement != new_engagement {
            self.learner.current_engagement = new_engagement;
            self.learner_dirty = true;
        }
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
    use primer_core::error::PrimerError;
    use primer_core::inference::{
        GenerationParams, InferenceBackend, Prompt, TokenChunk, TokenStream,
    };
    use primer_core::knowledge::{KnowledgeBase, Passage, RetrievalParams};
    use primer_core::learner::{
        EngagementState, LearnerModel, LearnerProfile, LearningPreferences,
    };
    use primer_extractor::ExtractorSettings;
    use std::sync::Mutex;
    use uuid::Uuid;

    fn stub_classifier() -> Arc<dyn EngagementClassifier> {
        Arc::new(StubEngagementClassifier::new())
    }

    fn stub_extractor() -> Arc<dyn ConceptExtractor> {
        Arc::new(primer_extractor::StubConceptExtractor::new())
    }

    fn stub_comprehension() -> Arc<dyn primer_comprehension::ComprehensionClassifier> {
        Arc::new(primer_comprehension::StubComprehensionClassifier::new())
    }

    /// Default-everything subsystems bundle for tests that don't care
    /// about the specifics of the classifier/extractor/comprehension.
    fn default_subsystems() -> DialogueManagerSubsystems {
        DialogueManagerSubsystems {
            classifier: stub_classifier(),
            classifier_settings: ClassifierSettings::default(),
            extractor: stub_extractor(),
            extractor_settings: ExtractorSettings::default(),
            comprehension: stub_comprehension(),
            comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
        }
    }

    /// Subsystems bundle for tests that need a specific extractor
    /// (e.g. scripted concepts) but otherwise default classifier/settings.
    fn subsystems_with_extractor(
        extractor: Arc<dyn ConceptExtractor>,
    ) -> DialogueManagerSubsystems {
        DialogueManagerSubsystems {
            classifier: stub_classifier(),
            classifier_settings: ClassifierSettings::default(),
            extractor,
            extractor_settings: ExtractorSettings::default(),
            comprehension: stub_comprehension(),
            comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
        }
    }

    /// Subsystems bundle for tests that need a specific comprehension
    /// classifier but otherwise default classifier/extractor/settings.
    #[allow(dead_code)]
    fn subsystems_with_comprehension(
        comprehension: Arc<dyn primer_comprehension::ComprehensionClassifier>,
    ) -> DialogueManagerSubsystems {
        DialogueManagerSubsystems {
            classifier: stub_classifier(),
            classifier_settings: ClassifierSettings::default(),
            extractor: stub_extractor(),
            extractor_settings: ExtractorSettings::default(),
            comprehension,
            comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
        }
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
        fn set_script(&self, items: Vec<Result<TokenChunk>>) {
            *self.script.lock().unwrap() = Some(items);
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

        async fn most_recent_session_learner_id(&self) -> Result<Option<uuid::Uuid>> {
            Ok(None)
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

    /// Learner-store spy: counts `save_learner` calls. Used to prove that
    /// the per-turn save site fires (or doesn't) per the dirty-flag policy.
    struct CountingLearnerStore {
        saves: Mutex<u32>,
    }

    impl CountingLearnerStore {
        fn new() -> Self {
            Self {
                saves: Mutex::new(0),
            }
        }
        fn save_count(&self) -> u32 {
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

    fn test_learner() -> LearnerModel {
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
            DialogueManagerStores::default(),
            default_subsystems(),
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
            DialogueManagerStores::default(),
            default_subsystems(),
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
            DialogueManagerStores::default(),
            default_subsystems(),
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
            DialogueManagerStores::default(),
            default_subsystems(),
            PedagogyConfig::default(),
        );

        let result = dm.respond_to_streaming("question", |_| {}).await;
        assert!(result.is_err(), "expected Err on mid-stream failure");

        // Child turn should be recorded; Primer turn should NOT be.
        assert_eq!(dm.session.turns.len(), 1);
        assert_eq!(dm.session.turns[0].speaker, Speaker::Child);
    }

    #[tokio::test]
    async fn respond_to_streaming_preserves_typed_inference_error_variant() {
        // Regression test for the dialogue_manager.rs:534 fix (commit c1578251).
        // Before the fix, a .map_err wrap re-wrapped typed InferenceError
        // variants from the backend back into InferenceError::Other via
        // format!(...).into(). That destroyed the typed dispatch the i18n
        // render layer relies on — a 401 from Anthropic landed as
        // Other("Generation failed: ...") and the user saw "Something
        // unexpected went wrong" instead of the friendly Auth message.
        //
        // This test asserts that a typed Auth variant from the backend
        // survives the dialogue_manager round-trip with its variant intact.
        let backend = ScriptedBackend::new(vec![Err(PrimerError::Inference(
            primer_core::error::InferenceError::Auth,
        ))]);
        let knowledge = EmptyKnowledge;
        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            DialogueManagerStores::default(),
            default_subsystems(),
            PedagogyConfig::default(),
        );
        let result = dm.respond_to_streaming("question", |_| {}).await;
        assert!(
            matches!(
                result,
                Err(PrimerError::Inference(
                    primer_core::error::InferenceError::Auth
                ))
            ),
            "expected typed Auth variant to survive round-trip, got: {result:?}"
        );
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
            DialogueManagerStores::default(),
            default_subsystems(),
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
            DialogueManagerStores::default(),
            default_subsystems(),
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
            DialogueManagerStores {
                session: Some(Arc::clone(&store) as Arc<dyn SessionStore>),
                learner: None,
            },
            default_subsystems(),
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
            DialogueManagerStores {
                session: Some(Arc::clone(&store) as Arc<dyn SessionStore>),
                learner: None,
            },
            default_subsystems(),
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
            DialogueManagerStores {
                session: Some(Arc::clone(&store) as Arc<dyn SessionStore>),
                learner: None,
            },
            default_subsystems(),
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
            DialogueManagerStores {
                session: Some(Arc::clone(&store) as Arc<dyn SessionStore>),
                learner: None,
            },
            default_subsystems(),
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
            DialogueManagerStores::default(),
            default_subsystems(),
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
            DialogueManagerStores {
                session: Some(Arc::clone(&store) as Arc<dyn SessionStore>),
                learner: None,
            },
            default_subsystems(),
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
            DialogueManagerStores::default(),
            default_subsystems(),
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
            DialogueManagerStores::default(),
            default_subsystems(),
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
            DialogueManagerStores::default(),
            default_subsystems(),
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
            DialogueManagerStores::default(),
            default_subsystems(),
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
            DialogueManagerStores::default(),
            default_subsystems(),
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
            DialogueManagerStores::default(),
            default_subsystems(),
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
        assert_eq!(
            learner.recent_assessments[0].state,
            EngagementState::Reflecting
        );
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
        assert_eq!(
            learner.recent_assessments[0].state,
            EngagementState::Reflecting
        );
        assert_eq!(
            learner.recent_assessments[1].state,
            EngagementState::FrustratedStuck
        );
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
        assert_eq!(
            learner.current_engagement,
            EngagementState::FrustratedTrying
        );
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

    // ─── apply_comprehension ──────────────────────────────────────────

    #[test]
    fn apply_comprehension_promotes_depth_via_monotonic_max() {
        use primer_comprehension::ComprehensionSettings;
        use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};
        use primer_core::learner::{ConceptState, UnderstandingDepth};

        let mut learner = test_learner();
        learner.concepts.push(ConceptState {
            concept_id: "gravity".into(),
            depth: UnderstandingDepth::Aware,
            confidence: 0.5,
            encounter_count: 1,
            last_encountered: None,
            notes: vec![],
        });

        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "gravity".into(),
                depth: UnderstandingDepth::Comprehension,
                confidence: 0.85,
                evidence: None,
            }],
        };
        let settings = ComprehensionSettings::default();
        let changed = apply_comprehension(&mut learner, &result, &settings);
        assert!(changed);
        assert_eq!(
            learner
                .concepts
                .iter()
                .find(|c| c.concept_id == "gravity")
                .unwrap()
                .depth,
            UnderstandingDepth::Comprehension,
        );
    }

    #[test]
    fn apply_comprehension_does_not_demote() {
        use primer_comprehension::ComprehensionSettings;
        use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};
        use primer_core::learner::{ConceptState, UnderstandingDepth};

        let mut learner = test_learner();
        learner.concepts.push(ConceptState {
            concept_id: "gravity".into(),
            depth: UnderstandingDepth::Comprehension,
            confidence: 0.85,
            encounter_count: 5,
            last_encountered: None,
            notes: vec![],
        });
        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "gravity".into(),
                depth: UnderstandingDepth::Aware,
                confidence: 0.95,
                evidence: None,
            }],
        };
        let settings = ComprehensionSettings::default();
        let changed = apply_comprehension(&mut learner, &result, &settings);
        assert!(!changed);
        assert_eq!(
            learner
                .concepts
                .iter()
                .find(|c| c.concept_id == "gravity")
                .unwrap()
                .depth,
            UnderstandingDepth::Comprehension,
        );
    }

    #[test]
    fn apply_comprehension_skips_below_confidence_threshold() {
        use primer_comprehension::ComprehensionSettings;
        use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};
        use primer_core::learner::{ConceptState, UnderstandingDepth};

        let mut learner = test_learner();
        learner.concepts.push(ConceptState {
            concept_id: "gravity".into(),
            depth: UnderstandingDepth::Aware,
            confidence: 0.5,
            encounter_count: 1,
            last_encountered: None,
            notes: vec![],
        });
        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "gravity".into(),
                depth: UnderstandingDepth::Comprehension,
                confidence: 0.3, // below default threshold
                evidence: None,
            }],
        };
        let settings = ComprehensionSettings::default();
        let changed = apply_comprehension(&mut learner, &result, &settings);
        assert!(!changed);
        assert_eq!(
            learner
                .concepts
                .iter()
                .find(|c| c.concept_id == "gravity")
                .unwrap()
                .depth,
            UnderstandingDepth::Aware,
        );
    }

    #[test]
    fn apply_comprehension_skips_concept_not_in_learner_model() {
        use primer_comprehension::ComprehensionSettings;
        use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};
        use primer_core::learner::UnderstandingDepth;

        let mut learner = test_learner(); // empty concepts
        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "missing".into(),
                depth: UnderstandingDepth::Comprehension,
                confidence: 0.9,
                evidence: None,
            }],
        };
        let settings = ComprehensionSettings::default();
        let changed = apply_comprehension(&mut learner, &result, &settings);
        assert!(!changed);
        // No insertion — apply_extraction is the only insertion path.
        assert!(!learner.concepts.iter().any(|c| c.concept_id == "missing"));
    }

    // ─── Integration: classifier spawned and applied across turns ─────

    #[tokio::test]
    async fn resume_session_rehydrates_recent_assessments() {
        use primer_classifier::{EngagementClassifier, StubEngagementClassifier};
        use primer_core::classifier::EngagementAssessment;
        use primer_core::storage::SessionStore;
        use primer_storage::SqliteSessionStore;

        let storage: Arc<dyn SessionStore> = Arc::new(
            SqliteSessionStore::open_for_locale(
                std::path::Path::new(":memory:"),
                primer_core::i18n::Locale::default(),
            )
            .unwrap(),
        );
        let classifier: Arc<dyn EngagementClassifier> = Arc::new(StubEngagementClassifier::new());

        // Pre-seed: save a session with one child turn and one classification.
        let learner = test_learner();
        let mut session = Session::new(learner.profile.id);
        session.add_turn(Turn {
            speaker: Speaker::Child,
            text: "x".into(),
            timestamp: Utc::now(),
            intent: None,
            concepts: vec![],
        });
        storage.save_session(&session).await.unwrap();
        storage
            .save_classification(
                session.id,
                0,
                &EngagementAssessment {
                    state: EngagementState::FrustratedTrying,
                    confidence: 0.9,
                    reasoning: Some("test".into()),
                },
                "stub",
            )
            .await
            .unwrap();

        // Create a DialogueManager and resume the persisted session.
        let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
        let knowledge = EmptyKnowledge;
        let settings = ClassifierSettings::default();
        let mut dm = DialogueManager::new(
            learner,
            &backend,
            &knowledge,
            DialogueManagerStores {
                session: Some(Arc::clone(&storage) as Arc<dyn SessionStore>),
                learner: None,
            },
            DialogueManagerSubsystems {
                classifier: Arc::clone(&classifier),
                classifier_settings: settings,
                extractor: stub_extractor(),
                extractor_settings: ExtractorSettings::default(),
                comprehension: stub_comprehension(),
                comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
            },
            PedagogyConfig::default(),
        );

        let loaded = storage
            .load_session(session.id)
            .await
            .unwrap()
            .expect("must load");
        dm.resume_session(loaded).await.unwrap();

        // Verify rehydration.
        assert_eq!(
            dm.learner.recent_assessments.len(),
            1,
            "recent_assessments must be populated from the persisted classification"
        );
        assert_eq!(
            dm.learner.recent_assessments[0].state,
            EngagementState::FrustratedTrying,
            "rehydrated state must match what was saved"
        );
        assert_eq!(
            dm.learner.current_engagement,
            EngagementState::FrustratedTrying,
            "current_engagement must reflect the most recent rehydrated assessment"
        );
    }

    #[tokio::test]
    async fn respond_to_streaming_spawns_classify_task_and_persists() {
        use primer_classifier::{EngagementClassifier, StubEngagementClassifier};
        use primer_core::classifier::EngagementAssessment;
        use primer_core::storage::SessionStore;
        use primer_storage::SqliteSessionStore;

        // A classifier that always returns FrustratedTrying with high confidence.
        let target_state = EngagementState::FrustratedTrying;
        let classifier: Arc<dyn EngagementClassifier> = Arc::new(
            StubEngagementClassifier::with_response(EngagementAssessment {
                state: target_state,
                confidence: 0.95,
                reasoning: Some("integration test".into()),
            }),
        );

        let storage: Arc<dyn SessionStore> = Arc::new(
            SqliteSessionStore::open_for_locale(
                std::path::Path::new(":memory:"),
                primer_core::i18n::Locale::default(),
            )
            .unwrap(),
        );

        let backend = ScriptedBackend::new(vec![
            Ok(chunk("Great question!", false)),
            Ok(chunk("", true)),
        ]);
        let knowledge = EmptyKnowledge;
        let settings = ClassifierSettings::default();

        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            DialogueManagerStores {
                session: Some(Arc::clone(&storage) as Arc<dyn SessionStore>),
                learner: None,
            },
            DialogueManagerSubsystems {
                classifier: Arc::clone(&classifier),
                classifier_settings: settings,
                extractor: stub_extractor(),
                extractor_settings: ExtractorSettings::default(),
                comprehension: stub_comprehension(),
                comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
            },
            PedagogyConfig::default(),
        );

        dm.open_session().await.unwrap();

        // Run one full turn. After this call a classify_task should be live.
        let response = dm
            .respond_to_streaming("Why is the sky blue?", |_| {})
            .await
            .unwrap();
        assert!(!response.is_empty(), "should have a non-empty response");

        // The classify_task is now running (or already done). Simulating the
        // start of the next turn by calling await_pending_classification
        // should apply the FrustratedTrying assessment.
        dm.await_pending_classification().await;

        // Assessment applied: current_engagement updated by the stub.
        assert_eq!(
            dm.learner.current_engagement, target_state,
            "await_pending_classification must apply the spawned assessment"
        );
        assert_eq!(
            dm.learner.recent_assessments.len(),
            1,
            "assessment must be pushed into recent_assessments"
        );
    }

    #[tokio::test]
    async fn await_pending_classification_aborts_and_preserves_state_on_timeout() {
        use primer_classifier::EngagementClassifier;
        use primer_core::classifier::{EngagementAssessment, EngagementContext};
        use std::time::Duration;

        // Classifier that sleeps long enough to reliably exceed the test's
        // blocking_timeout. If the timeout path works, the sleep never
        // completes (task gets aborted) and current_engagement stays untouched.
        struct SlowClassifier;

        #[async_trait]
        impl EngagementClassifier for SlowClassifier {
            fn identifier(&self) -> &str {
                "slow"
            }
            async fn classify(&self, _ctx: EngagementContext<'_>) -> Result<EngagementAssessment> {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok(EngagementAssessment {
                    state: EngagementState::FrustratedTrying,
                    confidence: 0.99,
                    reasoning: None,
                })
            }
        }

        let backend = ScriptedBackend::new(vec![Ok(chunk("hi", false)), Ok(chunk("", true))]);
        let knowledge = EmptyKnowledge;
        // Tight timeout so the await reliably trips it before the 5s sleep.
        let settings = ClassifierSettings {
            blocking_timeout: Duration::from_millis(50),
            ..ClassifierSettings::default()
        };

        let mut dm = DialogueManager::new(
            test_learner(),
            &backend,
            &knowledge,
            DialogueManagerStores::default(),
            DialogueManagerSubsystems {
                classifier: Arc::new(SlowClassifier),
                classifier_settings: settings,
                extractor: stub_extractor(),
                extractor_settings: ExtractorSettings::default(),
                comprehension: stub_comprehension(),
                comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
            },
            PedagogyConfig::default(),
        );

        // Run a turn so a classify_task is spawned. The task is still
        // sleeping when respond_to_streaming returns.
        let _ = dm.respond_to_streaming("hi", |_| {}).await.unwrap();
        assert!(
            dm.classify_task.is_some(),
            "a classify_task must be spawned after a successful turn"
        );
        // Capture the engagement state AFTER respond_to_streaming so the
        // placeholder word-count heuristic in `update_learner_model` (which
        // mutates `current_engagement` independently of the classifier) does
        // not contaminate this test. We're checking that the timeout path
        // does not apply the slow classifier's pending result, not that the
        // pre-existing heuristic is bypassed.
        let initial = dm.learner.current_engagement;

        // This call should hit the timeout path: abort the task, log
        // tracing::debug!, and return without applying any assessment.
        let started = std::time::Instant::now();
        dm.await_pending_classification().await;
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(2),
            "await_pending_classification must give up within ~blocking_timeout, \
             not wait for the slow classifier; elapsed={elapsed:?}"
        );
        assert_eq!(
            dm.learner.current_engagement, initial,
            "timeout path must NOT update current_engagement"
        );
        assert!(
            dm.learner.recent_assessments.is_empty(),
            "timeout path must NOT push into recent_assessments"
        );
        assert!(
            dm.classify_task.is_none(),
            "the task handle must be consumed even on timeout"
        );
    }

    /// Backend that serves the same single-chunk response on every
    /// `generate_stream` call. Used by multi-turn tests where the exact
    /// content of the Primer response does not matter.
    struct RepeatingBackend;

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

    // ─── End-to-end: classifier routing across a multi-turn session ───

    #[tokio::test]
    async fn end_to_end_classifier_routing_across_multi_turn_session() {
        use primer_classifier::{EngagementClassifier, StubEngagementClassifier};
        use primer_core::classifier::EngagementAssessment;
        use primer_core::conversation::PedagogicalIntent;
        use primer_core::storage::SessionStore;
        use primer_storage::SqliteSessionStore;
        use std::time::Duration;

        // Scripted classifier:
        //   turn 1 -> Engaged, turn 2 -> FrustratedTrying, turn 3 -> Disengaging
        // Exhausted script falls back to Engaged for turn 4 — but by then
        // current_engagement is already Disengaging (applied before turn 4 starts).
        let classifier: Arc<dyn EngagementClassifier> =
            Arc::new(StubEngagementClassifier::with_script(vec![
                EngagementAssessment {
                    state: EngagementState::Engaged,
                    confidence: 0.9,
                    reasoning: None,
                },
                EngagementAssessment {
                    state: EngagementState::FrustratedTrying,
                    confidence: 0.9,
                    reasoning: None,
                },
                EngagementAssessment {
                    state: EngagementState::Disengaging,
                    confidence: 0.9,
                    reasoning: None,
                },
            ]));

        let storage: Arc<dyn SessionStore> = Arc::new(
            SqliteSessionStore::open_for_locale(
                std::path::Path::new(":memory:"),
                primer_core::i18n::Locale::default(),
            )
            .unwrap(),
        );

        let backend = RepeatingBackend;
        let knowledge = EmptyKnowledge;

        // Generous blocking timeout for deterministic test behaviour.
        let settings = ClassifierSettings {
            blocking_timeout: Duration::from_secs(5),
            ..Default::default()
        };

        let mut learner = test_learner();
        // 60-second threshold: a backdated session (120 s elapsed) reliably
        // routes Disengaging → SessionClose.
        learner.preferences.early_disengagement_threshold = Duration::from_secs(60);

        let mut dm = DialogueManager::new(
            learner,
            &backend,
            &knowledge,
            DialogueManagerStores {
                session: Some(Arc::clone(&storage) as Arc<dyn SessionStore>),
                learner: None,
            },
            DialogueManagerSubsystems {
                classifier: Arc::clone(&classifier),
                classifier_settings: settings,
                extractor: stub_extractor(),
                extractor_settings: ExtractorSettings::default(),
                comprehension: stub_comprehension(),
                comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
            },
            PedagogyConfig::default(),
        );

        dm.open_session().await.unwrap();

        // Backdate started_at so elapsed (120 s) exceeds the 60-second threshold.
        // This makes Disengaging → SessionClose rather than Encouragement.
        dm.session.started_at = Utc::now() - chrono::Duration::seconds(120);

        let session_id = dm.session.id;

        // ── Turn 1 ──
        // classify task returns Engaged (first script entry).
        let _r1 = dm
            .respond_to_streaming("i'm curious about gravity", |_| {})
            .await
            .unwrap();
        // Drain the spawned task; apply Engaged.
        dm.await_pending_classification().await;
        assert_eq!(
            dm.learner.current_engagement,
            EngagementState::Engaged,
            "turn 1: engagement must be Engaged"
        );

        // ── Turn 2 ──
        // At the START of respond_to_streaming, await_pending_classification
        // is called internally — but we already drained it above, so there is
        // nothing to await.  After this call, a new task carrying FrustratedTrying
        // is spawned.
        let _r2 = dm
            .respond_to_streaming("I think it's hard to explain", |_| {})
            .await
            .unwrap();
        // Drain the spawned task; apply FrustratedTrying.
        dm.await_pending_classification().await;
        assert_eq!(
            dm.learner.current_engagement,
            EngagementState::FrustratedTrying,
            "turn 2: engagement must be FrustratedTrying after classifier"
        );

        // ── Turn 3 ──
        // Task for this turn returns Disengaging.
        let _r3 = dm
            .respond_to_streaming("I'm not sure but maybe...", |_| {})
            .await
            .unwrap();
        // Drain; apply Disengaging.
        dm.await_pending_classification().await;
        assert_eq!(
            dm.learner.current_engagement,
            EngagementState::Disengaging,
            "turn 3: engagement must be Disengaging after classifier"
        );

        // ── Turn 4 ──
        // At the START of respond_to_streaming, await_pending_classification
        // is called (nothing to drain — we already did it). Then decide_intent
        // sees Disengaging + elapsed (120 s) > threshold (60 s) → SessionClose.
        let _r4 = dm.respond_to_streaming("ok", |_| {}).await.unwrap();

        // last_intent reads the intent stored on the most recent Primer turn.
        let intent = dm.last_intent().expect("intent must be set after turn 4");
        assert_eq!(
            intent,
            PedagogicalIntent::SessionClose,
            "turn 4: Disengaging + elapsed > threshold must route to SessionClose"
        );

        // Drain the task spawned after turn 4 (not needed for intent assertion,
        // but ensures we don't leave background work running after the test).
        dm.await_pending_classification().await;

        // All four child-turn classifications must have been persisted.
        let recent = storage
            .load_recent_assessments(session_id, "stub", 10)
            .await
            .unwrap();
        assert_eq!(
            recent.len(),
            4,
            "all four turn classifications must be persisted; got {}",
            recent.len()
        );
    }

    #[tokio::test]
    async fn end_to_end_save_learner_after_open_and_one_turn() {
        // Build a manager with both Some(SessionStore) and Some(LearnerStore)
        // backed by the same SqliteSessionStore (which implements both traits).
        // Run open_session + one turn and verify the learners row was upserted.
        use primer_core::storage::LearnerStore;
        use primer_storage::SqliteSessionStore;

        let store = Arc::new(
            SqliteSessionStore::open_for_locale(
                std::path::Path::new(":memory:"),
                primer_core::i18n::Locale::default(),
            )
            .unwrap(),
        );

        // Pre-save the learner so the DB has a row to UPDATE rather than INSERT.
        let learner = test_learner();
        store.save_learner(&learner).await.unwrap();

        let backend = ScriptedBackend::new(vec![Ok(chunk("Hello!", false)), Ok(chunk("", true))]);
        let knowledge = EmptyKnowledge;

        let mut dm = DialogueManager::new(
            learner,
            &backend,
            &knowledge,
            DialogueManagerStores {
                session: Some(Arc::clone(&store) as Arc<dyn SessionStore>),
                learner: Some(Arc::clone(&store) as Arc<dyn LearnerStore>),
            },
            default_subsystems(),
            PedagogyConfig::default(),
        );

        let _greeting = dm.open_session().await.unwrap();
        let _reply = dm.respond_to("hello").await.unwrap();

        // load_learner should return the persisted row.
        let loaded = store
            .load_learner()
            .await
            .unwrap()
            .expect("learner row must exist");
        assert_eq!(
            loaded.profile.id, dm.learner.profile.id,
            "persisted learner id must match"
        );
    }

    #[tokio::test]
    async fn divergence_bug_closed_via_cli_startup_flow() {
        // Fixture: a fresh DB seeded with a session under UUID U1, no
        // learners row yet (simulates the v3 → v4 upgrade-on-first-open).
        // Then run the CLI's first-run startup flow:
        //   load_learner() == None
        //   most_recent_session_learner_id() == Some(U1)
        //   mint LearnerModel with id=U1, save_learner(...)
        // Assert the resulting LearnerModel.profile.id == U1.
        use primer_core::conversation::Session as ConversationSession;
        use primer_core::storage::{LearnerStore, SessionStore};
        use primer_storage::SqliteSessionStore;

        let store = Arc::new(
            SqliteSessionStore::open_for_locale(
                std::path::Path::new(":memory:"),
                primer_core::i18n::Locale::default(),
            )
            .unwrap(),
        );
        let u1 = uuid::Uuid::new_v4();
        let s = ConversationSession::new(u1);
        store.save_session(&s).await.unwrap();

        // Simulate the CLI startup flow.
        let load_result = store.load_learner().await.unwrap();
        assert!(load_result.is_none(), "no learner row yet");

        let adopted = store
            .most_recent_session_learner_id()
            .await
            .unwrap()
            .expect("session exists");
        assert_eq!(adopted, u1);

        let mut adopted_learner = test_learner();
        adopted_learner.profile.id = adopted;
        store.save_learner(&adopted_learner).await.unwrap();

        // Construct a DialogueManager with the adopted learner.
        let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
        let knowledge = EmptyKnowledge;
        let mut dm = DialogueManager::new(
            adopted_learner,
            &backend,
            &knowledge,
            DialogueManagerStores {
                session: Some(Arc::clone(&store) as Arc<dyn SessionStore>),
                learner: Some(Arc::clone(&store) as Arc<dyn LearnerStore>),
            },
            default_subsystems(),
            PedagogyConfig::default(),
        );

        let _ = dm.open_session().await.unwrap();

        assert_eq!(
            dm.session.learner_id, dm.learner.profile.id,
            "session learner_id must match adopted learner id"
        );
        assert_eq!(
            dm.session.learner_id, u1,
            "adopted learner id must be the original session's learner_id"
        );
    }

    // ─── Per-turn save gating (learner_dirty flag) ─────────────────────

    /// Build a manager with a `CountingLearnerStore` and a learner state
    /// that `update_learner_model` will NOT change for the chosen input.
    /// `current_engagement = Reflecting` + a 1-or-2-word input maps to
    /// `Reflecting` again via the `match other => other` branch, so
    /// `current_engagement` is unchanged → no dirty → no per-turn save.
    fn dirty_flag_test_setup(
        starting: EngagementState,
    ) -> (LearnerModel, Arc<CountingLearnerStore>) {
        let mut learner = test_learner();
        learner.current_engagement = starting;
        let store = Arc::new(CountingLearnerStore::new());
        (learner, store)
    }

    #[tokio::test]
    async fn per_turn_save_skipped_when_no_persisted_field_changes() {
        // learner starts at Reflecting; the input "ok yes" is < 3 words so
        // update_learner_model takes the "match other => other" branch
        // and leaves current_engagement at Reflecting. The classifier is
        // a stub returning no assessments. The only save_learner call is
        // the one open_session emits.
        let (learner, store) = dirty_flag_test_setup(EngagementState::Reflecting);
        let backend = RepeatingBackend;
        let knowledge = EmptyKnowledge;

        let mut dm = DialogueManager::new(
            learner,
            &backend,
            &knowledge,
            DialogueManagerStores {
                session: None,
                learner: Some(Arc::clone(&store) as Arc<dyn LearnerStore>),
            },
            default_subsystems(),
            PedagogyConfig::default(),
        );

        let _ = dm.open_session().await.unwrap();
        assert_eq!(
            store.save_count(),
            1,
            "open_session must save once (lifecycle event)"
        );

        let _ = dm.respond_to("ok yes").await.unwrap();
        assert_eq!(
            store.save_count(),
            1,
            "per-turn save must be SKIPPED when no persisted field changed (still 1 from open)"
        );
    }

    #[tokio::test]
    async fn per_turn_save_fires_when_engagement_changes() {
        // learner starts at Reflecting; a long input (>=3 words) maps to
        // Engaged in update_learner_model, which IS a change to a
        // persisted field. dirty=true → per-turn save fires.
        let (learner, store) = dirty_flag_test_setup(EngagementState::Reflecting);
        let backend = RepeatingBackend;
        let knowledge = EmptyKnowledge;

        let mut dm = DialogueManager::new(
            learner,
            &backend,
            &knowledge,
            DialogueManagerStores {
                session: None,
                learner: Some(Arc::clone(&store) as Arc<dyn LearnerStore>),
            },
            default_subsystems(),
            PedagogyConfig::default(),
        );

        let _ = dm.open_session().await.unwrap();
        let count_after_open = store.save_count();

        let _ = dm
            .respond_to("this is a longer answer with many words")
            .await
            .unwrap();
        assert_eq!(
            store.save_count(),
            count_after_open + 1,
            "per-turn save must fire exactly once when current_engagement changes"
        );
    }

    #[tokio::test]
    async fn dirty_cleared_after_save_so_subsequent_idle_turn_skips_save() {
        // Sequence: open → dirty turn → idle turn.
        // After the dirty turn, the flag should be cleared; the idle
        // turn must not produce a second per-turn save.
        let (learner, store) = dirty_flag_test_setup(EngagementState::Reflecting);
        let backend = RepeatingBackend;
        let knowledge = EmptyKnowledge;

        let mut dm = DialogueManager::new(
            learner,
            &backend,
            &knowledge,
            DialogueManagerStores {
                session: None,
                learner: Some(Arc::clone(&store) as Arc<dyn LearnerStore>),
            },
            default_subsystems(),
            PedagogyConfig::default(),
        );

        let _ = dm.open_session().await.unwrap();
        let after_open = store.save_count();

        // Dirty turn — updates engagement Reflecting → Engaged.
        let _ = dm
            .respond_to("this is a longer answer with many words")
            .await
            .unwrap();
        let after_dirty = store.save_count();
        assert_eq!(after_dirty, after_open + 1, "dirty turn must save");

        // Idle turn — current_engagement is now Engaged, input "ok yes"
        // (word_count<3) maps Engaged → Reflecting via the "Engaged =>
        // Reflecting" arm, so the value DOES change. We need an input
        // that keeps Engaged as Engaged: a long input. But that would
        // also keep dirty stable (Engaged → Engaged is no change).
        let _ = dm
            .respond_to("yes that is exactly what I think")
            .await
            .unwrap();
        assert_eq!(
            store.save_count(),
            after_dirty,
            "idle turn (Engaged → Engaged) must NOT save again"
        );
    }

    /// Always-failing learner store: every `save_learner` returns Err.
    /// Used to prove that save failures are logged-and-swallowed rather
    /// than propagated up through the dialogue-manager API.
    struct FailingLearnerStore {
        attempts: Mutex<u32>,
    }
    impl FailingLearnerStore {
        fn new() -> Self {
            Self {
                attempts: Mutex::new(0),
            }
        }
        fn attempt_count(&self) -> u32 {
            *self.attempts.lock().unwrap()
        }
    }
    #[async_trait]
    impl primer_core::storage::LearnerStore for FailingLearnerStore {
        async fn save_learner(&self, _learner: &LearnerModel) -> Result<()> {
            *self.attempts.lock().unwrap() += 1;
            Err(PrimerError::Storage(
                "simulated save_learner failure".into(),
            ))
        }
        async fn load_learner(&self) -> Result<Option<LearnerModel>> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn save_learner_failure_does_not_propagate_through_respond_to() {
        // A failing LearnerStore must be visible only as a tracing::warn —
        // the conversation must continue. Otherwise a flaky disk would
        // shut down the child's session, which is the wrong failure mode
        // for a children's product.
        let mut learner = test_learner();
        learner.current_engagement = EngagementState::Reflecting;
        let failing = Arc::new(FailingLearnerStore::new());
        let backend = RepeatingBackend;
        let knowledge = EmptyKnowledge;

        let mut dm = DialogueManager::new(
            learner,
            &backend,
            &knowledge,
            DialogueManagerStores {
                session: None,
                learner: Some(Arc::clone(&failing) as Arc<dyn LearnerStore>),
            },
            default_subsystems(),
            PedagogyConfig::default(),
        );

        // open_session must succeed despite the underlying save failing.
        let _ = dm
            .open_session()
            .await
            .expect("open_session must not propagate save_learner errors");
        let after_open = failing.attempt_count();
        assert!(after_open >= 1, "open_session must attempt to save");

        // A dirty turn must succeed despite the underlying save failing.
        let reply = dm
            .respond_to("this is a longer answer with many words")
            .await
            .expect("respond_to must not propagate save_learner errors");
        assert!(!reply.is_empty(), "Primer reply must still come through");
        assert!(
            failing.attempt_count() > after_open,
            "per-turn dirty save must be attempted, even though it errors"
        );

        // close_session must also swallow the error (no return value, no panic).
        dm.close_session().await;

        // Because every save errors, the dirty flag should still be set
        // — the save site only clears dirty on success. This is the
        // correct invariant: a failed save did NOT actually flush, so
        // marking clean would be a lie.
        assert!(
            dm.learner_dirty,
            "dirty must remain set when save_learner errors so a future save still runs"
        );
    }

    #[tokio::test]
    async fn close_session_always_saves_learner_regardless_of_dirty() {
        // Lifecycle events flush unconditionally — they're explicit
        // checkpoints, not "save when dirty" sites.
        let (learner, store) = dirty_flag_test_setup(EngagementState::Engaged);
        let backend = RepeatingBackend;
        let knowledge = EmptyKnowledge;

        let mut dm = DialogueManager::new(
            learner,
            &backend,
            &knowledge,
            DialogueManagerStores {
                session: None,
                learner: Some(Arc::clone(&store) as Arc<dyn LearnerStore>),
            },
            default_subsystems(),
            PedagogyConfig::default(),
        );

        let _ = dm.open_session().await.unwrap();
        let after_open = store.save_count();
        dm.close_session().await;
        assert!(
            store.save_count() > after_open,
            "close_session must save unconditionally"
        );
    }

    /// Session-store spy that records `update_turn_concepts` calls so
    /// tests can assert the extractor's persistence side effect. Also
    /// records `save_comprehensions` calls so chain tests can assert
    /// the comprehension persistence side effect.
    struct ConceptCapturingStore {
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
        fn new() -> Self {
            Self {
                inner: CountingStore::new(),
                captures: Mutex::new(vec![]),
                comprehensions: Mutex::new(vec![]),
            }
        }
        fn captured(&self) -> Vec<(usize, Vec<String>)> {
            self.captures.lock().unwrap().clone()
        }
        fn captured_comprehensions(
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

    #[tokio::test]
    async fn extract_task_persists_concepts_for_both_turns_after_response() {
        let backend = ScriptedBackend::new(vec![Ok(chunk("Hi there!", true))]);
        let extractor = Arc::new(primer_extractor::StubConceptExtractor::with_response(
            ConceptExtraction {
                child_concepts: vec!["topic-a".into()],
                primer_concepts: vec!["topic-b".into()],
            },
        ));
        let store = Arc::new(ConceptCapturingStore::new());

        let stores = DialogueManagerStores {
            session: Some(store.clone() as Arc<dyn primer_core::storage::SessionStore>),
            learner: None,
        };

        let mut dm = DialogueManager::new(
            test_learner(),
            &backend as &dyn InferenceBackend,
            &EmptyKnowledge as &dyn KnowledgeBase,
            stores,
            subsystems_with_extractor(extractor as Arc<dyn ConceptExtractor>),
            PedagogyConfig::default(),
        );

        dm.respond_to("Hello").await.unwrap();

        // Yield until the spawned extractor task lands its captures.
        for _ in 0..50 {
            if store.captured().len() >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let captures = store.captured();
        assert_eq!(captures.len(), 2, "expected child + primer captures");
        // Child turn is at index 0, primer at index 1.
        let child_capture = captures.iter().find(|(i, _)| *i == 0).unwrap();
        let primer_capture = captures.iter().find(|(i, _)| *i == 1).unwrap();
        assert_eq!(child_capture.1, vec!["topic-a".to_string()]);
        assert_eq!(primer_capture.1, vec!["topic-b".to_string()]);
    }

    #[tokio::test]
    async fn extract_task_does_not_spawn_on_inference_error() {
        let backend = ScriptedBackend::new(vec![Err(PrimerError::Inference("boom".into()))]);
        let extractor = Arc::new(primer_extractor::StubConceptExtractor::with_response(
            ConceptExtraction {
                child_concepts: vec!["should-not-persist".into()],
                primer_concepts: vec![],
            },
        ));
        let store = Arc::new(ConceptCapturingStore::new());

        let stores = DialogueManagerStores {
            session: Some(store.clone() as Arc<dyn primer_core::storage::SessionStore>),
            learner: None,
        };

        let mut dm = DialogueManager::new(
            test_learner(),
            &backend as &dyn InferenceBackend,
            &EmptyKnowledge as &dyn KnowledgeBase,
            stores,
            subsystems_with_extractor(extractor as Arc<dyn ConceptExtractor>),
            PedagogyConfig::default(),
        );

        let _ = dm.respond_to("Hello").await;

        // Give the runtime a chance to run any spuriously-spawned task.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            store.captured().is_empty(),
            "extractor must not run on inference error"
        );
    }

    #[tokio::test]
    async fn pending_extraction_applied_to_learner_at_next_turn() {
        let backend = ScriptedBackend::new(vec![Ok(chunk("Hi turn 1!", true))]);
        // Two turns of extraction scripted: turn 1 surfaces "gravity" + "physics",
        // turn 2 surfaces "mass". Only the first one matters for this test —
        // we want to assert that after respond_to(turn 2), the learner has
        // gravity from turn 1's extraction.
        let extractor = Arc::new(primer_extractor::StubConceptExtractor::with_script(vec![
            ConceptExtraction {
                child_concepts: vec!["gravity".into()],
                primer_concepts: vec!["physics".into()],
            },
            ConceptExtraction {
                child_concepts: vec!["mass".into()],
                primer_concepts: vec![],
            },
        ]));

        let mut dm = DialogueManager::new(
            test_learner(),
            &backend as &dyn InferenceBackend,
            &EmptyKnowledge as &dyn KnowledgeBase,
            DialogueManagerStores::default(),
            subsystems_with_extractor(extractor as Arc<dyn ConceptExtractor>),
            PedagogyConfig::default(),
        );

        dm.respond_to("turn 1").await.unwrap();

        // Refill the backend script for turn 2.
        backend.set_script(vec![Ok(chunk("Hi turn 2!", true))]);

        // Allow the previous-turn extractor task to complete before turn 2 starts.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        dm.respond_to("turn 2").await.unwrap();

        let names: std::collections::HashSet<&str> = dm
            .learner
            .concepts
            .iter()
            .map(|c| c.concept_id.as_str())
            .collect();
        assert!(
            names.contains("gravity"),
            "child concept 'gravity' should be applied to learner; got: {:?}",
            names
        );
        assert!(
            names.contains("physics"),
            "primer concept 'physics' should be applied to learner; got: {:?}",
            names
        );
    }

    #[tokio::test]
    async fn close_session_drains_extractor_task() {
        let backend = ScriptedBackend::new(vec![Ok(chunk("Hi", true))]);
        let extractor = Arc::new(primer_extractor::StubConceptExtractor::with_response(
            ConceptExtraction {
                child_concepts: vec!["x".into()],
                primer_concepts: vec![],
            },
        ));
        let store = Arc::new(ConceptCapturingStore::new());

        let stores = DialogueManagerStores {
            session: Some(store.clone() as Arc<dyn primer_core::storage::SessionStore>),
            learner: None,
        };

        let mut dm = DialogueManager::new(
            test_learner(),
            &backend as &dyn InferenceBackend,
            &EmptyKnowledge as &dyn KnowledgeBase,
            stores,
            subsystems_with_extractor(extractor as Arc<dyn ConceptExtractor>),
            PedagogyConfig::default(),
        );

        dm.respond_to("hi").await.unwrap();
        // close_session must drain so the extractor's update_turn_concepts
        // call has landed by the time close returns.
        dm.close_session().await;

        let captures = store.captured();
        assert!(
            !captures.is_empty(),
            "expected extraction to land before close returns"
        );
    }

    // ─── Chained post-response (extraction → comprehension) ──────────

    #[tokio::test]
    async fn post_response_chain_persists_extraction_and_comprehension() {
        use primer_comprehension::StubComprehensionClassifier;
        use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};
        use primer_core::extractor::ConceptExtraction;
        use primer_core::learner::UnderstandingDepth;

        let backend = ScriptedBackend::new(vec![Ok(chunk("Hi there!", true))]);
        let extractor = Arc::new(primer_extractor::StubConceptExtractor::with_response(
            ConceptExtraction {
                child_concepts: vec!["gravity".into()],
                primer_concepts: vec![],
            },
        ));
        let comprehension = Arc::new(StubComprehensionClassifier::with_response(
            ComprehensionResult {
                assessments: vec![ComprehensionAssessment {
                    concept: "gravity".into(),
                    depth: UnderstandingDepth::Recall,
                    confidence: 0.8,
                    evidence: Some("named the concept".into()),
                }],
            },
        )) as Arc<dyn primer_comprehension::ComprehensionClassifier>;
        let store = Arc::new(ConceptCapturingStore::new());

        let stores = DialogueManagerStores {
            session: Some(store.clone() as Arc<dyn primer_core::storage::SessionStore>),
            learner: None,
        };

        // Build subsystems directly (subsystems_with_extractor doesn't
        // accept a custom comprehension classifier).
        let subsystems = DialogueManagerSubsystems {
            classifier: stub_classifier(),
            classifier_settings: ClassifierSettings::default(),
            extractor: extractor as Arc<dyn ConceptExtractor>,
            extractor_settings: ExtractorSettings::default(),
            comprehension,
            comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
        };

        let mut dm = DialogueManager::new(
            test_learner(),
            &backend as &dyn InferenceBackend,
            &EmptyKnowledge as &dyn KnowledgeBase,
            stores,
            subsystems,
            PedagogyConfig::default(),
        );

        dm.respond_to("Hello").await.unwrap();
        // close_session drains the post-response chain so both
        // update_exchange_concepts and save_comprehensions have landed
        // and in-memory state has been applied by the time it returns.
        dm.close_session().await;

        // Extraction persisted: child concept captured.
        let captures = store.captured();
        assert!(
            captures
                .iter()
                .any(|(_, names)| names.contains(&"gravity".to_string())),
            "expected child capture of 'gravity'; got {:?}",
            captures
        );

        // Comprehension persisted via save_comprehensions.
        let comp_captures = store.captured_comprehensions();
        assert_eq!(
            comp_captures.len(),
            1,
            "expected one save_comprehensions call; got {:?}",
            comp_captures
        );
        let (primer_idx, assessments, classifier_id) = &comp_captures[0];
        assert_eq!(*primer_idx, 1, "primer turn index should be 1");
        assert_eq!(assessments.len(), 1);
        assert_eq!(assessments[0].concept, "gravity");
        assert_eq!(assessments[0].depth, UnderstandingDepth::Recall);
        assert_eq!(classifier_id, "stub");

        // Last comprehension applied to learner via await_pending_background.
        let last_comp = dm
            .last_comprehension()
            .expect("last_comprehension must be set after the chain runs");
        assert_eq!(last_comp.assessments.len(), 1);
        assert_eq!(last_comp.assessments[0].concept, "gravity");

        // learner.concepts has gravity at Recall (extraction inserted at
        // Aware first; comprehension promoted to Recall via monotonic max).
        let gravity = dm
            .learner
            .concepts
            .iter()
            .find(|c| c.concept_id == "gravity")
            .expect("'gravity' must be in learner concepts");
        assert_eq!(gravity.depth, UnderstandingDepth::Recall);
    }

    #[tokio::test]
    async fn post_response_chain_skips_comprehension_on_empty_extraction() {
        // Stub extractor returns empty → candidate_concepts is empty →
        // comprehension MUST NOT be invoked. The spy comprehension
        // classifier panics if classify() is called.
        struct PanicOnCall;
        #[async_trait]
        impl primer_comprehension::ComprehensionClassifier for PanicOnCall {
            fn identifier(&self) -> &str {
                "panic"
            }
            async fn classify(
                &self,
                _ctx: primer_core::comprehension::ComprehensionContext<'_>,
            ) -> Result<primer_core::comprehension::ComprehensionResult> {
                panic!("comprehension must not be called when extractor returned empty");
            }
        }

        let backend = ScriptedBackend::new(vec![Ok(chunk("Hi there!", true))]);
        let extractor =
            Arc::new(primer_extractor::StubConceptExtractor::new()) as Arc<dyn ConceptExtractor>;
        let comprehension =
            Arc::new(PanicOnCall) as Arc<dyn primer_comprehension::ComprehensionClassifier>;
        let store = Arc::new(ConceptCapturingStore::new());

        let stores = DialogueManagerStores {
            session: Some(store.clone() as Arc<dyn primer_core::storage::SessionStore>),
            learner: None,
        };

        let subsystems = DialogueManagerSubsystems {
            classifier: stub_classifier(),
            classifier_settings: ClassifierSettings::default(),
            extractor,
            extractor_settings: ExtractorSettings::default(),
            comprehension,
            comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
        };

        let mut dm = DialogueManager::new(
            test_learner(),
            &backend as &dyn InferenceBackend,
            &EmptyKnowledge as &dyn KnowledgeBase,
            stores,
            subsystems,
            PedagogyConfig::default(),
        );

        dm.respond_to("Hello").await.unwrap();
        // Drain. If PanicOnCall.classify() were called, the panicked
        // task would surface as an Err inside apply_post_response_outcome
        // (logged) — but the comprehension code path guards on
        // candidates.is_empty() before invoking classify, so classify
        // is never reached and no panic surfaces.
        dm.close_session().await;

        // No comprehension captures because classify was never called.
        let comp_captures = store.captured_comprehensions();
        assert!(
            comp_captures.is_empty(),
            "save_comprehensions must not be invoked when extraction is empty; got {:?}",
            comp_captures
        );
    }
}
