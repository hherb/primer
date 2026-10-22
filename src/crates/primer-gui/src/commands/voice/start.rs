//! `start_voice_mode`: builds the voice loop and stashes its handle in
//! `AppState::voice`, plus the non-`speech` stub.

use tauri::AppHandle;

use crate::state::AppState;
use crate::types::SessionInfo;

use super::StartVoiceModeError;
#[cfg(feature = "speech")]
use super::{missing_to_error, stop_voice_mode_inner};

#[cfg(feature = "speech")]
use std::sync::Arc;

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
    crate::commands::session::close_session_inner(&state)
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
            .map_err(|missing| missing_to_error(cfg.speech.disable_auto_download, missing))?;

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
