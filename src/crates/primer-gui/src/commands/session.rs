//! Session lifecycle + per-turn streaming commands.
//!
//! `start_session` builds an [`ActiveSession`] (which carries a
//! long-lived [`DialogueManager`]) from the persisted `GuiConfig` and
//! stores it in `AppState`. `close_session` drops it, draining any
//! in-flight background tasks first via `dm.close_session()`.
//!
//! `send_message` clones the DM Arc out of the session guard, releases
//! the session guard, then locks the DM independently for the duration
//! of the turn — so the long streaming wallclock doesn't keep other
//! commands (e.g. `current_session_info`) waiting.

use std::sync::Arc;

use primer_pedagogy::DialogueManager;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::state::{ActiveSession, AppState};
use crate::types::{ChunkEvent, LearnerSummary, SessionInfo, TurnComplete};
use crate::wiring;

/// Construct an `ActiveSession` from the persisted settings and store
/// it in `AppState`. Errors surface as `String` for inline rendering.
///
/// If a session is already open, it is closed first (no resource leak
/// even if the frontend forgets to close before re-starting). The
/// previous learner's state is saved as part of `close_session`'s
/// internal drain.
#[tauri::command]
pub async fn start_session(state: tauri::State<'_, AppState>) -> Result<SessionInfo, String> {
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
/// Drains the DM's background tasks (classifier / extractor /
/// comprehension) before drop so the final turn's analysis lands on
/// disk before the Arcs are released.
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

/// Internal helper used by both `close_session` and `start_session`.
///
/// Two-step lock dance: pop the `ActiveSession` out of the session
/// mutex first (so other commands aren't blocked while DM drain runs),
/// then lock the DM mutex and call `close_session` on it. The DM mutex
/// lock will WAIT for any in-flight `send_message` to finish — exactly
/// the right behaviour so a "Close" click never aborts a partially-
/// streamed response.
async fn close_session_inner(state: &tauri::State<'_, AppState>) -> Result<(), String> {
    let active = state.session.lock().await.take();
    if let Some(active) = active {
        let mut dm = active.dialogue_manager.lock().await;
        dm.close_session().await;
    }
    Ok(())
}

/// Drive one streaming dialogue turn end-to-end.
///
/// Flow:
/// 1. Clone the DM Arc out of the session guard and release the
///    session guard. Other commands (`current_session_info`,
///    `update_settings`, …) keep running for the duration of the turn.
/// 2. Lock the DM mutex. Concurrent `send_message` calls serialise
///    here — there can only be one in-flight turn at a time, which is
///    what the pedagogy crate expects.
/// 3. Capture the Primer-turn index, then run `respond_to_streaming`.
///    `await_pending_background` at the top of that method drains the
///    PREVIOUS turn's classifier / extractor / comprehension tasks
///    inside the natural inter-turn pause — that's the latency
///    property we get back from holding DM long-lived.
/// 4. Emit `primer://turn_complete` immediately on success. The current
///    turn's spawned background tasks keep running in the background
///    and will be awaited at the start of the next turn (or at
///    `close_session`).
#[tauri::command]
pub async fn send_message(
    state: tauri::State<'_, AppState>,
    app: AppHandle,
    input: String,
) -> Result<TurnComplete, String> {
    // Clone the DM Arc out under the session mutex, then release it.
    let dm_arc = {
        let guard = state.session.lock().await;
        let active = guard
            .as_ref()
            .ok_or_else(|| "no active session — call start_session first".to_string())?;
        Arc::clone(&active.dialogue_manager)
    };

    run_turn(&dm_arc, &input, |primer_turn_index, chunk| {
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

/// Drive one streaming turn against a held DM.
///
/// Split out from `send_message` so unit tests can exercise the full
/// lock / respond / unlock flow without a Tauri `AppHandle` in scope.
///
/// `on_chunk` is invoked once per streamed token with the Primer-turn
/// index and the chunk text.
async fn run_turn<F>(
    dm_arc: &Arc<Mutex<DialogueManager>>,
    input: &str,
    mut on_chunk: F,
) -> Result<TurnComplete, String>
where
    F: FnMut(usize, &str),
{
    let mut dm = dm_arc.lock().await;

    // The Primer turn lands at `turns.len() + 1` after the child turn
    // appends at `turns.len()`. Captured before respond_to_streaming
    // runs so the chunk callback can address its bubble.
    let primer_turn_index = dm.session.turns.len() + 1;
    let session_id: Uuid = dm.session.id;

    dm.respond_to_streaming(input, |chunk| on_chunk(primer_turn_index, chunk))
        .await
        .map_err(|e| e.to_string())?;

    Ok(TurnComplete {
        session_id,
        child_turn_index: primer_turn_index - 1,
        primer_turn_index,
    })
}

async fn info_from(active: &ActiveSession) -> SessionInfo {
    let dm = active.dialogue_manager.lock().await;
    let session_has_turns = !dm.session.turns.is_empty();
    SessionInfo {
        // The session-row UUID is real from construction (DM mints it
        // in `new`), but the on-disk row only exists after the first
        // turn lands (`persist_turn` saves it). Return None until then
        // so the frontend doesn't display a UUID that can't yet be
        // round-tripped through `load_session`.
        session_id: session_has_turns.then_some(dm.session.id),
        learner: LearnerSummary {
            id: dm.learner.profile.id,
            name: dm.learner.profile.name.clone(),
            age: dm.learner.profile.age,
            concept_count: dm.learner.concepts.len(),
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
        let active = build_active_session(home.path(), &cfg).await.unwrap();
        let dm_arc = Arc::clone(&active.dialogue_manager);

        let mut chunks = Vec::<(usize, String)>::new();
        let payload = run_turn(&dm_arc, "hello", |i, c| chunks.push((i, c.to_string())))
            .await
            .unwrap();

        assert_eq!(payload.child_turn_index, 0, "child lands at index 0");
        assert_eq!(payload.primer_turn_index, 1, "primer lands at index 1");
        assert!(!chunks.is_empty(), "stub backend emits at least one chunk");
        for (idx, _) in &chunks {
            assert_eq!(*idx, payload.primer_turn_index);
        }
    }

    #[tokio::test]
    async fn second_turn_continues_same_session() {
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();
        let dm_arc = Arc::clone(&active.dialogue_manager);

        let first = run_turn(&dm_arc, "hello", |_, _| {}).await.unwrap();
        let second = run_turn(&dm_arc, "tell me more", |_, _| {}).await.unwrap();

        assert_eq!(
            first.session_id, second.session_id,
            "long-lived DM holds the same Session across turns"
        );
        assert_eq!(second.child_turn_index, 2, "child #2 lands after first exchange");
        assert_eq!(second.primer_turn_index, 3, "primer #2 lands at index 3");
    }

    #[tokio::test]
    async fn turn_persists_to_session_store() {
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();
        let dm_arc = Arc::clone(&active.dialogue_manager);

        let payload = run_turn(&dm_arc, "what is curiosity?", |_, _| {})
            .await
            .unwrap();

        // Reach into the DM's storage Arc to verify the on-disk row
        // round-trips. The DM exposes its stores via the public field
        // path indirectly — we re-open via the test config's session
        // db path instead, which validates the actual on-disk artefact
        // independently of any DM-internal caching.
        let session_db = home.path().join("test_session.db");
        let store = primer_storage::SqliteSessionStore::open_for_locale(
            &session_db,
            active.locale,
        )
        .unwrap();
        let loaded = primer_core::storage::SessionStore::load_session(&store, payload.session_id)
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
