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
    /// Whether the `RECORD_AUDIO` runtime permission is currently granted.
    /// The GUI checks this up front before arming the recognizer so a denial
    /// surfaces as a user-visible message rather than a silent `onError`
    /// (`ERROR_INSUFFICIENT_PERMISSIONS`) the child never sees.
    fn has_record_audio_permission(&self) -> Result<bool>;
    /// Open the OS app-details settings screen so the user can grant a
    /// previously denied permission. Best-effort — the recovery affordance
    /// behind the permission-denied message.
    fn open_app_settings(&self) -> Result<()>;
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
        /// Scriptable `RECORD_AUDIO` grant state (default granted). Flip to
        /// `false` to exercise the permission-denied path.
        pub record_audio_permission: bool,
        /// How many times [`AndroidSpeechBridge::open_app_settings`] was called.
        pub settings_opened: Mutex<u32>,
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
                record_audio_permission: true,
                settings_opened: Mutex::new(0),
            }
        }
    }

    impl AndroidSpeechBridge for MockBridge {
        fn query_capabilities(&self) -> Result<SpeechCapabilities> {
            Ok(self.caps.clone())
        }
        fn has_record_audio_permission(&self) -> Result<bool> {
            Ok(self.record_audio_permission)
        }
        fn open_app_settings(&self) -> Result<()> {
            *self.settings_opened.lock().unwrap() += 1;
            Ok(())
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

    #[test]
    fn bridge_reports_record_audio_permission() {
        let mut bridge = MockBridge::with_events(vec![]);
        // Default: granted.
        assert!(bridge.has_record_audio_permission().unwrap());
        // Denied path.
        bridge.record_audio_permission = false;
        assert!(!bridge.has_record_audio_permission().unwrap());
    }

    #[test]
    fn open_app_settings_is_counted() {
        let bridge = MockBridge::with_events(vec![]);
        assert_eq!(*bridge.settings_opened.lock().unwrap(), 0);
        bridge.open_app_settings().unwrap();
        bridge.open_app_settings().unwrap();
        assert_eq!(*bridge.settings_opened.lock().unwrap(), 2);
    }
}
