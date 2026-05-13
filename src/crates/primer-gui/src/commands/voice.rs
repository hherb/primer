//! Voice-mode Tauri commands.
//!
//! `start_voice_mode` builds the voice loop and stashes its handle in
//! `AppState::voice`. `stop_voice_mode` drains the loop.
//! `cancel_voice_response` aborts the in-flight LLM call + TTS synthesis.
//!
//! All commands are gated by `#[cfg(feature = "speech")]`; the non-speech
//! build provides stubs returning `Err(NotBuilt)` or `Ok(())`.

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::state::AppState;
use crate::types::SessionInfo;

#[cfg(feature = "speech")]
use std::sync::Arc;

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
///
/// `Deserialize` is required because the frontend echoes the original
/// asset list back into `download_voice_assets` after the user consents
/// to the download.
#[derive(Serialize, Deserialize, Clone, Debug)]
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
/// Closes any active text session, closes any active voice loop, resolves
/// the locale's voice assets (returning `Err(AssetMissing { … })` if any
/// are absent so the frontend can render the consent dialog), builds the
/// local backends (mic + speaker + VAD + STT + TTS), and spawns the
/// shared voice loop. The active session is moved into `state.session`
/// (so sidebar / learner-state commands keep working) and the loop
/// handle is moved into `state.voice`.
///
/// Returns a [`SessionInfo`] on success so the frontend can display the
/// active learner/backend identity in voice mode the same way it does in
/// text mode. On `AssetMissing`, the sticky `voice_mode_enabled` flag is
/// left at its current value so the consent dialog can render the toggle
/// in its original position.
#[cfg(feature = "speech")]
#[tauri::command]
pub async fn start_voice_mode(
    state: tauri::State<'_, AppState>,
    app: AppHandle,
) -> Result<SessionInfo, StartVoiceModeError> {
    use primer_core::i18n::Locale;

    // 1. Close any active text session (drains background tasks).
    super::session::close_session_inner(&state)
        .await
        .map_err(StartVoiceModeError::from)?;

    // 2. Close any already-active voice loop.
    stop_voice_mode_inner(&state).await.ok();

    let cfg = state.config.lock().await.clone();

    // 3. Build the active session via the shared wiring so DM
    //    construction is identical to text mode. The active session is
    //    moved into `state.session` so `current_session_info` /
    //    `get_learner_state` / sidebar refresh commands keep working
    //    while voice mode runs.
    let active_session = crate::wiring::build_active_session(&state.home, &cfg)
        .await
        .map_err(StartVoiceModeError::from)?;

    // 4. Resolve voice assets for the active locale.
    let locale = Locale::from_pack_id(&cfg.learner.locale).unwrap_or_default();
    let assets =
        crate::voice::assets::resolve_voice_assets(&state.home, &cfg.speech, &locale).map_err(
            |missing| StartVoiceModeError::AssetMissing {
                entries: missing.entries,
            },
        )?;

    // 5. Build the local backends (cpal mic + speaker, VAD, STT, TTS,
    //    audio thread, on_audio, drain hook). Lives in primer-speech;
    //    GUI wraps via voice::backends::build_loop_backends.
    let mut local =
        crate::voice::backends::build_loop_backends(&assets, locale, cfg.speech.mic_silence_ms)
            .await
            .map_err(|e| StartVoiceModeError::from(format!("backend init: {e}")))?;

    let backends = local
        .backends
        .take()
        .expect("build_local_backends always returns backends");
    let event_rx = local
        .event_rx
        .take()
        .expect("build_local_backends always returns event_rx");
    let on_audio = local
        .on_audio
        .take()
        .expect("build_local_backends always returns on_audio");
    let drain_hook = local
        .drain_hook
        .take()
        .expect("build_local_backends always returns drain_hook");
    let is_speaking = Arc::clone(&local.is_speaking);

    // 6. Construct the responder + observer + spawn the loop.
    let dm_arc = Arc::clone(&active_session.dialogue_manager);
    let observer = crate::voice::observer::TauriEventObserver::new(app.clone(), locale);
    let responder: Box<dyn primer_speech::voice_loop::Responder + 'static> =
        Box::new(crate::voice::responder::ArcDmResponder::new(dm_arc));

    let (handle, join) = primer_speech::voice_loop::run_loop(
        backends,
        event_rx,
        responder,
        on_audio,
        Some(drain_hook),
        false, // verbose: GUI logs via tracing, never stderr
        Some(is_speaking),
        observer,
    );

    // The audio thread + cpal streams live inside `local`; the spawned
    // run_loop task holds the responder + backends. The voice-mode
    // shutdown path runs `local.shutdown()` after the loop joins, so
    // ownership of `local` must survive until then — we stash it inside
    // a tokio task wrapper that joins both.
    let wrapped_join = tokio::spawn(async move {
        let result = join.await;
        // Now that the loop has exited, signal the audio thread + drop
        // cpal streams.
        local.shutdown();
        drop(local);
        match result {
            Ok(Ok(_transcripts)) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(e) => Err(primer_speech::voice_loop::VoiceLoopError::Other(format!(
                "voice loop task panicked: {e}"
            ))),
        }
    });

    // 7. Build SessionInfo from the active session, then move both the
    //    active session (into state.session) and the loop handle (into
    //    state.voice). Acquire the snapshot lock once and read all four
    //    fields under it — re-locking per field would interleave reads
    //    against any concurrent snapshot mutation.
    let learner = {
        let snap = active_session.snapshot.lock().await;
        crate::types::LearnerSummary {
            id: snap.learner_id,
            name: snap.learner_name.clone(),
            age: snap.learner_age,
            concept_count: snap.concept_count,
        }
    };
    let info = SessionInfo {
        session_id: None,
        learner,
        backend_kind: active_session.backend_name.clone(),
        main_model: active_session.main_model.clone(),
        locale: active_session.locale.pack_id().to_string(),
        voice_mode_available: true,
    };

    *state.session.lock().await = Some(active_session);
    *state.voice.lock().await = Some(crate::state::ActiveVoiceLoop {
        join: wrapped_join,
        stop_tx: handle.stop_tx,
        cancel_response_tx: handle.cancel_response_tx,
        info: info.clone(),
    });

    // 8. Flip the sticky-toggle on successful start. Failure to persist
    //    is logged but not propagated — the voice loop is already
    //    running and the user expects voice mode to work.
    {
        let mut c = state.config.lock().await;
        c.speech.voice_mode_enabled = true;
        if let Err(e) = crate::config::save(&state.home, &c) {
            tracing::warn!("persist speech.voice_mode_enabled=true failed: {e}");
        }
    }

    Ok(info)
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
///
/// Flips `speech.voice_mode_enabled = false` on the way out, AFTER the
/// loop has actually been joined (or timed out). That keeps the sticky
/// toggle aligned with what the user just did: pressing Stop persists
/// the off-state durably, while a start-failure leaves the prior value
/// untouched so the consent-dialog reach-back from `start_voice_mode`'s
/// `AssetMissing` path continues to render the toggle in its original
/// position.
#[cfg(feature = "speech")]
pub(crate) async fn stop_voice_mode_inner(state: &AppState) -> Result<(), String> {
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
    {
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

/// Download every [`MissingAsset`] in `missing`. Emits
/// `primer://voice/download_progress` events as each file streams in.
/// Returns `Ok(())` on full success or `Err(String)` on the first
/// failure; the consent modal renders the error inline.
///
/// Idempotent at the resolver layer: if a file already exists on disk
/// it would not appear in `missing` and is silently skipped here.
#[cfg(feature = "speech")]
#[tauri::command]
pub async fn download_voice_assets(
    _state: tauri::State<'_, AppState>,
    app: AppHandle,
    missing: Vec<MissingAsset>,
) -> Result<(), String> {
    for asset in &missing {
        crate::voice::download::download_one(&app, asset).await?;
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
    _missing: Vec<MissingAsset>,
) -> Result<(), String> {
    Err("voice mode not built in this binary".into())
}

/// Locale-aware copy strings for the voice-state widget.
///
/// Returns the six display strings (label + hint for each of the three
/// voice states) in the learner's current locale. Not feature-gated — it
/// is just a locale table lookup and works in default (non-speech) builds
/// too, so the Settings → Speech badge can show the right language even
/// when the voice loop isn't compiled in.
#[derive(Serialize, Debug)]
pub struct VoiceStateCopy {
    pub listen_label: String,
    pub listen_hint: String,
    pub thinking_label: String,
    pub thinking_hint: String,
    pub speak_label: String,
    pub speak_hint: String,
}

impl VoiceStateCopy {
    fn for_locale(locale: &primer_core::i18n::Locale) -> Self {
        match locale {
            primer_core::i18n::Locale::German => Self {
                listen_label:   "Höre zu…".into(),
                listen_hint:    "lass dir Zeit".into(),
                thinking_label: "Denke nach…".into(),
                thinking_hint:  "der Primer überlegt eine Antwort".into(),
                speak_label:    "Spreche…".into(),
                speak_hint:     "lass den Primer ausreden".into(),
            },
            // English is the default for any unrecognised locale.
            _ => Self {
                listen_label:   "Listening…".into(),
                listen_hint:    "take your time".into(),
                thinking_label: "Thinking…".into(),
                thinking_hint:  "the Primer is working on a reply".into(),
                speak_label:    "Speaking…".into(),
                speak_hint:     "let the Primer finish".into(),
            },
        }
    }
}

#[tauri::command]
pub async fn get_voice_state_copy(
    state: tauri::State<'_, AppState>,
) -> Result<VoiceStateCopy, String> {
    let cfg = state.config.lock().await.clone();
    let locale =
        primer_core::i18n::Locale::from_pack_id(&cfg.learner.locale).unwrap_or_default();
    Ok(VoiceStateCopy::for_locale(&locale))
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
