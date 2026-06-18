//! The bridge seam. `query_capabilities()` on the module routes through an
//! `AndroidSpeechBridge`; the real impl is JNI (android-only), the mock makes
//! the surrounding logic host-testable — the `primer-inference::qnn`
//! `GenieLibrary`/`GenieDialog` pattern.

use crate::android::SpeechCapabilities;
use primer_core::error::Result;

/// Everything the diagnostic needs from the device. Plan 2 extends this trait
/// with `start_listening` / `speak` / `cancel` / `poll_event`.
pub trait AndroidSpeechBridge: Send {
    fn query_capabilities(&self) -> Result<SpeechCapabilities>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::android::TtsVoiceInfo;

    struct MockBridge(SpeechCapabilities);
    impl AndroidSpeechBridge for MockBridge {
        fn query_capabilities(&self) -> Result<SpeechCapabilities> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn bridge_returns_capabilities() {
        let caps = SpeechCapabilities {
            on_device_recognition_available: true,
            recognition_locales: vec![],
            tts_voices: vec![TtsVoiceInfo {
                name: "offline".into(),
                locale: "en-US".into(),
                network_required: false,
                not_installed: false,
            }],
        };
        let bridge = MockBridge(caps.clone());
        assert_eq!(bridge.query_capabilities().unwrap(), caps);
    }
}
