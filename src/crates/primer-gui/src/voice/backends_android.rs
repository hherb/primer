//! GUI-side constructor for the Android-native voice backends.
//!
//! Thin wrapper over
//! [`primer_speech::voice_loop::backends_android_native::build_android_voice_backends`]:
//! it takes the live JNI speech bridge (constructed by the caller via
//! [`primer_speech::android::new_jni_bridge`], so the up-front
//! `RECORD_AUDIO`-permission check can run on the same bridge before the loop
//! spawns) and a default [`VoiceProfile`] (the Android TTS plays the OS voice,
//! so the profile's `model_id` is unused beyond locale), then builds the
//! cpal-free backend bundle. Must run inside a tokio runtime — the underlying
//! builder spawns the recognizer-consumer task (the Tauri command is async, so
//! this holds).

use std::sync::Arc;

use primer_core::error::Result;
use primer_core::i18n::Locale;
use primer_core::speech::VoiceProfile;
use primer_speech::android::bridge::AndroidSpeechBridge;
use primer_speech::voice_loop::backends_android_native::build_android_voice_backends;

// Re-exported so the command module can destructure the bundle without
// reaching into the primer-speech module path.
pub use primer_speech::voice_loop::backends_android_native::AndroidVoiceBackends;

/// Build the Android voice backends for `locale` over an existing `bridge`.
/// The OS owns the mic (SpeechRecognizer) and speaker (TextToSpeech), so
/// there is no cpal, no asset resolution, and no download — unlike the
/// desktop path. The caller constructs the bridge so the permission check and
/// the loop share one instance.
pub fn build_android_backends(
    bridge: Arc<dyn AndroidSpeechBridge>,
    locale: Locale,
) -> Result<AndroidVoiceBackends> {
    build_android_voice_backends(bridge, locale, VoiceProfile::default())
}
