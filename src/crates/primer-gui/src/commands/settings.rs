//! Settings commands: load, return, persist.
//!
//! `get_settings` returns a redacted view ([`GuiConfigView`]) so the
//! inline API key never crosses the IPC boundary. `update_settings`
//! consumes a [`GuiConfigUpdate`] whose `ApiKeyUpdate::Keep` variant
//! lets the frontend save the rest of the config without ever holding
//! the secret. Validation runs *before* the disk write so a bad config
//! never lands on disk.

use crate::config::{self, GuiConfigUpdate, GuiConfigView};
use crate::state::AppState;
use crate::validation;

/// Return the current GUI settings (redacted view — no inline API key).
/// Always succeeds; missing-on-disk returns the in-memory defaults
/// loaded at startup.
#[tauri::command]
pub async fn get_settings(state: tauri::State<'_, AppState>) -> Result<GuiConfigView, String> {
    Ok((&*state.config.lock().await).into())
}

/// Replace the current GUI settings.
///
/// Steps, in order:
/// 1. Resolve the update against the persisted value (so
///    `ApiKeyUpdate::Keep` carries forward the existing secret).
/// 2. Validate — surface obviously-bad configs (unknown backend kind,
///    embedder kind, locale, etc.) here rather than at the next
///    `start_session`.
/// 3. Atomically persist to disk.
/// 4. Swap the in-memory copy.
///
/// **Active-session impact:** the in-memory ActiveSession (if any) is
/// NOT mutated here. Settings that affect the active session (backend,
/// model, locale, embedder) take effect only after the next
/// `start_session` — this matches the "Save & start new session"
/// flow planned for the settings modal in step 8.
#[tauri::command]
pub async fn update_settings(
    state: tauri::State<'_, AppState>,
    config: GuiConfigUpdate,
) -> Result<(), String> {
    let mut guard = state.config.lock().await;
    let resolved = config.into_config(&guard);
    validation::validate(&resolved)?;
    config::save(&state.home, &resolved).map_err(|e| e.to_string())?;
    *guard = resolved;
    Ok(())
}
