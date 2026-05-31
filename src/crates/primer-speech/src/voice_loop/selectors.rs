//! Runtime STT/TTS backend selectors and the shared construction helpers
//! that turn a `(SttBackend, TtsBackend)` choice plus assets into a built
//! voice-loop backend set. STT picks which builder skeleton runs; TTS picks
//! which `Arc<dyn StreamingTextToSpeech>` is injected. They vary
//! independently — see
//! `docs/superpowers/specs/2026-05-31-supertonic-stage-c-decoupled-speech-design.md`.

use serde::{Deserialize, Serialize};

/// Which speech-to-text builder skeleton the voice loop runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SttBackend {
    /// whisper.cpp streaming STT (cross-platform; the only STT on a
    /// default build).
    #[default]
    Whisper,
    /// Apple Speech.framework STT (macOS-only; SFSpeechRecognizer on
    /// `macos-native`, SpeechAnalyzer on `macos-native-26`).
    MacosNative,
}

/// Which synthesiser the voice loop injects as its TTS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TtsBackend {
    /// Piper ONNX voices (the cross-platform default).
    #[default]
    Piper,
    /// Supertonic 3 multilingual TTS (Hindi/Japanese unlock).
    Supertonic,
    /// Apple AVSpeechSynthesizer (macOS-only).
    MacosNative,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stt_backend_serde_round_trips_kebab_case() {
        assert_eq!(
            serde_json::to_string(&SttBackend::Whisper).unwrap(),
            "\"whisper\""
        );
        assert_eq!(
            serde_json::to_string(&SttBackend::MacosNative).unwrap(),
            "\"macos-native\""
        );
        let parsed: SttBackend = serde_json::from_str("\"macos-native\"").unwrap();
        assert_eq!(parsed, SttBackend::MacosNative);
    }

    #[test]
    fn tts_backend_serde_round_trips_kebab_case() {
        assert_eq!(
            serde_json::to_string(&TtsBackend::Piper).unwrap(),
            "\"piper\""
        );
        assert_eq!(
            serde_json::to_string(&TtsBackend::Supertonic).unwrap(),
            "\"supertonic\""
        );
        assert_eq!(
            serde_json::to_string(&TtsBackend::MacosNative).unwrap(),
            "\"macos-native\""
        );
    }

    #[test]
    fn defaults_are_whisper_and_piper() {
        assert_eq!(SttBackend::default(), SttBackend::Whisper);
        assert_eq!(TtsBackend::default(), TtsBackend::Piper);
    }
}
