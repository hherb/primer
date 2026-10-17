//! `download_voice_assets`: the consent-modal download command (plus its
//! non-`speech` stub). Downloads go through `crate::voice::download`.

use tauri::AppHandle;

use crate::state::AppState;

/// Download the voice assets matching the frontend-requested `kinds`.
///
/// **Hardened IPC:** the frontend echoes only the `kind` strings from the
/// original `AssetMissing.entries`; the server re-resolves `path` +
/// `suggested_url` via [`crate::voice::assets::resolve_requested_kinds`].
/// A compromised webview therefore cannot direct the host to write outside
/// `~/.cache/primer/models/` or fetch from a non-canonical URL — the path
/// and URL never cross the trust boundary as input.
///
/// Emits `primer://voice/download_progress` events as each file streams
/// in. Returns `Ok(())` on full success (or when nothing is missing —
/// e.g. another process completed the download concurrently) or
/// `Err(String)` on the first failure; the consent modal renders the
/// error inline.
///
/// Unknown / already-present kinds are silently dropped (safe — there
/// is nothing to download).
#[cfg(feature = "speech")]
#[tauri::command]
pub async fn download_voice_assets(
    state: tauri::State<'_, AppState>,
    app: AppHandle,
    kinds: Vec<String>,
) -> Result<(), String> {
    use primer_core::i18n::Locale;
    let cfg = state.config.lock().await.clone();
    let locale = Locale::from_pack_id(&cfg.learner.locale).unwrap_or_default();
    // `disable_auto_download` is intentionally NOT re-checked here: the gate
    // sits at the single `start_voice_mode` resolver-Err site, and reaching
    // this command means the consent modal already rendered — which only
    // happens when the flag is off. See `missing_to_error`.
    let to_download =
        crate::voice::assets::resolve_requested_kinds(&state.home, &cfg.speech, &locale, &kinds);
    for asset in &to_download {
        crate::voice::download::download_one(&app, asset, cfg.speech.download_timeout_secs).await?;
    }
    Ok(())
}

/// Stub for builds without the speech feature. Returns an error so the
/// frontend doesn't silently noop.
#[cfg(not(feature = "speech"))]
#[tauri::command]
pub async fn download_voice_assets(
    _state: tauri::State<'_, AppState>,
    _app: AppHandle,
    _kinds: Vec<String>,
) -> Result<(), String> {
    Err("voice mode not built in this binary".into())
}
