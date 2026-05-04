//! Knowledge-base RAG retrieval and session-store long-term memory
//! retrieval helpers for the dialogue manager.
//!
//! Both helpers are best-effort: failures from the underlying store
//! return empty results and emit a `tracing::warn!` rather than
//! propagating, so a flaky knowledge or storage backend never blocks
//! the conversation turn.

use primer_core::conversation::Turn;
use primer_core::knowledge::{Passage, RetrievalParams};

use super::DialogueManager;

impl<'a> DialogueManager<'a> {
    /// Retrieve knowledge passages relevant to the child's input.
    /// Falls back gracefully if the knowledge base is empty or errors.
    pub(super) async fn retrieve_knowledge(&self, query: &str) -> Vec<Passage> {
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
    pub(super) async fn retrieve_long_term_memory(&self, child_input: &str) -> (String, Vec<Turn>) {
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
}
