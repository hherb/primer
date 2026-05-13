//! Voice-mode Tauri commands.
//!
//! `start_voice_mode` builds the voice loop and stashes its handle in
//! `AppState::voice`. `stop_voice_mode` drains the loop.
//! `cancel_voice_response` aborts the in-flight LLM call + TTS synthesis.
//!
//! All commands are gated by `#[cfg(feature = "speech")]`; the non-speech
//! build provides stubs returning `Err(NotBuilt)` or `Ok(())`.

use serde::Serialize;
use tauri::AppHandle;

use crate::state::AppState;
use crate::types::SessionInfo;

/// Structured error returned by `start_voice_mode`.
///
/// Uses `#[serde(tag = "kind", rename_all = "snake_case")]` so the
/// frontend can switch on `err.kind` without deserializing a nested
/// `message` field when it's not needed.
#[derive(Serialize, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StartVoiceModeError {
    /// Built without the `speech` cargo feature.
    NotBuilt,
    /// One or more required model files are missing on disk.
    AssetMissing { entries: Vec<MissingAsset> },
    /// Any other error — message is dev-facing; the frontend renders
    /// a generic banner and does not surface the inner string to the user.
    Other { message: String },
}

/// One missing asset entry in [`StartVoiceModeError::AssetMissing`].
#[derive(Serialize, Clone, Debug)]
pub struct MissingAsset {
    /// Asset type identifier. Stable strings: `"piper_onnx"`,
    /// `"piper_config"`, `"whisper_model"`.
    pub kind: String,
    /// Absolute path where the asset was expected.
    pub path: std::path::PathBuf,
    /// Suggested download URL, if known. `None` for assets where
    /// no canonical upstream URL is available.
    pub suggested_url: Option<String>,
    /// Approximate on-disk size in MiB after download. `None` when
    /// unknown. Used by the asset-consent modal to show a budget.
    pub approx_size_mb: Option<u32>,
}

impl From<String> for StartVoiceModeError {
    fn from(message: String) -> Self {
        Self::Other { message }
    }
}

/// Start voice mode.
///
/// Closes any active text session, closes any active voice loop, validates
/// the config, then (in PR 4) builds the LoopBackends and spawns the voice
/// loop. For PR 3 this command is a lifecycle-plumbing stub — asset
/// resolution is not yet implemented and the command always returns
/// `Err(Other { message: "asset resolution not yet implemented (PR 4)" })`.
///
/// Returns a [`SessionInfo`] on success so the frontend can display the
/// active learner/backend identity in voice mode the same way it does in
/// text mode.
#[cfg(feature = "speech")]
#[tauri::command]
pub async fn start_voice_mode(
    state: tauri::State<'_, AppState>,
    _app: AppHandle,
) -> Result<SessionInfo, StartVoiceModeError> {
    // 1. Close any active text session (drains background tasks).
    super::session::close_session_inner(&state)
        .await
        .map_err(StartVoiceModeError::from)?;

    // 2. Close any already-active voice loop.
    stop_voice_mode_inner(&state).await.ok();

    let cfg = state.config.lock().await.clone();

    // 3. Build the active session via the shared wiring so DM
    //    construction is identical to text mode.
    let active_session = crate::wiring::build_active_session(&state.home, &cfg)
        .await
        .map_err(StartVoiceModeError::from)?;

    // 4. PR 4 will plug in real asset resolution here. For PR 3,
    //    hard-fail with a clear "not implemented" so the lifecycle
    //    plumbing is the only thing under test. PR 4's task 4.3
    //    replaces this stub.
    let _ = active_session; // silence unused-warning until PR 4 wires this in
    Err(StartVoiceModeError::Other {
        message: "asset resolution not yet implemented (PR 4)".into(),
    })
}

/// Stub for builds without the speech feature.
#[cfg(not(feature = "speech"))]
#[tauri::command]
pub async fn start_voice_mode(
    _state: tauri::State<'_, AppState>,
    _app: AppHandle,
) -> Result<SessionInfo, StartVoiceModeError> {
    Err(StartVoiceModeError::NotBuilt)
}

/// Stop the active voice loop, if any.
///
/// Sends the stop signal then joins the loop task with a 5-second timeout.
/// On timeout, the join handle is dropped, which aborts the task.
/// Idempotent — returns `Ok(())` when no voice loop is active.
#[cfg(feature = "speech")]
#[tauri::command]
pub async fn stop_voice_mode(state: tauri::State<'_, AppState>) -> Result<(), String> {
    stop_voice_mode_inner(&state).await
}

/// Stub for builds without the speech feature.
#[cfg(not(feature = "speech"))]
#[tauri::command]
pub async fn stop_voice_mode(_state: tauri::State<'_, AppState>) -> Result<(), String> {
    Ok(())
}

/// Internal helper so `start_voice_mode` can close any active loop
/// before spawning a new one.
#[cfg(feature = "speech")]
pub(crate) async fn stop_voice_mode_inner(state: &AppState) -> Result<(), String> {
    let Some(active) = state.voice.lock().await.take() else {
        return Ok(());
    };
    // Signal the loop to exit cleanly at the next LISTEN boundary.
    let _ = active.stop_tx.send(());
    // Bound the join wait: a stuck audio thread cannot hang the GUI.
    let timeout = std::time::Duration::from_secs(5);
    match tokio::time::timeout(timeout, active.join).await {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the `MissingAsset` serialisation shape. The frontend switches on
    /// `asset.kind` and reads `approx_size_mb` to estimate download budget —
    /// a field rename here silently breaks the asset-consent modal.
    #[test]
    fn missing_asset_serialises_with_snake_case_kind() {
        let m = MissingAsset {
            kind: "whisper_model".into(),
            path: "/tmp/foo.bin".into(),
            suggested_url: Some("https://example.com/foo.bin".into()),
            approx_size_mb: Some(470),
        };
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(json["kind"], "whisper_model");
        assert_eq!(json["approx_size_mb"], 470);
    }

    /// Pin the `StartVoiceModeError` tag format. The frontend branches on
    /// `err.kind` — a rename or format change here silently breaks the
    /// banner rendering and the asset-missing detection path.
    #[test]
    fn start_voice_mode_error_uses_kind_tag() {
        let err = StartVoiceModeError::NotBuilt;
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], "not_built");

        let err = StartVoiceModeError::AssetMissing { entries: vec![] };
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], "asset_missing");
        assert_eq!(json["entries"], serde_json::json!([]));

        let err = StartVoiceModeError::Other {
            message: "test message".into(),
        };
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], "other");
        assert_eq!(json["message"], "test message");
    }
}
