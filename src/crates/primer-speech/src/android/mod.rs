//! Android-native speech: on-device `SpeechRecognizer` STT + `TextToSpeech`
//! TTS via a Kotlin helper called over JNI. Plan 1 ships only the capability
//! diagnostic; the voice loop lands in Plan 2.

pub mod bridge;
mod capabilities;
pub mod events;
pub mod vad;
pub mod vm;

pub use capabilities::{SpeechCapabilities, TtsVoiceInfo, select_offline_voice};

#[cfg(target_os = "android")]
mod jni_bridge;

use primer_core::error::Result;

/// Query the device's speech capabilities. On Android this drives the real
/// JNI bridge; on every other target it is a platform stub so the crate and
/// its tests still build host-side.
#[cfg(target_os = "android")]
pub fn query_capabilities() -> Result<SpeechCapabilities> {
    use crate::android::bridge::AndroidSpeechBridge;
    jni_bridge::JniSpeechBridge::new()?.query_capabilities()
}

#[cfg(not(target_os = "android"))]
pub fn query_capabilities() -> Result<SpeechCapabilities> {
    Err(primer_core::error::PrimerError::Speech(
        "android speech capabilities are only available on android targets".into(),
    ))
}
