//! Per-turn hot path for the dialogue manager.
//!
//! Houses `respond_to` (thin wrapper) and `respond_to_streaming` (the
//! orchestrator, in this file) plus the private helper methods the
//! orchestrator decomposes into, split by responsibility:
//!
//! - Step 0: `await_pending_background` (in `background.rs`) — drain
//!   previous turn's spawned tasks.
//! - Step 1: `record_child_turn` (`prompt.rs`) — push the Child `Turn`.
//! - Step 2: `build_turn_prompt` (`prompt.rs`) — retrieve_knowledge +
//!   retrieve_long_term_memory + build_prompt_with_pack.
//! - Step 3: `stream_inference_response` + `run_recovery_loop`
//!   (`stream.rs`) — drive the inference token stream into the caller's
//!   chunk callback, with context-limit truncation recovery.
//! - Step 4: `record_primer_turn` (`prompt.rs`) — push the Primer `Turn`
//!   with active concepts.
//! - Step 5: `persist_turn` (`persist.rs`) — save session + (gated) save
//!   learner + fire-and-forget turn embedding.
//! - Step 6a: `spawn_classification_task` (`spawn_tasks.rs`) — spawn the
//!   engagement classifier.
//! - Step 6b: `spawn_post_response_task` (`spawn_tasks.rs`) — spawn the
//!   chained extractor → comprehension task.
//!
//! Spawn order in step 6 is load-bearing on serialised backends (e.g.
//! single-instance Ollama with `OLLAMA_NUM_PARALLEL=1`): the classifier
//! gets admitted to the model queue first so it has a chance to
//! complete within its `blocking_timeout`. Documented in
//! docs/TODO/OPTIMIZING_LLM_REQUEST_ORDER.md.

mod persist;
mod prompt;
mod spawn_tasks;
mod stream;

use primer_core::error::Result;

use super::DialogueManager;
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
        mut on_chunk: F,
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

        // 2+3. Progressive-shrink recovery loop: build the prompt at the
        // current budget tier, stream the reply, and on a context-limit
        // truncation notify the child and retry with a smaller prompt.
        // Only the final answer is returned (and later recorded); partial
        // attempts before a successful retry are dropped. The child turn
        // was already recorded once above, so retries don't duplicate it.
        let result = self
            .run_recovery_loop(child_input, intent, &mut on_chunk)
            .await;

        // 4. On success, record the Primer turn, update the learner,
        //    and refresh the rolling summary if due. The break gate is
        //    stamped here rather than at intent selection: if the turn
        //    errors, the child never heard the break suggestion, and
        //    stamping early would suppress any re-suggestion for a full
        //    interval (default 30 min).
        if let Ok(accumulated) = &result {
            if intent == primer_core::conversation::PedagogicalIntent::SuggestBreak {
                self.last_break_suggested_at = Some(now);
            }
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
}
