//! Android-native voice-mode Tauri commands.
//!
//! Mirror the desktop [`super::voice`] commands but build the cpal-free
//! `AndroidVoiceBackends` (OS-owned mic + speaker) instead of the local
//! cpal backends, and drive [`primer_speech::voice_loop::run_loop`] with a no-op `on_committed_audio`,
//! `None` speaker-drain, and `None` `is_speaking` (D1 — the OS plays the TTS
//! itself, so there is no sample stream to forward and no ringbuf to drain).
//!
//! Gated on `#[cfg(feature = "android-native")]`. The frontend selects these
//! over the desktop commands when [`super::voice::android_voice_available`]
//! returns true.

use std::sync::Arc;

use tauri::AppHandle;

use super::voice::StartVoiceModeError;
use crate::state::AppState;
use crate::types::SessionInfo;

/// Start Android voice mode.
///
/// Constructs the JNI bridge and runs the mic-permission check *before*
/// closing any active text/voice session, so a failure there keeps the
/// user's session (issue #366). Then builds the active dialogue manager via
/// the shared wiring (identical to text mode), and only then the Android
/// voice backends, whose recognizer task arms the mic as soon as it is
/// spawned — so nothing is captured while the session builds. Finally it
/// spawns the shared voice loop. Returns a [`SessionInfo`] so the frontend
/// shows the same learner/backend identity as text mode.
#[tauri::command]
pub async fn start_voice_mode_android<R: tauri::Runtime>(
    state: tauri::State<'_, AppState>,
    app: AppHandle<R>,
) -> Result<SessionInfo, StartVoiceModeError> {
    use primer_core::i18n::Locale;

    let cfg = state.config.lock().await.clone();

    // 1. Construct the JNI bridge (OS mic + speaker). No asset resolution /
    //    download — the recognizer + TTS are OS-managed. The up-front
    //    mic-permission check runs on the same instance the loop will use.
    //    Steps 1-1a run before any teardown (issue #366), so a denied mic
    //    keeps the text session.
    let locale = Locale::from_pack_id(&cfg.learner.locale).unwrap_or_default();
    let bridge = primer_speech::android::new_jni_bridge()
        .map_err(|e| StartVoiceModeError::from(format!("android bridge init: {e}")))?;

    // 1a. Without RECORD_AUDIO the on-device recognizer only ever emits a
    //     silent ERROR_INSUFFICIENT_PERMISSIONS. Check up front and surface a
    //     typed error the frontend renders as a "grant the mic" banner with
    //     an Open-settings button — rather than a voice toggle that silently
    //     does nothing (issue #253). A genuine `Ok(false)` denial maps to the
    //     PermissionDenied banner; a JNI `Err` is a different fault entirely
    //     (the permission may well be granted), so log it and surface a generic
    //     error instead of misdirecting the user to a settings page that shows
    //     nothing to fix.
    match bridge.has_record_audio_permission() {
        Ok(true) => {}
        Ok(false) => return Err(StartVoiceModeError::PermissionDenied),
        Err(e) => {
            tracing::warn!("RECORD_AUDIO permission check failed: {e}");
            return Err(StartVoiceModeError::from(format!(
                "mic permission check: {e}"
            )));
        }
    }

    // 2. Close any active text session (drains background tasks), then any
    //    already-active voice loop (so its recognizer task is gone before
    //    step 3a arms a new one on the shared `PrimerSpeech` singleton).
    //    Step 6 flips the sticky toggle back to `true` after the new loop
    //    is up.
    super::session::close_session_inner(&state)
        .await
        .map_err(StartVoiceModeError::from)?;
    if let Err(e) = super::voice::stop_voice_mode_inner(&state, false).await {
        tracing::warn!("stopping the previous voice loop before restart: {e}");
    }

    // 3. Build the active session via the shared wiring (same as text mode).
    //    It must follow step 2: the old DM's close flushes pending
    //    learner-model updates, which this build reads back from the DB.
    //    shortcut: a failure here still loses the closed text session;
    //    usually a config change between the two builds (e.g. the locale
    //    guard), so it is rare.
    let active_session = crate::wiring::build_active_session(&state.home, &cfg)
        .await
        .map_err(StartVoiceModeError::from)?;

    // 3a. Build the Android voice backends. After step 3 on purpose: this
    //     spawns the recognizer task, which arms the mic at once, and its
    //     only fallible step (locale coverage) cannot fail for one locale.
    let android = crate::voice::backends_android::build_android_backends(bridge, locale)
        .map_err(|e| StartVoiceModeError::from(format!("android backend init: {e}")))?;
    let crate::voice::backends_android::AndroidVoiceBackends {
        backends,
        event_rx,
        stop: recognizer_stop,
    } = android;

    // 4. Construct the responder + observer + spawn the loop. No
    //    `on_committed_audio` work (OS plays the TTS), no speaker drain, no
    //    `is_speaking` flag (the cpal-only no-barge-in mechanism does not
    //    apply — the recognizer is the audio source, not a ringbuf).
    let dm_arc = Arc::clone(&active_session.dialogue_manager);
    let observer = crate::voice::observer::TauriEventObserver::new(app.clone(), locale);
    let responder: Box<dyn primer_speech::voice_loop::Responder + 'static> =
        Box::new(crate::voice::responder::ArcDmResponder::new(dm_arc));

    let (handle, join) = primer_speech::voice_loop::run_loop(
        backends,
        event_rx,
        responder,
        Box::new(|_| {}), // on_committed_audio: no-op (D1)
        None,             // wait_for_speaker_drain: OS owns playback
        false,            // verbose: GUI logs via tracing
        None,             // is_speaking: no cpal ringbuf to gate
        observer,
    );

    // Stop the recognizer-consumer task once the loop exits (it polls the
    // JNI bridge on its own oneshot; the loop's stop_tx does not reach it).
    let wrapped_join = tokio::spawn(async move {
        let result = join.await;
        let _ = recognizer_stop.send(());
        match result {
            Ok(Ok(_transcripts)) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(e) => Err(primer_speech::voice_loop::VoiceLoopError::Other(format!(
                "voice loop task panicked: {e}"
            ))),
        }
    });

    // 5. Build SessionInfo, then move both the active session and the loop
    //    handle into state (same as the desktop path).
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
        backend_notice: active_session.backend_notice.clone(),
        retrieval_notice: active_session.retrieval_notice.clone(),
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

    // 6. Flip the sticky toggle on. Persist failure is logged, not fatal.
    {
        let mut c = state.config.lock().await;
        c.speech.voice_mode_enabled = true;
        if let Err(e) = crate::config::save(&state.home, &c) {
            tracing::warn!("persist speech.voice_mode_enabled=true failed: {e}");
        }
    }

    Ok(info)
}

/// Stop the active Android voice loop, if any. Delegates to the shared
/// `super::voice::stop_voice_mode_by_user` (which fires the loop's `stop_tx`,
/// joins with a timeout, drops the active session, and flips the sticky
/// toggle off — even when no loop was running, #372). The wrapped join from `start_voice_mode_android` stops the
/// recognizer consumer once the loop exits.
#[tauri::command]
pub async fn stop_voice_mode_android(state: tauri::State<'_, AppState>) -> Result<(), String> {
    super::voice::stop_voice_mode_by_user(&state).await
}

/// Cancel the in-flight LLM call + TTS synthesis for the current Android
/// voice turn. Non-blocking, idempotent — mirrors the desktop
/// `cancel_voice_response`.
#[tauri::command]
pub async fn cancel_voice_response_android(
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let guard = state.voice.lock().await;
    if let Some(active) = guard.as_ref() {
        let _ = active.cancel_response_tx.try_send(());
    }
    Ok(())
}

/// Open the OS app-details settings screen so the user can grant the
/// `RECORD_AUDIO` permission after a denial. Wired to the "Open settings"
/// button on the permission-denied banner (issue #253). Best-effort: errors
/// surface as a dev-facing string the frontend logs.
#[tauri::command]
pub async fn open_app_settings() -> Result<(), String> {
    let bridge = primer_speech::android::new_jni_bridge().map_err(|e| e.to_string())?;
    bridge.open_app_settings().map_err(|e| e.to_string())
}
