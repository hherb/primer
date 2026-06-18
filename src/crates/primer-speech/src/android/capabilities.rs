//! Types describing on-device Android speech capabilities.
//!
//! `SpeechCapabilities` is the result of querying the Android device for
//! installed STT recognisers and TTS voices. It is returned by
//! `android::query_capabilities()` and is the host-visible output of the
//! JNI bridge.

/// A single installed TTS voice available on the Android device.
#[derive(Debug, Clone)]
pub struct TtsVoiceInfo {
    /// BCP-47 locale tag (e.g. `"en-US"`, `"de-DE"`).
    pub locale: String,
    /// Human-readable voice name as reported by Android.
    pub name: String,
    /// Whether the voice can be used without a network connection.
    pub is_network_connection_required: bool,
}

/// Describes the speech capabilities available on the current Android device.
#[derive(Debug, Clone, Default)]
pub struct SpeechCapabilities {
    /// Whether an on-device (`EXTRA_PREFER_OFFLINE = true`) STT recogniser is
    /// available for the requested locale.
    pub offline_stt_available: bool,
    /// All TTS voices reported by the default TextToSpeech engine.
    pub tts_voices: Vec<TtsVoiceInfo>,
}

/// Select the best offline voice for `locale` from `caps`, or `None` if no
/// offline voice is available.
pub fn select_offline_voice<'a>(
    caps: &'a SpeechCapabilities,
    locale: &str,
) -> Option<&'a TtsVoiceInfo> {
    caps.tts_voices
        .iter()
        .find(|v| !v.is_network_connection_required && v.locale.starts_with(locale))
}
