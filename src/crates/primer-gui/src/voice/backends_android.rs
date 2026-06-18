//! GUI-side constructor for the Android-native voice backends.
//!
//! Thin wrapper over
//! [`primer_speech::voice_loop::backends_android_native::build_android_voice_backends`]:
//! it resolves the live JNI speech bridge ([`primer_speech::android::new_jni_bridge`])
//! and a default [`VoiceProfile`] (the Android TTS plays the OS voice, so the
//! profile's `model_id` is unused beyond locale), then builds the cpal-free
//! backend bundle. Must run inside a tokio runtime — the underlying builder
//! spawns the recognizer-consumer task (the Tauri command is async, so this
//! holds).

use primer_core::error::Result;
use primer_core::i18n::Locale;
use primer_core::speech::VoiceProfile;
use primer_speech::voice_loop::backends_android_native::build_android_voice_backends;

// Re-exported so the command module can destructure the bundle without
// reaching into the primer-speech module path.
pub use primer_speech::voice_loop::backends_android_native::AndroidVoiceBackends;

/// Build the Android voice backends for `locale`. The OS owns the mic
/// (SpeechRecognizer) and speaker (TextToSpeech), so there is no cpal,
/// no asset resolution, and no download — unlike the desktop path.
pub fn build_android_backends(locale: Locale) -> Result<AndroidVoiceBackends> {
    let bridge = primer_speech::android::new_jni_bridge()?;
    build_android_voice_backends(bridge, locale, VoiceProfile::default())
}
