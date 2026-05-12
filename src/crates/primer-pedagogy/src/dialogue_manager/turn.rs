//! Per-turn hot path for the dialogue manager.
//!
//! Houses `respond_to` (thin wrapper) and `respond_to_streaming` (the
//! orchestrator) plus the seven private helper methods the orchestrator
//! decomposes into:
//!
//! - Step 0: `await_pending_background` (in `background.rs`) — drain
//!   previous turn's spawned tasks.
//! - Step 1: `record_child_turn` — push the Child `Turn`.
//! - Step 2: `build_turn_prompt` — retrieve_knowledge +
//!   retrieve_long_term_memory + build_prompt_with_pack.
//! - Step 3: `stream_inference_response` — drive the inference token
//!   stream into the caller's chunk callback.
//! - Step 4: `record_primer_turn` — push the Primer `Turn` with active
//!   concepts.
//! - Step 5: `persist_turn` — save session + (gated) save learner.
//! - Step 6a: `spawn_classification_task` — spawn the engagement classifier.
//! - Step 6b: `spawn_post_response_task` — spawn the chained extractor →
//!   comprehension task.
//!
//! Spawn order in step 6 is load-bearing on serialised backends (e.g.
//! single-instance Ollama with `OLLAMA_NUM_PARALLEL=1`): the classifier
//! gets admitted to the model queue first so it has a chance to
//! complete within its `blocking_timeout`. Documented in
//! docs/TODO/OPTIMIZING_LLM_REQUEST_ORDER.md.

use std::sync::Arc;

use chrono::Utc;
use futures::StreamExt;
use primer_core::classifier::EngagementAssessment;
use primer_core::conversation::{PedagogicalIntent, Speaker, Turn};
use primer_core::error::Result;
use primer_core::inference::{GenerationParams, Prompt};

use super::{DialogueManager, ExtractionPart, PostResponseResult};
use crate::prompt_builder;

impl DialogueManager {
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
        on_chunk: F,
    ) -> Result<String>
    where
        F: FnMut(&str),
    {
        // 0. Wait for the previous turn's classification + extraction (if any)
        //    to complete with bounded timeouts, then apply their results so
        //    decide_intent sees the updated engagement state and the system
        //    prompt sees freshly-extracted learner concepts.
        self.await_pending_background().await;

        // 1. Record the child's turn.
        self.record_child_turn(child_input);

        // 2. Decide intent and assemble the prompt (knowledge + long-term
        //    memory + system-prompt construction).
        let now = self.now();
        let break_gate = primer_core::session_timing::BreakGate {
            interval_minutes: self.config.break_suggest_after_minutes,
            last_suggested_at: self.last_break_suggested_at,
        };
        let intent = prompt_builder::decide_intent_at_with_pack(
            &*self.prompt_pack,
            &self.learner,
            &self.session,
            now,
            break_gate,
        );

        if intent == primer_core::conversation::PedagogicalIntent::SuggestBreak {
            self.last_break_suggested_at = Some(now);
        }

        let prompt = self.build_turn_prompt(child_input, intent).await;

        // 3. Stream the response, accumulating into a single String.
        let result = self.stream_inference_response(&prompt, on_chunk).await;

        // 4. On success, record the Primer turn, update the learner,
        //    and refresh the rolling summary if due.
        if let Ok(accumulated) = &result {
            self.record_primer_turn(accumulated, intent);
            self.update_learner_model(child_input, &intent);
            self.refresh_summary_if_due().await;
        }

        // 5. Save the session and learner if stores are configured. Runs on
        //    both Ok and Err paths. Save failures are logged, not propagated.
        self.persist_turn().await;

        // 6. Spawn the classifier and post-response (extractor →
        //    comprehension) tasks. Skipped on error paths — without a
        //    completed Primer response there is no exchange to assess
        //    and the partial Primer turn was dropped.
        //
        //    Spawn order is load-bearing on serialised backends (e.g.
        //    single-instance Ollama with OLLAMA_NUM_PARALLEL=1): the
        //    classifier spawns first so it gets admitted to the model
        //    queue first and has a chance to complete within its
        //    blocking_timeout. See
        //    docs/TODO/OPTIMIZING_LLM_REQUEST_ORDER.md.
        if result.is_ok() {
            self.spawn_classification_task();
            self.spawn_post_response_task();
        }

        result
    }

    // ─── respond_to_streaming decomposition (private helpers) ─────────

    /// Step 1. Push a Child `Turn` carrying `child_input` onto the
    /// session. No side effects beyond `session.add_turn`.
    fn record_child_turn(&mut self, child_input: &str) {
        self.session.add_turn(Turn {
            speaker: Speaker::Child,
            text: child_input.to_string(),
            timestamp: Utc::now(),
            intent: None,
            concepts: vec![],
        });
    }

    /// Step 2 (the assembly half). Retrieve the per-turn knowledge
    /// context and long-term memory, then hand them to the prompt
    /// builder along with the active intent. `decide_intent_with_pack`
    /// stays with the caller so the orchestrator can hold the intent
    /// for use in step 4.
    pub(super) async fn build_turn_prompt(
        &self,
        child_input: &str,
        intent: PedagogicalIntent,
    ) -> Prompt {
        let knowledge_context = self.retrieve_knowledge(child_input).await;
        let (summary, retrieved_older) = self.retrieve_long_term_memory(child_input).await;
        // Compute due-vocab once per turn. Wallclock dependency is
        // `chrono::Utc::now()` here — pure functions stay testable via
        // `now`-injection, but the production call site reads the system
        // clock. A future "fast-forward time for testing" mode would
        // override at this call site.
        let due_vocab = primer_core::vocab::due_concepts(
            &self.learner,
            chrono::Utc::now(),
            self.vocab_settings.max_per_prompt,
        );
        prompt_builder::build_prompt_with_pack_and_vocab(
            &*self.prompt_pack,
            &self.learner,
            &self.session,
            intent,
            &knowledge_context,
            &summary,
            &retrieved_older,
            self.config.context_window_turns,
            &due_vocab,
            self.config.break_suggest_after_minutes,
        )
    }

    /// Step 3. Drive the inference backend's token stream into
    /// `on_chunk`, accumulating the full text for return. Mid-stream
    /// errors propagate as `Err(_)` — the orchestrator's "Ok-only"
    /// branches downstream then skip recording the Primer turn etc.
    async fn stream_inference_response<F>(&self, prompt: &Prompt, mut on_chunk: F) -> Result<String>
    where
        F: FnMut(&str),
    {
        let params = GenerationParams::default();
        let mut stream = self.inference.generate_stream(prompt, &params).await?;

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

    /// Step 4. Compute the active concepts for the just-completed
    /// exchange and push the Primer `Turn`. Empty `text` is logged
    /// (rare; signals a backend that finished without emitting any
    /// chunks) but still recorded so the turn-pair invariant for the
    /// post-response task holds.
    fn record_primer_turn(&mut self, text: &str, intent: PedagogicalIntent) {
        if text.is_empty() {
            tracing::warn!("Inference stream produced no text");
        }
        let active_concepts = prompt_builder::extract_active_concepts(
            &self.session,
            crate::consts::ACTIVE_CONCEPT_LOOKBACK,
        );
        self.session.add_turn(Turn {
            speaker: Speaker::Primer,
            text: text.to_string(),
            timestamp: Utc::now(),
            intent: Some(intent),
            concepts: active_concepts,
        });
    }

    /// Step 5. Save the session unconditionally (when storage is set);
    /// save the learner only when `learner_dirty` (gating per-turn
    /// SQLite write transactions). Lifecycle events save the learner
    /// unconditionally — that path is in `lifecycle.rs`.
    ///
    /// When an embedder is configured, also fire-and-forget a task that
    /// embeds the most-recent (child, primer) turns and stores their
    /// vectors. The task is detached: hybrid retrieval will pick up the
    /// vectors whenever they finish, and a not-yet-embedded recent turn
    /// is still inside the context window so the model already sees it
    /// directly. Failures `tracing::warn!` and never block.
    async fn persist_turn(&mut self) {
        if let Some(ref store) = self.storage {
            if let Err(e) = store.save_session(&self.session).await {
                tracing::warn!("session save failed: {e}");
            }
        }
        if self.learner_dirty {
            if let Some(ref ls) = self.learner_store {
                if let Err(e) = ls.save_learner(&self.learner).await {
                    tracing::warn!("learner save failed (per-turn): {e}");
                } else {
                    self.learner_dirty = false;
                }
            }
        }
        self.spawn_embedding_task();
    }

    /// Spawn a fire-and-forget embedding task for the most-recent
    /// (child, primer) exchange. Idempotent at the storage layer
    /// (`save_turn_embedding` upserts), so re-running over already-
    /// embedded turns is a no-op write.
    fn spawn_embedding_task(&self) {
        let (Some(store), Some(embedder)) = (self.storage.clone(), self.embedder.clone()) else {
            return;
        };
        let session_id = self.session.id;
        let total = self.session.turns.len();
        if total == 0 {
            return;
        }
        // Embed up to the last two turns. Most respond_to_streaming
        // calls have appended both a child and a primer turn; some
        // open_session paths produce only a primer greeting.
        let start = total.saturating_sub(2);
        let texts_with_idx: Vec<(usize, String)> = self.session.turns[start..]
            .iter()
            .enumerate()
            .map(|(rel, t)| (start + rel, t.text.clone()))
            .collect();
        tokio::spawn(async move {
            let texts: Vec<&str> = texts_with_idx.iter().map(|(_, t)| t.as_str()).collect();
            let vecs = match embedder.embed(&texts).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("embedding task: embed failed: {e}");
                    return;
                }
            };
            for ((idx, _), v) in texts_with_idx.iter().zip(vecs.into_iter()) {
                if let Err(e) = store
                    .save_turn_embedding(session_id, *idx, embedder.model_id(), embedder.dim(), &v)
                    .await
                {
                    tracing::warn!("embedding task: save turn {idx}: {e}");
                }
            }
        });
    }

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
    fn spawn_classification_task(&mut self) {
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
    fn spawn_post_response_task(&mut self) {
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
