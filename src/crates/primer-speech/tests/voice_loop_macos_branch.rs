#![cfg(all(target_os = "macos", feature = "macos-native", feature = "voice-loop"))]

use primer_core::i18n::Locale;
use primer_speech::voice_loop::backends::build_local_backends_macos_native;

#[tokio::test]
async fn macos_native_builder_returns_local_backends() {
    let result = build_local_backends_macos_native(Locale::English, 600, false).await;
    // We don't assert Ok specifically because the test runner may lack
    // microphone permission, but the call must not panic and must return
    // a typed result. If it does succeed, the LocalBackends struct must
    // be cleanly droppable.
    match result {
        Ok(mut backends) => backends.shutdown(),
        Err(_) => {
            // Acceptable in CI without mic permission — just don't panic.
        }
    }
}
