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

#[cfg(feature = "speech")]
use std::sync::Arc;

/// Asset-kind identifiers. Shared source of truth between the
/// emit site (`voice::assets::resolve_voice_assets`) and the filter site
/// (`voice::assets::resolve_requested_kinds`). Lives here — not under
/// `voice::assets` — because it describes the IPC shape itself and must
/// be addressable in default (non-speech) builds too (the
/// `MissingAsset` type and its serialisation test are always compiled).
pub mod kind {
    pub const PIPER_ONNX: &str = "piper_onnx";
    pub const PIPER_CONFIG: &str = "piper_config";
    pub const WHISPER_MODEL: &str = "whisper_model";
}

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
/// **IPC direction is server→webview only.** `Deserialize` is deliberately
/// NOT derived: the frontend echoes back only the `kind` strings, and the
/// server re-resolves `path` + `suggested_url` server-side via
/// [`crate::voice::assets::resolve_requested_kinds`]. This keeps the IPC
/// trust boundary tight — a compromised webview cannot direct the host to
/// write outside `~/.cache/primer/models/` or to fetch from a non-canonical
/// URL because those fields never cross the trust boundary as input.
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

    // 2. Close any already-active voice loop. The toggle flip-down
    //    inside stop_voice_mode_inner is harmless here because step 8
    //    below flips it back to `true` after the new loop is up.
    stop_voice_mode_inner(&state, false).await.ok();

    let cfg = state.config.lock().await.clone();

    // 3. Build the active session via the shared wiring so DM
    //    construction is identical to text mode. The active session is
    //    moved into `state.session` so `current_session_info` /
    //    `get_learner_state` / sidebar refresh commands keep working
    //    while voice mode runs.
    let active_session = crate::wiring::build_active_session(&state.home, &cfg)
        .await
        .map_err(StartVoiceModeError::from)?;

    // 4. Resolve voice assets for the active locale, gated on the
    //    decoupled (stt, tts) choice.
    let locale = Locale::from_pack_id(&cfg.learner.locale).unwrap_or_default();
    let (stt, tts) = cfg.speech.resolve_backends();
    let assets =
        crate::voice::assets::resolve_voice_assets(&state.home, &cfg.speech, &locale, stt, tts)
            .map_err(|missing| StartVoiceModeError::AssetMissing {
                entries: missing.entries,
            })?;

    // 5. Build the local backends (cpal mic + speaker, VAD, STT, TTS,
    //    audio thread, on_audio, drain hook). Lives in primer-speech;
    //    GUI wraps via voice::backends::build_loop_backends.
    let mut local = crate::voice::backends::build_loop_backends(
        &assets,
        locale,
        cfg.speech.mic_silence_ms,
        stt,
        tts,
    )
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
#[cfg(feature = "speech")]
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
    /// Build the locale-aware copy by reading the active prompt pack's
    /// `[voice_state]` table. The embedded packs are validated at build
    /// time so a load failure here would be a structural codebase bug,
    /// not a user-recoverable condition — mirrors the established pattern
    /// at `dialogue_manager::lifecycle::DialogueManager::new`.
    fn for_locale(locale: &primer_core::i18n::Locale) -> Self {
        let pack = primer_pedagogy::prompt_pack::load_cached(*locale)
            .expect("prompt pack load failed; this should be impossible at runtime");
        let labels = pack.voice_state_labels();
        Self {
            listen_label: labels.listen_label.clone(),
            listen_hint: labels.listen_hint.clone(),
            thinking_label: labels.thinking_label.clone(),
            thinking_hint: labels.thinking_hint.clone(),
            speak_label: labels.speak_label.clone(),
            speak_hint: labels.speak_hint.clone(),
        }
    }
}

#[tauri::command]
pub async fn get_voice_state_copy(
    state: tauri::State<'_, AppState>,
) -> Result<VoiceStateCopy, String> {
    let cfg = state.config.lock().await.clone();
    let locale = primer_core::i18n::Locale::from_pack_id(&cfg.learner.locale).unwrap_or_default();
    Ok(VoiceStateCopy::for_locale(&locale))
}

/// Whether voice mode is built into this binary.
///
/// Returns `cfg!(feature = "speech")` — a compile-time constant. Independent
/// of any session state so the frontend can enable / disable the voice
/// toggle at launch without waiting for a session to start. Previously the
/// frontend read this flag off `current_session_info`, which left the
/// toggle permanently disabled on the session-picker screen (no active
/// session at launch → `null` → `state.available = false` → tooltip
/// incorrectly says "Voice mode is not built into this binary").
#[tauri::command]
pub async fn voice_mode_available() -> Result<bool, String> {
    Ok(cfg!(feature = "speech"))
}

/// Whether this binary was compiled with a macOS-native speech stack
/// (`macos-native` or `macos-native-26`). The settings modal uses this
/// to enable/disable the "macOS Native" speech-backend option: selecting
/// it on a build without the feature would silently fall through to
/// whisper/piper (see `voice::backends::build_loop_backends`), so the
/// option is shown-but-disabled with a hint instead. Mirrors
/// `voice_mode_available` — a pure compile-time flag, no session state.
#[tauri::command]
pub async fn macos_native_speech_available() -> Result<bool, String> {
    Ok(cfg!(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26"),
    )))
}

/// Whether this binary was compiled with the `supertonic` feature. The
/// settings modal uses this to enable/disable the "Supertonic" option in
/// the TTS-backend dropdown: selecting it on a build without the feature
/// would fail at session start with a "rebuild with --features supertonic"
/// error (see `voice_loop::build_tts`), so the option is shown-but-disabled
/// with a hint instead. Mirrors `macos_native_speech_available` — a pure
/// compile-time flag, no session state.
#[tauri::command]
pub async fn supertonic_tts_available() -> Result<bool, String> {
    Ok(cfg!(feature = "supertonic"))
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
            kind: super::kind::WHISPER_MODEL.into(),
            path: "/tmp/foo.bin".into(),
            suggested_url: Some("https://example.com/foo.bin".into()),
            approx_size_mb: Some(470),
        };
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(json["kind"], "whisper_model");
        assert_eq!(json["approx_size_mb"], 470);
    }

    // Trust-boundary invariant: `MissingAsset` must NEVER implement
    // `Deserialize`. The IPC direction is server→webview only — if a
    // future contributor re-derives `Deserialize` (e.g. "just to
    // round-trip through a frontend cache"), the trust-boundary
    // hardening from #90 silently regresses. This compile-time check is
    // the load-bearing structural guarantee; the doc comment on
    // `MissingAsset` itself explains the rationale. If you genuinely
    // need to round-trip the type back, define a separate echoed-
    // identity DTO instead of re-deriving `Deserialize` here.
    static_assertions::assert_not_impl_any!(MissingAsset: serde::de::DeserializeOwned);

    /// Pin the existing English `VoiceStateCopy` strings byte-identically.
    /// This is the regression witness for the i18n refactor that moves the
    /// six display strings into `primer_pedagogy::prompt_pack`. The pack
    /// values must reproduce these strings exactly; any drift here would
    /// silently change UI copy.
    #[test]
    fn voice_state_copy_english_strings_pinned() {
        let copy = VoiceStateCopy::for_locale(&primer_core::i18n::Locale::English);
        assert_eq!(copy.listen_label, "Listening…");
        assert_eq!(copy.listen_hint, "take your time");
        assert_eq!(copy.thinking_label, "Thinking…");
        assert_eq!(copy.thinking_hint, "the Primer is working on a reply");
        assert_eq!(copy.speak_label, "Speaking…");
        assert_eq!(copy.speak_hint, "let the Primer finish");
    }

    /// Pin the existing German `VoiceStateCopy` strings byte-identically.
    /// Sibling of [`voice_state_copy_english_strings_pinned`] — see that
    /// test's doc for rationale.
    #[test]
    fn voice_state_copy_german_strings_pinned() {
        let copy = VoiceStateCopy::for_locale(&primer_core::i18n::Locale::German);
        assert_eq!(copy.listen_label, "Höre zu…");
        assert_eq!(copy.listen_hint, "lass dir Zeit");
        assert_eq!(copy.thinking_label, "Denke nach…");
        assert_eq!(copy.thinking_hint, "der Primer überlegt eine Antwort");
        assert_eq!(copy.speak_label, "Spreche…");
        assert_eq!(copy.speak_hint, "lass den Primer ausreden");
    }

    /// `voice_mode_available` must mirror `cfg!(feature = "speech")` exactly.
    /// The frontend uses this flag at launch (independent of session state)
    /// to enable / disable the voice toggle. Drift here would silently
    /// break the consent-modal flow or the never-enabled regression that
    /// motivated splitting this off from `current_session_info`.
    #[tokio::test]
    async fn voice_mode_available_matches_cfg_feature_speech() {
        let result = voice_mode_available().await.expect("command never errors");
        assert_eq!(result, cfg!(feature = "speech"));
    }

    /// The macOS-native speech capability reflects the compiled feature
    /// set exactly. Written against the same `cfg!` expression as the
    /// command so it holds on both a default build (false) and a
    /// `--features macos-native` build (true on macOS).
    #[tokio::test]
    async fn macos_native_speech_available_matches_cfg() {
        let expected = cfg!(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26"),
        ));
        assert_eq!(
            macos_native_speech_available()
                .await
                .expect("command never errors"),
            expected
        );
    }

    /// The Supertonic TTS capability reflects the compiled feature set
    /// exactly. Holds on a default build (false) and a `--features
    /// supertonic` build (true).
    #[tokio::test]
    async fn supertonic_tts_available_matches_cfg() {
        assert_eq!(
            supertonic_tts_available()
                .await
                .expect("command never errors"),
            cfg!(feature = "supertonic")
        );
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

    // ─── Issue #102 polished follow-up: preserve sticky toggle ──────

    /// Synthesize an `ActiveVoiceLoop` whose task completes the moment
    /// `stop_tx` is signaled — mirrors the production join contract
    /// without spinning up cpal/whisper/piper. Returned ready to stash
    /// into `state.voice`.
    #[cfg(feature = "speech")]
    fn fake_voice_loop(info: crate::types::SessionInfo) -> crate::state::ActiveVoiceLoop {
        use primer_speech::voice_loop::VoiceLoopError;
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let (cancel_tx, _cancel_rx) = tokio::sync::mpsc::channel::<()>(8);
        let join: tokio::task::JoinHandle<Result<(), VoiceLoopError>> = tokio::spawn(async move {
            let _ = stop_rx.await;
            Ok(())
        });
        crate::state::ActiveVoiceLoop {
            join,
            stop_tx,
            cancel_response_tx: cancel_tx,
            info,
        }
    }

    #[cfg(feature = "speech")]
    fn make_synthetic_session_info() -> crate::types::SessionInfo {
        use crate::types::LearnerSummary;
        crate::types::SessionInfo {
            session_id: None,
            learner: LearnerSummary {
                id: uuid::Uuid::nil(),
                name: "test".into(),
                age: 8,
                concept_count: 0,
            },
            backend_kind: "stub".into(),
            main_model: "stub".into(),
            locale: "en".into(),
            voice_mode_available: true,
        }
    }

    /// User pressed Stop (or a start-error teardown happened) →
    /// `preserve_toggle = false` → the sticky toggle is durably
    /// flipped off so the next launch doesn't auto-resume voice mode.
    #[cfg(feature = "speech")]
    #[tokio::test]
    async fn stop_voice_mode_inner_flips_toggle_off_by_default() {
        use crate::config::GuiConfig;
        use crate::state::AppState;
        use tempfile::TempDir;

        let home = TempDir::new().unwrap();
        let mut cfg = GuiConfig::default();
        cfg.speech.voice_mode_enabled = true;
        let state = AppState::new(home.path().to_path_buf(), cfg);
        *state.voice.lock().await = Some(fake_voice_loop(make_synthetic_session_info()));

        stop_voice_mode_inner(&state, false).await.unwrap();

        assert!(state.voice.lock().await.is_none(), "loop must be cleared");
        assert!(
            !state.config.lock().await.speech.voice_mode_enabled,
            "preserve_toggle=false must flip the sticky toggle off"
        );
    }

    /// Session switch → `preserve_toggle = true` → the sticky toggle
    /// stays at its current value so the frontend can auto-restart
    /// voice mode under the new locale (closes #102 polished
    /// follow-up).
    #[cfg(feature = "speech")]
    #[tokio::test]
    async fn stop_voice_mode_inner_preserves_toggle_when_requested() {
        use crate::config::GuiConfig;
        use crate::state::AppState;
        use tempfile::TempDir;

        let home = TempDir::new().unwrap();
        let mut cfg = GuiConfig::default();
        cfg.speech.voice_mode_enabled = true;
        let state = AppState::new(home.path().to_path_buf(), cfg);
        *state.voice.lock().await = Some(fake_voice_loop(make_synthetic_session_info()));

        stop_voice_mode_inner(&state, true).await.unwrap();

        assert!(state.voice.lock().await.is_none(), "loop must be cleared");
        assert!(
            state.config.lock().await.speech.voice_mode_enabled,
            "preserve_toggle=true must leave the sticky toggle untouched so the frontend's \
             post-`start_session` `primerRestoreVoiceMode` sees true and auto-restarts"
        );
    }
}
