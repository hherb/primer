//! Recognizer / synthesizer events the Kotlin side enqueues and Rust
//! drains via `pollSpeechEvent()`. The serde tag/shape is the exact JSON
//! `PrimerSpeech.pollSpeechEvent()` emits (Plan 2 Task 2).

use serde::{Deserialize, Serialize};

/// One event from the Android speech engines.
///
/// `kind`-tagged so the Kotlin side can emit a flat JSON object per event
/// (`{"kind":"partial","text":...}`) and Rust deserialises it directly.
/// The voice loop never sees this type — the recognizer consumer
/// ([`crate::android::stt`]) translates it into `VadEvent` edges plus a
/// committed transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SpeechEvent {
    /// A volatile partial transcript (`onPartialResults`).
    Partial { text: String },
    /// The final transcript for the utterance (`onResults`).
    Final { text: String },
    /// The recognizer detected end-of-speech (`onEndOfSpeech`).
    EndOfSpeech,
    /// Recognizer error (`onError`), carrying the Android error code.
    SttError { code: i32 },
    /// TTS finished speaking the current utterance (`onDone`).
    TtsDone,
    /// TTS error (`onError`).
    TtsError { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_partial_and_final_and_end() {
        let p: SpeechEvent = serde_json::from_str(r#"{"kind":"partial","text":"how do"}"#).unwrap();
        assert_eq!(
            p,
            SpeechEvent::Partial {
                text: "how do".into()
            }
        );
        let f: SpeechEvent =
            serde_json::from_str(r#"{"kind":"final","text":"how do birds fly"}"#).unwrap();
        assert_eq!(
            f,
            SpeechEvent::Final {
                text: "how do birds fly".into()
            }
        );
        let e: SpeechEvent = serde_json::from_str(r#"{"kind":"end_of_speech"}"#).unwrap();
        assert_eq!(e, SpeechEvent::EndOfSpeech);
    }

    #[test]
    fn parses_error_and_tts_events() {
        let s: SpeechEvent = serde_json::from_str(r#"{"kind":"stt_error","code":7}"#).unwrap();
        assert_eq!(s, SpeechEvent::SttError { code: 7 });
        let d: SpeechEvent = serde_json::from_str(r#"{"kind":"tts_done"}"#).unwrap();
        assert_eq!(d, SpeechEvent::TtsDone);
        let t: SpeechEvent =
            serde_json::from_str(r#"{"kind":"tts_error","message":"boom"}"#).unwrap();
        assert_eq!(
            t,
            SpeechEvent::TtsError {
                message: "boom".into()
            }
        );
    }

    #[test]
    fn round_trips_through_serialize() {
        // The Kotlin side parses what Rust would emit; pin the wire tag.
        let json = serde_json::to_string(&SpeechEvent::EndOfSpeech).unwrap();
        assert_eq!(json, r#"{"kind":"end_of_speech"}"#);
        let back: SpeechEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, SpeechEvent::EndOfSpeech);
    }
}
