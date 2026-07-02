//! Step 3 of the `respond_to_streaming` decomposition: drive the
//! inference token stream, plus the context-limit recovery loop that
//! wraps it (progressive prompt-budget shrink on truncation).

use futures::StreamExt;
use primer_core::conversation::PedagogicalIntent;
use primer_core::error::Result;
use primer_core::inference::{GenerationParams, Prompt};

use crate::dialogue_manager::{DialogueManager, PromptBudgetTier};

/// Outcome of one streaming attempt: the accumulated text plus why the
/// stream ended (drives the context-limit retry decision).
pub(super) struct StreamOutcome {
    pub text: String,
    pub finish_reason: primer_core::inference::FinishReason,
}

impl DialogueManager {
    /// Step 3. Drive the inference backend's token stream into
    /// `on_chunk`, accumulating the full text for return. Mid-stream
    /// errors propagate as `Err(_)` — the orchestrator's "Ok-only"
    /// branches downstream then skip recording the Primer turn etc.
    ///
    /// `intent` and `passage_count` are threaded in from step 2 so that
    /// `GenerationParams.routing` is populated for every call. A
    /// `RouterBackend` reads these signals to make complexity-aware
    /// routing decisions; every other backend ignores them.
    ///
    /// Returns a `StreamOutcome` containing the accumulated text and the
    /// `FinishReason`, which the caller uses to decide whether to retry
    /// with a smaller prompt tier.
    async fn stream_inference_response<F>(
        &self,
        prompt: &Prompt,
        intent: PedagogicalIntent,
        passage_count: usize,
        on_chunk: &mut F,
    ) -> Result<StreamOutcome>
    where
        F: FnMut(&str),
    {
        let params = GenerationParams {
            routing: Some(primer_core::router::RoutingSignals {
                intent,
                retrieved_passages: passage_count,
            }),
            ..GenerationParams::default()
        };
        let mut stream = self.inference.generate_stream(prompt, &params).await?;

        let mut accumulated = String::new();
        let mut finish_reason = primer_core::inference::FinishReason::Stop;
        while let Some(item) = stream.next().await {
            let chunk = item.inspect_err(|e| {
                tracing::warn!("Stream error mid-generation: {e}");
            })?;
            if !chunk.text.is_empty() {
                on_chunk(&chunk.text);
                accumulated.push_str(&chunk.text);
            }
            if chunk.done {
                finish_reason = chunk.finish_reason;
                break;
            }
        }
        Ok(StreamOutcome {
            text: accumulated,
            finish_reason,
        })
    }

    /// Drive the context-limit recovery loop: stream at the current budget
    /// tier; on a `FinishReason::Length` truncation, stream the locale-aware
    /// apology and retry at the next tighter tier (up to
    /// [`crate::consts::MAX_TRUNCATION_RETRIES`] retries). When the tightest
    /// tier still truncates, stream the soft-stop cue and accept the partial.
    /// Returns the final accumulated text (the only text recorded as the
    /// Primer turn). A pre-stream error from any attempt propagates as `Err`.
    pub(super) async fn run_recovery_loop<F>(
        &self,
        child_input: &str,
        intent: PedagogicalIntent,
        on_chunk: &mut F,
    ) -> Result<String>
    where
        F: FnMut(&str),
    {
        use primer_core::inference::FinishReason;
        let sep = crate::consts::TURN_NOTICE_SEPARATOR;

        let mut tier = PromptBudgetTier::Full;
        // Most recent non-empty partial, kept as a fallback for the
        // exhaustion path: if the tightest tier truncates having produced
        // no visible text, recording an empty Primer turn would pollute the
        // session record, so we fall back to the last attempt that said
        // something. A clean (`Stop`) empty reply is left untouched — that
        // is the model genuinely answering nothing, pre-existing behaviour.
        let mut last_nonempty = String::new();
        loop {
            let (prompt, passage_count) = self.build_turn_prompt(child_input, intent, tier).await;
            let outcome = self
                .stream_inference_response(&prompt, intent, passage_count, on_chunk)
                .await?;
            if !outcome.text.is_empty() {
                last_nonempty = outcome.text.clone();
            }
            match (outcome.finish_reason, tier.next_tighter()) {
                (FinishReason::Stop, _) => return Ok(outcome.text),
                (FinishReason::Length, Some(next)) => {
                    let msg = self.prompt_pack.memory_limit_retry().to_string();
                    on_chunk(sep);
                    on_chunk(&msg);
                    on_chunk(sep);
                    tier = next;
                }
                (FinishReason::Length, None) => {
                    // Every tier truncated; accept the partial. Prefer the
                    // final attempt's text, falling back to the last
                    // non-empty partial so we never record an empty turn.
                    let text = if outcome.text.is_empty() {
                        last_nonempty
                    } else {
                        outcome.text
                    };
                    let msg = self.prompt_pack.memory_limit_soft_stop().to_string();
                    // No trailing separator: nothing follows the soft-stop
                    // cue (unlike the retry apology, which precedes the next
                    // attempt's text).
                    on_chunk(sep);
                    on_chunk(&msg);
                    return Ok(text);
                }
            }
        }
    }
}
