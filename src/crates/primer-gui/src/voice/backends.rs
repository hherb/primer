//! Voice-loop backend construction for the GUI.
//!
//! Thin wrapper around `primer_speech::voice_loop::build_local_backends`.
//! Kept as a GUI-side shim so that the call site in
//! `commands::voice::start_voice_mode` doesn't have to import the
//! lifted module path directly — easier to swap or wrap later if the
//! GUI ever needs to inject its own audio strategy.

use crate::voice::assets::ResolvedAssets;

/// Build every local voice-loop backend (cpal mic + speaker, silero VAD,
/// whisper STT, piper TTS, audio capture thread, `on_audio` closure,
/// drain hook). The caller drains the returned `LocalBackends` into
/// `run_loop` and stashes the post-loop handle so `shutdown()` can run
/// when voice mode ends.
pub async fn build_loop_backends(
    assets: &ResolvedAssets,
    locale: primer_core::i18n::Locale,
    mic_silence_ms: u32,
) -> Result<primer_speech::voice_loop::LocalBackends, String> {
    primer_speech::voice_loop::build_local_backends(
        &assets.piper_onnx,
        &assets.piper_config,
        &assets.whisper_model,
        &assets.voice_id,
        locale,
        mic_silence_ms,
        // The GUI logs via tracing, never stderr — keep build_local_backends
        // quiet about device-open rates.
        false,
    )
    .await
    .map_err(|e| e.to_string())
}
