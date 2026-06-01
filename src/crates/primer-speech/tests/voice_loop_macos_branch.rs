#![cfg(all(target_os = "macos", feature = "macos-native", feature = "voice-loop"))]

use primer_core::i18n::Locale;
use primer_speech::voice_loop::{
    TtsAssets, TtsBackend, build_local_backends_macos_native, build_tts,
};

#[tokio::test]
async fn macos_native_builder_returns_local_backends() {
    // Stage C (#170) decoupled the TTS from the STT builder: the macOS-native
    // builder now takes an injected `Arc<dyn StreamingTextToSpeech>` + voice.
    // Mirror production by constructing the AVSpeech TTS via `build_tts` and
    // injecting it — the same path `primer-cli`'s speech_loop uses.
    let assets = TtsAssets {
        locale: Locale::English,
        ..Default::default()
    };
    let (tts, voice) = match build_tts(TtsBackend::MacosNative, &assets) {
        Ok(pair) => pair,
        Err(e) => {
            // AVSpeech construction shouldn't need a device, but stay tolerant
            // of a constrained CI host rather than panicking.
            eprintln!("macOS-native TTS build returned Err (acceptable in CI): {e}");
            return;
        }
    };
    let result = build_local_backends_macos_native(tts, voice, Locale::English, 600, false).await;
    // We don't assert Ok specifically because the test runner may lack
    // microphone permission, but the call must not panic and must return
    // a typed result. If it does succeed, the LocalBackends struct must
    // be cleanly droppable.
    match result {
        Ok(mut backends) => backends.shutdown(),
        Err(e) => {
            eprintln!(
                "macos-native builder returned Err (acceptable in CI without mic/STT permission): {e}"
            );
        }
    }
}
