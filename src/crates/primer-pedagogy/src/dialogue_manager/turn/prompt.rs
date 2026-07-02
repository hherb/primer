//! Turn recording + prompt assembly: steps 1, 2 (assembly half), and 4
//! of the `respond_to_streaming` decomposition.

use chrono::Utc;
use primer_core::conversation::{PedagogicalIntent, Speaker, Turn};
use primer_core::inference::Prompt;

use crate::dialogue_manager::{DialogueManager, PromptBudgetTier};
use crate::prompt_builder;

impl DialogueManager {
    /// Step 1. Push a Child `Turn` carrying `child_input` onto the
    /// session. No side effects beyond `session.add_turn`.
    pub(super) fn record_child_turn(&mut self, child_input: &str) {
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
    ///
    /// Returns `(prompt, passage_count)` where `passage_count` is the
    /// number of knowledge passages retrieved, used to populate
    /// `GenerationParams.routing` in step 3 so a `RouterBackend` can
    /// make complexity-aware routing decisions.
    pub(in crate::dialogue_manager) async fn build_turn_prompt(
        &self,
        child_input: &str,
        intent: PedagogicalIntent,
        tier: PromptBudgetTier,
    ) -> (Prompt, usize) {
        let knowledge_context = if tier.includes_knowledge() {
            self.retrieve_knowledge(child_input).await
        } else {
            Vec::new()
        };
        // Reflects what retrieval matched (the routing complexity signal),
        // independent of any small-context truncation/budget drop below.
        let passage_count = knowledge_context.len();
        let (summary, retrieved_older) = if tier.includes_long_term_memory() {
            self.retrieve_long_term_memory(child_input).await
        } else {
            (String::new(), Vec::new())
        };
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
        let backend_name = self.inference.name();
        let base_window = self.config.effective_context_window_turns(backend_name);
        let context_turns = tier.context_window_turns(base_window);
        let break_minutes = self.config.break_suggest_after_minutes;

        // Small-context backends (the Qualcomm NPU `QnnBackend` runs a
        // 2048-token Genie context) get a token-budgeted assembly: each KB
        // passage is truncated to its relevant lead, and the prompt builder
        // drops the lowest-value optional sections to keep the system prompt
        // under budget — leaving room for the reply. The pedagogical core
        // (Socratic base prompt + intent) is never trimmed. Large-context
        // backends keep the unbounded path, byte-for-byte as before.
        let prompt = if primer_core::backend::is_small_context_backend(backend_name) {
            use primer_core::consts::prompt_budget as pb;
            let truncated = prompt_builder::truncate_passages(
                &knowledge_context,
                pb::KB_PASSAGE_MAX_TOKENS_SMALL_CONTEXT,
            );
            prompt_builder::build_prompt_within_budget_with_pack_and_vocab(
                &*self.prompt_pack,
                &self.learner,
                &self.session,
                intent,
                &truncated,
                &summary,
                &retrieved_older,
                context_turns,
                &due_vocab,
                break_minutes,
                pb::SYSTEM_PROMPT_BUDGET_TOKENS_SMALL_CONTEXT,
            )
        } else {
            prompt_builder::build_prompt_with_pack_and_vocab(
                &*self.prompt_pack,
                &self.learner,
                &self.session,
                intent,
                &knowledge_context,
                &summary,
                &retrieved_older,
                context_turns,
                &due_vocab,
                break_minutes,
            )
        };
        (prompt, passage_count)
    }

    /// Step 4. Compute the active concepts for the just-completed
    /// exchange and push the Primer `Turn`. Empty `text` is logged
    /// (rare; signals a backend that finished without emitting any
    /// chunks) but still recorded so the turn-pair invariant for the
    /// post-response task holds.
    pub(super) fn record_primer_turn(&mut self, text: &str, intent: PedagogicalIntent) {
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
}
