//! placeholder, filled in Task 4
//!
//! Minimal scaffold so the `mod jni_bridge` declaration in `android/mod.rs`
//! resolves and `JniSpeechBridge::new()` type-checks on the android target.
//! The real implementation lands in Task 4.

use primer_core::error::{PrimerError, Result};

use super::capabilities::SpeechCapabilities;

/// Scaffold JNI bridge to Android's `SpeechRecognizer` + `TextToSpeech`.
/// Filled in by Task 4; construction always returns an error until then.
pub struct JniSpeechBridge {
    _private: (),
}

impl JniSpeechBridge {
    /// Construct the bridge. Returns an error until Task 4 implements the body.
    pub fn new() -> Result<Self> {
        Err(PrimerError::Speech(
            "JniSpeechBridge is not yet implemented (Task 4)".into(),
        ))
    }

    /// Query the device's speech capabilities.
    pub fn query_capabilities(&self) -> Result<SpeechCapabilities> {
        Err(PrimerError::Speech(
            "JniSpeechBridge is not yet implemented (Task 4)".into(),
        ))
    }
}
