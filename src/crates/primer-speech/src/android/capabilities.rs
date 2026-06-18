//! Pure capability types + offline-voice selection. No JNI here — this is
//! the host-testable decision layer.

use serde::{Deserialize, Serialize};

/// A snapshot of the device's speech capabilities, mirroring the JSON
/// emitted by `PrimerSpeech.queryCapabilities()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechCapabilities {
    /// `SpeechRecognizer.isOnDeviceRecognitionAvailable(context)`.
    pub on_device_recognition_available: bool,
    /// Best-effort on-device recognition locales (BCP-47). May be empty —
    /// Android exposes no synchronous accessor; populated only if a later
    /// async probe lands. Empty is not a failure.
    pub recognition_locales: Vec<String>,
    /// Every installed `TextToSpeech` voice with its offline flags.
    pub tts_voices: Vec<TtsVoiceInfo>,
}

/// One `android.speech.tts.Voice`, reduced to the fields the offline-first
/// guard needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TtsVoiceInfo {
    pub name: String,
    /// BCP-47 from `Voice.getLocale().toLanguageTag()`, e.g. `en-US`.
    pub locale: String,
    /// `Voice.isNetworkConnectionRequired()`.
    pub network_required: bool,
    /// `Voice.getFeatures()` contains `KEY_FEATURE_NOT_INSTALLED`.
    pub not_installed: bool,
}

/// Pick the best fully-offline, installed TTS voice for `bcp47`, or `None`.
/// Exact-locale match wins; otherwise a language-prefix match (`en` matches
/// `en-US`). Network-required or not-installed voices are never returned —
/// strict offline-first.
pub fn select_offline_voice<'a>(
    voices: &'a [TtsVoiceInfo],
    bcp47: &str,
) -> Option<&'a TtsVoiceInfo> {
    let usable = |v: &&TtsVoiceInfo| !v.network_required && !v.not_installed;
    voices
        .iter()
        .find(|v| usable(v) && v.locale.eq_ignore_ascii_case(bcp47))
        .or_else(|| {
            let lang = bcp47.split('-').next().unwrap_or(bcp47);
            voices.iter().find(|v| {
                usable(v)
                    && v.locale
                        .split('-')
                        .next()
                        .unwrap_or("")
                        .eq_ignore_ascii_case(lang)
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kotlin_capabilities_json() {
        let json = r#"{
            "on_device_recognition_available": true,
            "recognition_locales": [],
            "tts_voices": [
                {"name":"en-us-x-sfg#female_1-local","locale":"en-US","network_required":false,"not_installed":false},
                {"name":"en-us-x-iol-network","locale":"en-US","network_required":true,"not_installed":false}
            ]
        }"#;
        let caps: SpeechCapabilities = serde_json::from_str(json).unwrap();
        assert!(caps.on_device_recognition_available);
        assert_eq!(caps.tts_voices.len(), 2);
        assert!(!caps.tts_voices[0].network_required);
        assert!(caps.tts_voices[1].network_required);
    }

    fn v(name: &str, locale: &str, net: bool, missing: bool) -> TtsVoiceInfo {
        TtsVoiceInfo {
            name: name.into(),
            locale: locale.into(),
            network_required: net,
            not_installed: missing,
        }
    }

    #[test]
    fn picks_offline_exact_locale_and_rejects_network() {
        let voices = vec![
            v("net", "en-US", true, false),
            v("offline", "en-US", false, false),
        ];
        assert_eq!(
            select_offline_voice(&voices, "en-US").unwrap().name,
            "offline"
        );
    }

    #[test]
    fn rejects_not_installed_even_if_offline() {
        let voices = vec![v("ghost", "en-US", false, true)];
        assert!(select_offline_voice(&voices, "en-US").is_none());
    }

    #[test]
    fn falls_back_to_language_prefix() {
        let voices = vec![v("gb", "en-GB", false, false)];
        assert_eq!(select_offline_voice(&voices, "en").unwrap().name, "gb");
    }

    #[test]
    fn none_when_only_network_voices() {
        let voices = vec![v("net", "en-US", true, false)];
        assert!(select_offline_voice(&voices, "en-US").is_none());
    }

    #[test]
    fn prefix_fallback_rejects_network_voice() {
        // The only prefix-matching voice is network-required → None, never a
        // network voice (the offline-first invariant holds in the fallback arm).
        let voices = vec![v("net-gb", "en-GB", true, false)];
        assert!(select_offline_voice(&voices, "en").is_none());
    }
}
