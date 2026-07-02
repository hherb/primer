//! Step 6 of the `respond_to_streaming` decomposition: spawn the
//! post-response background tasks — 6a the engagement classifier, 6b
//! the chained extractor → comprehension task. Both self-persist and
//! hand their results back through `JoinHandle`s drained at the start
//! of the next turn (see `background.rs`).

use std::sync::Arc;

use primer_core::classifier::EngagementAssessment;
use primer_core::conversation::{Speaker, Turn};

use crate::dialogue_manager::{DialogueManager, ExtractionPart, PostResponseResult};

impl DialogueManager {
    /// Step 6a. Spawn the engagement classifier on the just-completed
    /// child turn. The spawned task self-persists to
    /// `turn_classifications`; its `Option<EngagementAssessment>`
    /// result is drained at the start of the next turn (see
    /// `await_pending_background` in `background.rs`) so the next
    /// turn's `decide_intent` sees fresh engagement state.
    ///
    /// All inputs are owned at spawn time (the closure must satisfy
    /// `'static`); cloning the small `Vec<Turn>` and
    /// `Vec<EngagementAssessment>` is cheap relative to the LLM call
    /// the spawned task is about to make.
    pub(super) fn spawn_classification_task(&mut self) {
        let Some(child_idx) = self
            .session
            .turns
            .iter()
            .enumerate()
            .rev()
            .find(|(_, t)| t.speaker == Speaker::Child)
            .map(|(i, _)| i)
        else {
            return;
        };

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
        let prior_assessments: Vec<EngagementAssessment> = self.learner.recent_assessments.clone();

        // Latency instrumentation (Phase 1, see
        // docs/TODO/OPTIMIZING_LLM_REQUEST_ORDER.md). Owned identifier
        // string and pre-spawn instant captured before tokio::spawn so
        // the closure can compute queued_ms without borrowing self.
        let classifier_id = classifier.identifier().to_string();
        let classifier_pre_spawn = std::time::Instant::now();

        let task = tokio::spawn(async move {
            let task_start = std::time::Instant::now();
            let queued_ms = task_start.duration_since(classifier_pre_spawn).as_millis() as u64;
            let ctx = primer_core::classifier::EngagementContext {
                recent_child_turns: &recent_child_turns,
                prior_assessments: &prior_assessments,
            };
            let outcome = match classifier.classify(ctx).await {
                Ok(a) => {
                    if let Some(store) = store {
                        if let Err(e) = store
                            .save_classification(session_id, child_idx, &a, classifier.identifier())
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

    /// Step 6b. Spawn the chained extractor → comprehension task on
    /// the just-completed (child, primer) exchange. The task self-
    /// persists to `turn_concepts` and `turn_comprehensions`; its
    /// `Option<PostResponseResult>` carries the indices and outputs
    /// needed at the next turn's `await_pending_background` to sync
    /// in-memory state.
    ///
    /// Skipped if the trailing two turns aren't the expected
    /// (Child, Primer) pair — defensive guard against the orchestrator
    /// ever invoking this when a turn pair didn't actually complete.
    pub(super) fn spawn_post_response_task(&mut self) {
        let total_turns = self.session.turns.len();
        if total_turns < 2
            || self.session.turns[total_turns - 1].speaker != Speaker::Primer
            || self.session.turns[total_turns - 2].speaker != Speaker::Child
        {
            return;
        }

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
