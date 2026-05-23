//! Voice-loop backend construction for the GUI.
//!
//! Thin wrapper around `primer_speech::voice_loop::build_local_backends`.
//! Kept as a GUI-side shim so that the call site in
//! `commands::voice::start_voice_mode` doesn't have to import the
//! lifted module path directly — easier to swap or wrap later if the
//! GUI ever needs to inject its own audio strategy.

use crate::config::SpeechBackend;
use crate::voice::assets::ResolvedAssets;

/// Build every local voice-loop backend (cpal mic + speaker, silero VAD,
/// whisper STT, piper TTS, audio capture thread, `on_audio` closure,
/// drain hook). The caller drains the returned `LocalBackends` into
/// `run_loop` and stashes the post-loop handle so `shutdown()` can run
/// when voice mode ends.
///
/// When `backend` is [`SpeechBackend::MacosNative`] **and** this binary
/// was compiled with `--features primer-gui/macos-native` on macOS, the
/// Apple-native SFSpeechRecognizer + AVSpeechSynthesizer stack is used
/// instead of whisper/piper. On all other combinations the function falls
/// through to the whisper/piper path unconditionally.
pub async fn build_loop_backends(
    assets: &ResolvedAssets,
    locale: primer_core::i18n::Locale,
    mic_silence_ms: u32,
    backend: SpeechBackend,
) -> Result<primer_speech::voice_loop::LocalBackends, String> {
    // ── macOS 26 native branch ──────────────────────────────────────
    // Most-specific arm first. Both compile-time cfg AND runtime config
    // must select it. `not(feature = "macos-native")` is belt-and-
    // suspenders — the compile_error in lib.rs already prevents both
    // features being active simultaneously, but the explicit guard makes
    // the intent visible to readers.
    #[cfg(all(
        target_os = "macos",
        feature = "macos-native-26",
        not(feature = "macos-native"),
    ))]
    {
        if matches!(backend, SpeechBackend::MacosNative) {
            return primer_speech::voice_loop::backends::build_local_backends_macos_native_26(
                locale,
                mic_silence_ms,
                // The GUI logs via tracing, never stderr.
                false,
            )
            .await
            .map_err(|e| e.to_string());
        }
    }

    // ── macOS-native branch (older macOS, SFSpeechRecognizer + AVSpeechSynthesizer) ──
    // Both the compile-time cfg AND the runtime config must select it.
    // This lets an evaluator flip A/B via gui-config.json without
    // rebuilding, while non-macOS / non-feature builds get the
    // whisper-piper path at zero cost.
    #[cfg(all(
        target_os = "macos",
        feature = "macos-native",
        not(feature = "macos-native-26"),
    ))]
    {
        if matches!(backend, SpeechBackend::MacosNative) {
            return primer_speech::voice_loop::backends::build_local_backends_macos_native(
                locale,
                mic_silence_ms,
                // The GUI logs via tracing, never stderr.
                false,
            )
            .await
            .map_err(|e| e.to_string());
        }
    }

    // ── Whisper/Piper fallthrough ────────────────────────────────────
    // Reached for:
    //   - non-macOS builds,
    //   - macOS builds without `macos-native` feature, OR
    //   - macOS+macos-native builds where config selects whisper-piper.
    let _ = backend; // suppress unused-variable warning on non-macOS builds
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
