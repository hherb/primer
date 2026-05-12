//! Background-task drain & apply machinery for the dialogue manager.
//!
//! Two tasks are spawned at the end of each turn:
//!
//! 1. **Engagement classifier** — assesses the child's engagement on
//!    the just-completed exchange. Result: `Option<EngagementAssessment>`.
//! 2. **Post-response chain** — extractor → comprehension. Result:
//!    `Option<PostResponseResult>` carrying both extraction and
//!    comprehension outputs along with the turn indices needed to sync
//!    in-memory state.
//!
//! At the start of the next turn, both tasks are awaited concurrently
//! via `tokio::join!` (cap on wallclock = `max(classifier_timeout,
//! extractor_timeout + comprehension_timeout)`). On timeout, the
//! classifier task is aborted (its result is point-in-time and stale
//! data is worse than no data); the post-response chain is detached
//! (its tokio::spawn'd DB writes still complete in the background).
//!
//! Soft-fail policy throughout — a failed background task emits
//! `tracing::warn!` and leaves prior in-memory state unchanged. Never
//! propagates up.

use super::{
    ClassificationOutcome, DialogueManager, PostResponseOutcome, apply_assessment,
    apply_comprehension, apply_extraction, merge_concepts_into_turn,
};

impl DialogueManager {
    /// Drain only the classifier task and apply its outcome.
    ///
    /// Production paths use `await_pending_background` (which drains both
    /// background tasks in parallel); this focused variant exists for unit
    /// tests that exercise classifier behaviour in isolation without
    /// setting up a post-response chain.
    #[cfg(test)]
    pub(super) async fn await_pending_classification(&mut self) {
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
    pub(super) async fn await_pending_background(&mut self) {
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
}
