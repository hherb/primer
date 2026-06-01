//! Voice-loop backend construction for the GUI.
//!
//! Thin wrapper around the decoupled `primer_speech::voice_loop` builders
//! (`build_tts` + `build_voice_backends`). Kept as a GUI-side shim so the
//! call site in `commands::voice::start_voice_mode` doesn't have to import
//! the lifted module paths directly, and so the GUI's `(stt, tts)` config
//! enums are converted to the `primer-speech` ones in exactly one place.

use crate::config::{SttBackend, TtsBackend};
use crate::voice::assets::ResolvedAssets;

/// Build every local voice-loop backend, picking the STT skeleton and TTS
/// synthesiser from the decoupled `(stt, tts)` config choice. STT dispatch
/// (whisper / macOS-native) happens inside
/// `primer_speech::voice_loop::build_voice_backends`; the TTS is built here
/// via `build_tts` and injected.
///
/// The caller drains the returned `LocalBackends` into `run_loop` and
/// stashes the post-loop handle so `shutdown()` can run when voice mode
/// ends.
pub async fn build_loop_backends(
    assets: &ResolvedAssets,
    locale: primer_core::i18n::Locale,
    mic_silence_ms: u32,
    stt: SttBackend,
    tts: TtsBackend,
) -> Result<primer_speech::voice_loop::LocalBackends, String> {
    use primer_speech::voice_loop::{TtsAssets, build_tts, build_voice_backends};

    let ps_stt = match stt {
        SttBackend::Whisper => primer_speech::voice_loop::SttBackend::Whisper,
        SttBackend::MacosNative => primer_speech::voice_loop::SttBackend::MacosNative,
    };
    let ps_tts = match tts {
        TtsBackend::Piper => primer_speech::voice_loop::TtsBackend::Piper,
        TtsBackend::Supertonic => primer_speech::voice_loop::TtsBackend::Supertonic,
        TtsBackend::MacosNative => primer_speech::voice_loop::TtsBackend::MacosNative,
    };

    let tts_assets = TtsAssets {
        piper_onnx: Some(assets.piper_onnx.clone()),
        piper_config: Some(assets.piper_config.clone()),
        supertonic_onnx_dir: assets.supertonic_onnx_dir.clone(),
        supertonic_voice_style: assets.supertonic_voice_style.clone(),
        locale,
    };

    let (tts_arc, mut voice) = build_tts(ps_tts, &tts_assets).map_err(|e| e.to_string())?;
    // Honour the resolved Piper voice id (its model_id must match the .onnx
    // stem). Supertonic / macOS-native derive or ignore the id.
    if matches!(ps_tts, primer_speech::voice_loop::TtsBackend::Piper) {
        voice.model_id = assets.voice_id.clone();
    }

    build_voice_backends(
        ps_stt,
        tts_arc,
        voice,
        &assets.whisper_model,
        locale,
        mic_silence_ms,
        // The GUI logs via tracing, never stderr.
        false,
    )
    .await
    .map_err(|e| e.to_string())
}
