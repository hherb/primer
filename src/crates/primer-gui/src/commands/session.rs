//! Session lifecycle commands: start, close, query.
//!
//! `start_session` builds an [`ActiveSession`] from the persisted
//! `GuiConfig` and stores it in `AppState`. `close_session` drops it.
//! `current_session_info` returns a serialisable summary so the
//! frontend can render its header.
//!
//! `send_message` (the streaming command that drives a turn through
//! `DialogueManager`) lands in step 4. `resume_session` + `list_sessions`
//! land in step 9 alongside the picker.

use std::sync::Arc;

use primer_pedagogy::{DialogueManager, DialogueManagerStores, DialogueManagerSubsystems};
use tauri::{AppHandle, Emitter};

use crate::state::{ActiveSession, AppState};
use crate::types::{ChunkEvent, LearnerSummary, SessionInfo, TurnComplete};
use crate::wiring;

/// Construct an `ActiveSession` from the persisted settings and store
/// it in `AppState`. Errors surface as `String` for inline rendering.
///
/// If a session is already open, it is closed first (no resource leak
/// even if the frontend forgets to close before re-starting). The
/// previous learner's state is saved before the new session opens.
#[tauri::command]
pub async fn start_session(state: tauri::State<'_, AppState>) -> Result<SessionInfo, String> {
    // Close any pre-existing session first so the Arcs from the old
    // construction drop before we build the new ones.
    close_session_inner(&state).await?;

    let cfg = state.config.lock().await.clone();
    let active = wiring::build_active_session(&state.home, &cfg).await?;
    let info = info_from(&active).await;
    *state.session.lock().await = Some(active);
    Ok(info)
}

/// Drop the active session, if any. Idempotent — calling it with no
/// active session is a no-op (returns Ok).
///
/// Persists the current learner state on the way out so the next
/// `start_session` (or the next CLI run) sees the latest box-levels
/// / concept counts / engagement history.
#[tauri::command]
pub async fn close_session(state: tauri::State<'_, AppState>) -> Result<(), String> {
    close_session_inner(&state).await
}

/// Return a summary of the active session, or `None` if no session is
/// open. Used by the frontend on launch to decide whether to render
/// the picker or the chat view.
#[tauri::command]
pub async fn current_session_info(
    state: tauri::State<'_, AppState>,
) -> Result<Option<SessionInfo>, String> {
    let guard = state.session.lock().await;
    if let Some(active) = guard.as_ref() {
        Ok(Some(info_from(active).await))
    } else {
        Ok(None)
    }
}

/// Internal helper used by both `close_session` and `start_session` so
/// the persistence-then-drop flow is centralised. Lock is released
/// before the (potentially I/O-heavy) `save_learner` call so other
/// commands aren't blocked on slow disk.
async fn close_session_inner(state: &tauri::State<'_, AppState>) -> Result<(), String> {
    let to_save = {
        let mut guard = state.session.lock().await;
        if let Some(active) = guard.take() {
            let snapshot = active.learner.lock().await.clone();
            Some((std::sync::Arc::clone(&active.learner_store), snapshot))
        } else {
            None
        }
    };
    if let Some((store, snapshot)) = to_save {
        if let Err(e) = store.save_learner(&snapshot).await {
            tracing::warn!("save_learner on close failed: {e}");
        }
    }
    Ok(())
}

/// Drive one streaming dialogue turn end-to-end.
///
/// Flow:
/// 1. Construct a fresh `DialogueManager` from the active session's
///    Arcs (per the plan, DM is per-command rather than long-lived).
/// 2. If this isn't the very first turn, load the existing `Session`
///    from disk and `resume_session` so the timeline + summary +
///    engagement history carry over.
/// 3. Run `respond_to_streaming`, emitting one `primer://chunk` event
///    per token. The frontend reassembles the full response from the
///    chunk stream — no String accumulation is needed server-side.
/// 4. `close_session` to drain background tasks (classifier / extractor
///    / comprehension), mark `ended_at`, and save. See #74 for the
///    follow-up that moves this drain off the critical path.
/// 5. Write the updated `LearnerModel` and the now-stable `session_id`
///    back into `ActiveSession`. The session_id writeback also runs on
///    the error path so a mid-stream failure on the first turn doesn't
///    orphan its (saved-to-disk) child turn under a fresh UUID on retry.
/// 6. Emit `primer://turn_complete` with the bare-essentials payload.
///
/// **Concurrency.** The session mutex is held for the entire turn —
/// concurrent `send_message` calls would race over DM construction
/// and `LearnerModel` ownership. Other commands that don't lock
/// `state.session` (e.g. `get_settings`) keep running.
#[tauri::command]
pub async fn send_message(
    state: tauri::State<'_, AppState>,
    app: AppHandle,
    input: String,
) -> Result<TurnComplete, String> {
    let mut guard = state.session.lock().await;
    let active = guard
        .as_mut()
        .ok_or_else(|| "no active session — call start_session first".to_string())?;

    run_turn(active, &input, |primer_turn_index, chunk| {
        let payload = ChunkEvent {
            primer_turn_index,
            text: chunk.to_string(),
        };
        if let Err(e) = app.emit("primer://chunk", &payload) {
            tracing::warn!("emit primer://chunk failed: {e}");
        }
    })
    .await
    .inspect(|payload| {
        if let Err(e) = app.emit("primer://turn_complete", payload) {
            tracing::warn!("emit primer://turn_complete failed: {e}");
        }
    })
}

/// Drive one streaming turn end-to-end against an `ActiveSession`.
///
/// Split out from `send_message` so unit tests can exercise the full
/// DM construction / resume / respond / close / writeback flow
/// without a Tauri `AppHandle` in scope.
///
/// `on_chunk` is invoked once per streamed token with the Primer-turn
/// index and the chunk text. Callers wire it to whatever delivery
/// channel they want (Tauri events in production, a `Vec<String>`
/// accumulator in tests).
async fn run_turn<F>(
    active: &mut ActiveSession,
    input: &str,
    mut on_chunk: F,
) -> Result<TurnComplete, String>
where
    F: FnMut(usize, &str),
{
    // Clone the learner — DM takes ownership; we put the mutated copy
    // back into the active session on return.
    let learner = active.learner.lock().await.clone();
    let previous_session_id = active.session_id;

    // Bundle subsystems + stores DM expects, cloning the Arcs.
    let stores = DialogueManagerStores {
        session: Some(Arc::clone(&active.session_store)),
        learner: Some(Arc::clone(&active.learner_store)),
    };
    let subsystems = DialogueManagerSubsystems {
        classifier: Arc::clone(&active.classifier),
        classifier_settings: active.classifier_settings.clone(),
        extractor: Arc::clone(&active.extractor),
        extractor_settings: active.extractor_settings.clone(),
        comprehension: Arc::clone(&active.comprehension),
        comprehension_settings: active.comprehension_settings.clone(),
        vocab_settings: active.vocab_settings,
        embedder: active.embedder.clone(),
    };

    // Construct DM. `inference` and `knowledge` are borrowed from the
    // Arcs we keep alive on the stack; DM's `&'a dyn` lifetime is
    // tied to this function's scope.
    let inference = active.backend.as_ref();
    let knowledge = active.knowledge.as_ref();
    let mut dm = DialogueManager::new(
        learner,
        inference,
        knowledge,
        stores,
        subsystems,
        active.pedagogy_config.clone(),
    );

    // Resume the prior Session if this isn't the first turn.
    if let Some(prev_id) = previous_session_id {
        match active.session_store.load_session(prev_id).await {
            Ok(Some(loaded)) => {
                dm.resume_session(loaded)
                    .await
                    .map_err(|e| format!("resume_session({prev_id}): {e}"))?;
            }
            Ok(None) => {
                return Err(format!(
                    "session {prev_id} missing from store; the file may have been deleted"
                ));
            }
            Err(e) => return Err(format!("load_session({prev_id}): {e}")),
        }
    }

    // The Primer turn lands at `turns.len() + 1` after the child's
    // turn appends at `turns.len()`. Captured before respond_to_streaming
    // so the chunk callback can address its bubble.
    let primer_turn_index = dm.session.turns.len() + 1;

    let response_result = dm
        .respond_to_streaming(input, |chunk| on_chunk(primer_turn_index, chunk))
        .await;

    // close_session drains the background classifier / extractor /
    // comprehension tasks so their results land on disk AND in
    // dm.learner before we extract it. Idempotent — safe even on a
    // mid-stream error.
    dm.close_session().await;

    // Stamp the session_id back into ActiveSession on BOTH paths.
    // respond_to_streaming records the child turn before the inference
    // call (step 1 of its flow) and persist_turn saves the partial
    // session to disk regardless of stream outcome. Without writing
    // session_id back on the error path, a mid-stream failure on the
    // first turn would orphan that on-disk Session: the next
    // send_message would see `previous_session_id = None` and mint a
    // fresh UUID, leaving the failed child turn stranded.
    let session_id = dm.session.id;
    active.session_id = Some(session_id);

    response_result.map_err(|e| e.to_string())?;

    // Learner writeback only on success — respond_to_streaming's
    // contract is that the learner model is not updated on a mid-stream
    // error (the partial Primer turn was dropped).
    *active.learner.lock().await = dm.learner;

    Ok(TurnComplete {
        session_id,
        child_turn_index: primer_turn_index - 1,
        primer_turn_index,
    })
}

async fn info_from(active: &ActiveSession) -> SessionInfo {
    let learner = active.learner.lock().await;
    SessionInfo {
        session_id: active.session_id,
        learner: LearnerSummary {
            id: learner.profile.id,
            name: learner.profile.name.clone(),
            age: learner.profile.age,
            concept_count: learner.concepts.len(),
        },
        backend_kind: active.backend_name.clone(),
        main_model: active.main_model.clone(),
        locale: active.locale.pack_id().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GuiConfig;
    use crate::wiring::build_active_session;
    use tempfile::TempDir;

    fn stub_config_with_persistence(home: &std::path::Path) -> GuiConfig {
        // Persist to a real on-disk session DB so the second turn's
        // resume_session path is exercised against actual storage.
        let mut cfg = GuiConfig::default();
        cfg.persistence.session_db = Some(home.join("test_session.db"));
        cfg
    }

    #[tokio::test]
    async fn first_turn_creates_session_and_returns_payload() {
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let mut active = build_active_session(home.path(), &cfg).await.unwrap();

        assert!(active.session_id.is_none(), "no session_id before first turn");

        let mut chunks = Vec::<(usize, String)>::new();
        let payload = run_turn(&mut active, "hello", |i, c| chunks.push((i, c.to_string())))
            .await
            .unwrap();

        assert!(active.session_id.is_some(), "session_id set after first turn");
        assert_eq!(active.session_id.unwrap(), payload.session_id);
        assert_eq!(payload.child_turn_index, 0, "child lands at index 0");
        assert_eq!(payload.primer_turn_index, 1, "primer lands at index 1");
        assert!(!chunks.is_empty(), "stub backend emits at least one chunk");
        // Every chunk must address the correct primer turn.
        for (idx, _) in &chunks {
            assert_eq!(*idx, payload.primer_turn_index);
        }
    }

    #[tokio::test]
    async fn second_turn_resumes_same_session() {
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let mut active = build_active_session(home.path(), &cfg).await.unwrap();

        let first = run_turn(&mut active, "hello", |_, _| {}).await.unwrap();
        let second = run_turn(&mut active, "tell me more", |_, _| {}).await.unwrap();

        assert_eq!(
            first.session_id, second.session_id,
            "second turn resumes the same Session — no orphan ids"
        );
        assert_eq!(second.child_turn_index, 2, "child #2 lands after first exchange");
        assert_eq!(second.primer_turn_index, 3, "primer #2 lands at index 3");
    }

    #[tokio::test]
    async fn turn_persists_to_session_store() {
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let mut active = build_active_session(home.path(), &cfg).await.unwrap();
        let store = Arc::clone(&active.session_store);

        let payload = run_turn(&mut active, "what is curiosity?", |_, _| {})
            .await
            .unwrap();

        // Verify the session is round-trippable through the on-disk store.
        let loaded = store
            .load_session(payload.session_id)
            .await
            .unwrap()
            .expect("session must be loadable after first turn");
        assert!(
            loaded.turns.len() >= 2,
            "session must persist both the child and primer turns; got {} turns",
            loaded.turns.len()
        );
    }
}
