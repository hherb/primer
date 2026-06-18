//! The bridge seam. The Android voice loop routes every device call through
//! an `AndroidSpeechBridge`; the real impl is JNI (android-only,
//! [`crate::android::jni_bridge`]), the `MockBridge` makes the surrounding
//! logic host-testable — the `primer-inference::qnn`
//! `GenieLibrary`/`GenieDialog` pattern.

use crate::android::SpeechCapabilities;
use crate::android::events::SpeechEvent;
use primer_core::error::Result;

/// Everything the Android voice loop needs from the device. The real impl is
/// JNI ([`crate::android::jni_bridge::JniSpeechBridge`], android-only);
/// `MockBridge` (test) drives the host-side logic. All calls are Rust→Kotlin
/// (one direction) — events flow back via [`Self::poll_event`] (the D4 poll
/// model), not Kotlin upcalls.
pub trait AndroidSpeechBridge: Send + Sync {
    /// Query the device's STT/TTS capabilities (Plan 1).
    fn query_capabilities(&self) -> Result<SpeechCapabilities>;
    /// Arm the on-device recognizer for one utterance in `bcp47`.
    fn start_listening(&self, bcp47: &str) -> Result<()>;
    /// Stop / cancel the recognizer.
    fn stop_listening(&self) -> Result<()>;
    /// Pull the next queued speech event, waiting up to `timeout_ms`.
    /// `Ok(None)` = no event within the timeout (caller loops).
    fn poll_event(&self, timeout_ms: u32) -> Result<Option<SpeechEvent>>;
    /// Speak `text` and BLOCK until the engine reports done (D3). On Android
    /// the synthesis *is* the playback.
    fn speak(&self, text: &str) -> Result<()>;
    /// Abort any in-progress speech (GUI Stop / Esc).
    fn cancel_speech(&self) -> Result<()>;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::android::TtsVoiceInfo;
    use std::sync::Mutex;

    /// Scriptable bridge: `events` is drained by `poll_event`; `spoken`
    /// records every `speak`. Construct via [`MockBridge::with_events`].
    /// Visible to sibling test modules (`stt`, `tts`) so the host tests for
    /// the consumer and the TTS reuse one mock.
    pub struct MockBridge {
        pub caps: SpeechCapabilities,
        pub events: Mutex<std::collections::VecDeque<SpeechEvent>>,
        pub spoken: Mutex<Vec<String>>,
    }

    impl MockBridge {
        pub fn with_events(events: Vec<SpeechEvent>) -> Self {
            Self {
                caps: SpeechCapabilities {
                    on_device_recognition_available: true,
                    recognition_locales: vec![],
                    tts_voices: vec![TtsVoiceInfo {
                        name: "offline".into(),
                        locale: "en-US".into(),
                        network_required: false,
                        not_installed: false,
                    }],
                },
                events: Mutex::new(events.into()),
                spoken: Mutex::new(vec![]),
            }
        }
    }

    impl AndroidSpeechBridge for MockBridge {
        fn query_capabilities(&self) -> Result<SpeechCapabilities> {
            Ok(self.caps.clone())
        }
        fn start_listening(&self, _bcp47: &str) -> Result<()> {
            Ok(())
        }
        fn stop_listening(&self) -> Result<()> {
            Ok(())
        }
        fn poll_event(&self, _timeout_ms: u32) -> Result<Option<SpeechEvent>> {
            Ok(self.events.lock().unwrap().pop_front())
        }
        fn speak(&self, text: &str) -> Result<()> {
            self.spoken.lock().unwrap().push(text.to_string());
            Ok(())
        }
        fn cancel_speech(&self) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn bridge_returns_capabilities() {
        let bridge = MockBridge::with_events(vec![]);
        let caps = bridge.query_capabilities().unwrap();
        assert!(caps.on_device_recognition_available);
        assert_eq!(caps.tts_voices.len(), 1);
    }

    #[test]
    fn mock_bridge_polls_scripted_events_in_order() {
        let bridge = MockBridge::with_events(vec![
            SpeechEvent::Partial { text: "how".into() },
            SpeechEvent::Final {
                text: "how do birds fly".into(),
            },
            SpeechEvent::EndOfSpeech,
        ]);
        assert_eq!(
            bridge.poll_event(0).unwrap(),
            Some(SpeechEvent::Partial { text: "how".into() })
        );
        assert_eq!(
            bridge.poll_event(0).unwrap(),
            Some(SpeechEvent::Final {
                text: "how do birds fly".into()
            })
        );
        assert_eq!(
            bridge.poll_event(0).unwrap(),
            Some(SpeechEvent::EndOfSpeech)
        );
        assert_eq!(bridge.poll_event(0).unwrap(), None);
    }

    #[test]
    fn speak_records_spoken_text() {
        let bridge = MockBridge::with_events(vec![]);
        bridge.speak("hello there").unwrap();
        assert_eq!(bridge.spoken.lock().unwrap().as_slice(), ["hello there"]);
    }
}
