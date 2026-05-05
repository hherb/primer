//! Session lifecycle methods on `DialogueManager`: construction, opening,
//! resuming, closing, plus the small public accessors used by `--verbose`.
//!
//! The three lifecycle events (open / resume / close) all unconditionally
//! save the session and learner if stores are configured — unlike the
//! per-turn save site which gates on `learner_dirty`. Save failures emit
//! `tracing::warn!` rather than propagating, matching the soft-fail
//! posture used elsewhere.
//!
//! Break-suggestion timing decisions live in `decide_intent_at_with_pack`
//! — see `primer_core::session_timing` for the pure helper.

use chrono::Utc;

use primer_core::config::PedagogyConfig;
use primer_core::conversation::{PedagogicalIntent, Session, Speaker, Turn};
use primer_core::error::Result;
use primer_core::inference::InferenceBackend;
use primer_core::knowledge::KnowledgeBase;
use primer_core::learner::LearnerModel;

use super::{DialogueManager, DialogueManagerStores, DialogueManagerSubsystems};
use crate::prompt_pack;

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
            vocab_settings: subsystems.vocab_settings,
            last_comprehension: None,
            last_break_suggested_at: None,
            #[cfg(test)]
            clock_override: None,
            config,
            last_extraction: None,
            learner_dirty: false,
            prompt_pack,
            embedder: subsystems.embedder,
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
        self.last_break_suggested_at = None;
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
}
