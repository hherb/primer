//! Step 5 of the `respond_to_streaming` decomposition: per-turn
//! persistence (session save + dirty-gated learner save) and the
//! fire-and-forget turn-embedding task.

use crate::dialogue_manager::DialogueManager;

impl DialogueManager {
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
    pub(super) async fn persist_turn(&mut self) {
        if let Some(ref store) = self.storage {
            let session_id = self.session.id;
            let turn_count = self.session.turns.len();
            tracing::debug!(
                target: "primer_pedagogy::persistence",
                session_id = %session_id,
                turn_count,
                "persist_turn: saving session"
            );
            match store.save_session(&self.session).await {
                Ok(()) => tracing::debug!(
                    target: "primer_pedagogy::persistence",
                    session_id = %session_id,
                    turn_count,
                    "persist_turn: save ok"
                ),
                Err(e) => tracing::warn!(
                    target: "primer_pedagogy::persistence",
                    session_id = %session_id,
                    turn_count,
                    error = %e,
                    "persist_turn: save failed"
                ),
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
}
