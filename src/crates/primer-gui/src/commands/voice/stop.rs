//! Stopping and cancelling voice mode: the `stop_voice_mode` and
//! `cancel_voice_response` commands (plus their non-`speech` stubs) and
//! the `stop_voice_mode_inner` teardown helper shared with the Android
//! stop path and `session::prepare_for_session_change`.

use crate::state::AppState;

/// Stop the active voice loop, if any.
///
/// Sends the stop signal then joins the loop task with a 5-second timeout.
/// On timeout, the join handle is dropped, which aborts the task.
/// Idempotent — returns `Ok(())` when no voice loop is active.
#[cfg(feature = "speech")]
#[tauri::command]
pub async fn stop_voice_mode(state: tauri::State<'_, AppState>) -> Result<(), String> {
    stop_voice_mode_inner(&state, false).await
}

/// Stub for builds without the speech feature.
#[cfg(not(feature = "speech"))]
#[tauri::command]
pub async fn stop_voice_mode(_state: tauri::State<'_, AppState>) -> Result<(), String> {
    Ok(())
}

/// Internal helper so `start_voice_mode` can close any active loop
/// before spawning a new one.
///
/// When `preserve_toggle == false`, flips `speech.voice_mode_enabled = false`
/// on the way out, AFTER the loop has actually been joined (or timed out).
/// That keeps the sticky toggle aligned with what the user just did:
/// pressing Stop persists the off-state durably, while a start-failure
/// leaves the prior value untouched so the consent-dialog reach-back
/// from `start_voice_mode`'s `AssetMissing` path continues to render the
/// toggle in its original position.
///
/// When `preserve_toggle == true`, the sticky-toggle flip is skipped.
/// Used by `session::prepare_for_session_change` so a transient
/// teardown across a session switch doesn't surface as the user having
/// pressed the off button — the frontend reads the still-`true` flag
/// and auto-restarts voice mode under the new locale (closes #102
/// polished follow-up).
///
/// Shared by the desktop (`speech`) and Android (`android-native`) stop
/// paths: the body only touches `state`, the loop handle, config, and the
/// session — nothing cpal-specific — so the Android command delegates here
/// rather than duplicating the join + toggle + session-drop logic.
#[cfg(any(feature = "speech", feature = "android-native"))]
pub(crate) async fn stop_voice_mode_inner(
    state: &AppState,
    preserve_toggle: bool,
) -> Result<(), String> {
    let Some(active) = state.voice.lock().await.take() else {
        return Ok(());
    };
    // Signal the loop to exit cleanly at the next LISTEN boundary.
    let _ = active.stop_tx.send(());
    // Bound the join wait: a stuck audio thread cannot hang the GUI.
    let timeout = std::time::Duration::from_secs(5);
    let join_result = tokio::time::timeout(timeout, active.join).await;

    // Flip the sticky toggle off — voice mode just stopped. Persist
    // failure is logged but doesn't propagate; the in-memory state is
    // already correct and the next save_settings will pick it up.
    // Skipped on a transient teardown (`preserve_toggle == true`).
    if !preserve_toggle {
        let mut c = state.config.lock().await;
        c.speech.voice_mode_enabled = false;
        if let Err(e) = crate::config::save(&state.home, &c) {
            tracing::warn!("persist speech.voice_mode_enabled=false failed: {e}");
        }
    }

    // Also drop the underlying active session (the DM that the voice
    // responder was holding). The voice loop already exited, so the
    // Arc<Mutex<DM>> the responder captured drops at the same time as
    // the join future above — pulling the ActiveSession out of
    // state.session now releases the GUI's last strong ref.
    if let Some(active_session) = state.session.lock().await.take() {
        let mut dm = active_session.dialogue_manager.lock().await;
        dm.close_session().await;
    }

    match join_result {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(e))) => Err(format!("voice loop returned error: {e}")),
        Ok(Err(e)) => Err(format!("voice loop join failed: {e}")),
        Err(_) => {
            tracing::warn!("voice loop did not stop within 5s; the runtime will abort it");
            // Falling out of scope drops the JoinHandle, which aborts the task.
            Ok(())
        }
    }
}

/// Cancel the in-flight LLM call + TTS synthesis for the current turn.
///
/// Non-blocking — the cancel channel has capacity 8 so the loop can
/// handle rapid double-clicks without spinning. Idempotent when there
/// is no active voice loop.
#[cfg(feature = "speech")]
#[tauri::command]
pub async fn cancel_voice_response(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let guard = state.voice.lock().await;
    if let Some(active) = guard.as_ref() {
        // Non-blocking send. If the channel is full (user mashed Cancel
        // eight times in rapid succession) one cancel is enough.
        let _ = active.cancel_response_tx.try_send(());
    }
    Ok(())
}

/// Stub for builds without the speech feature.
#[cfg(not(feature = "speech"))]
#[tauri::command]
pub async fn cancel_voice_response(_state: tauri::State<'_, AppState>) -> Result<(), String> {
    Ok(())
}
