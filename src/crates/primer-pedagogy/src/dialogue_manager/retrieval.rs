//! Knowledge-base RAG retrieval and session-store long-term memory
//! retrieval helpers for the dialogue manager.
//!
//! Both helpers are best-effort: failures from the underlying store
//! return empty results and emit a `tracing::warn!` rather than
//! propagating, so a flaky knowledge or storage backend never blocks
//! the conversation turn.

use primer_core::consts::retrieval as r;
use primer_core::conversation::Turn;
use primer_core::knowledge::{HybridParams, Passage, RetrievalParams};

use super::DialogueManager;

fn kb_hybrid_params(final_top_k: usize) -> HybridParams {
    HybridParams {
        bm25_top_k: r::KB_BM25_TOP_K,
        vector_top_k: r::KB_VECTOR_TOP_K,
        final_top_k,
        rrf_k: r::RRF_K,
        min_score: r::KB_MIN_SCORE,
        source_filter: vec![],
    }
}

fn ltm_hybrid_params() -> HybridParams {
    HybridParams {
        bm25_top_k: r::LTM_BM25_TOP_K,
        vector_top_k: r::LTM_VECTOR_TOP_K,
        final_top_k: r::LTM_FINAL_TOP_K,
        rrf_k: r::RRF_K,
        // Long-term-memory hybrid path keeps everything in the fused
        // list (no floor): turns are session-internal context, not
        // ground-truth passages, and we'd rather see something old
        // than nothing.
        min_score: f64::NEG_INFINITY,
        source_filter: vec![],
    }
}

impl DialogueManager {
    /// Retrieve knowledge passages relevant to the child's input.
    /// Falls back gracefully if the knowledge base is empty or errors.
    ///
    /// When an embedder is configured (`self.embedder = Some`), uses
    /// hybrid BM25 + vector retrieval via Reciprocal Rank Fusion.
    /// Otherwise falls back to BM25-only — exactly today's behaviour.
    /// A hybrid retrieval failure is logged and falls back to BM25 too.
    pub(super) async fn retrieve_knowledge(&self, query: &str) -> Vec<Passage> {
        // Per-backend KB budget: small-context backends ("qnn:…") retrieve
        // fewer passages so the system prompt fits a 4K-token window
        // (step 1.2.5). Non-qnn backends keep the global KB_FINAL_TOP_K.
        let kb_top_k = self.config.effective_kb_top_k(self.inference.name());
        if let Some(ref embedder) = self.embedder {
            let params = kb_hybrid_params(kb_top_k);
            match self
                .knowledge
                .retrieve_hybrid(query, embedder.as_ref(), &params)
                .await
            {
                Ok(p) => return p,
                Err(e) => {
                    tracing::warn!("hybrid knowledge retrieval failed, falling back to BM25: {e}");
                }
            }
        }
        let params = RetrievalParams {
            top_k: kb_top_k,
            min_score: r::KB_BM25_ONLY_MIN_SCORE,
            source_filter: vec![],
        };
        self.knowledge
            .retrieve(query, &params)
            .await
            .unwrap_or_default()
    }

    /// Pull long-term memory for the current turn: the rolling summary
    /// of pre-window turns plus the top-K older turns that the index
    /// considers relevant to `child_input`. With an embedder configured,
    /// "the index" is hybrid (BM25 + vector RRF); otherwise BM25-only.
    ///
    /// Both pieces are empty when the session is still inside its first
    /// context window, when no store is configured, or when retrieval
    /// returns no matches. Errors from the store are logged and treated
    /// as "no retrieved turns" — long-term memory is best-effort.
    pub(super) async fn retrieve_long_term_memory(&self, child_input: &str) -> (String, Vec<Turn>) {
        let total = self.session.turns.len();
        let window = self
            .config
            .effective_context_window_turns(self.inference.name());
        if total <= window {
            return (String::new(), vec![]);
        }
        let exclude_at_or_after = total - window;
        let retrieved = match self.storage.as_deref() {
            None => vec![],
            Some(store) => match self.embedder.as_ref() {
                Some(embedder) => {
                    let params = ltm_hybrid_params();
                    store
                        .retrieve_session_turns_hybrid(
                            self.session.id,
                            child_input,
                            embedder.as_ref(),
                            &params,
                            exclude_at_or_after,
                        )
                        .await
                        .unwrap_or_else(|e| {
                            tracing::warn!("hybrid session retrieval failed: {e}");
                            vec![]
                        })
                }
                None => store
                    .retrieve_session_turns(
                        self.session.id,
                        child_input,
                        r::LTM_FINAL_TOP_K,
                        exclude_at_or_after,
                    )
                    .await
                    .unwrap_or_else(|e| {
                        tracing::warn!("session-turn retrieval failed: {e}");
                        vec![]
                    }),
            },
        };
        (self.session.summary.clone(), retrieved)
    }
}
