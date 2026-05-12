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

use crate::state::{ActiveSession, AppState};
use crate::types::{LearnerSummary, SessionInfo};
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
