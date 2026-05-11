//! Settings commands: load, return, persist.
//!
//! `get_settings` is a pure read of the in-memory copy held by
//! `AppState`. `update_settings` swaps the in-memory copy AND atomically
//! writes the JSON to disk so a crash between IPC round-trips never
//! leaves the user's settings out of sync.

use crate::config::{self, GuiConfig};
use crate::state::AppState;

/// Return the current GUI settings. Always succeeds — even when no
/// config file exists on disk (defaults are returned).
#[tauri::command]
pub async fn get_settings(state: tauri::State<'_, AppState>) -> Result<GuiConfig, String> {
    Ok(state.config.lock().await.clone())
}

/// Replace the current GUI settings.
///
/// Order matters: persist to disk first (so a panic between the disk
/// write and the in-memory swap never leaves disk lagging memory),
/// then update the in-memory copy. Returns the validation/error
/// string for the frontend to render inline.
///
/// **Active-session impact:** the in-memory ActiveSession (if any) is
/// NOT mutated here. Settings that affect the active session (backend,
/// model, locale, embedder) take effect only after the next
/// `start_session` — this matches the "Save & start new session"
/// flow planned for the settings modal in step 8.
#[tauri::command]
pub async fn update_settings(
    state: tauri::State<'_, AppState>,
    config: GuiConfig,
) -> Result<(), String> {
    config::save(&state.home, &config).map_err(|e| e.to_string())?;
    *state.config.lock().await = config;
    Ok(())
}
