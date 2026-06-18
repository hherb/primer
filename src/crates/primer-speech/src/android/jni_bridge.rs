//! placeholder, filled in Task 4 (real JNI). For now a stub that satisfies
//! the `AndroidSpeechBridge` seam so `android::query_capabilities` type-checks
//! on the android target.

use crate::android::bridge::AndroidSpeechBridge;
use crate::android::capabilities::SpeechCapabilities;
use primer_core::error::{PrimerError, Result};

pub struct JniSpeechBridge {
    _private: (),
}

impl JniSpeechBridge {
    pub fn new() -> Result<Self> {
        Err(PrimerError::Speech(
            "JniSpeechBridge is not yet implemented (Task 4)".into(),
        ))
    }
}

impl AndroidSpeechBridge for JniSpeechBridge {
    fn query_capabilities(&self) -> Result<SpeechCapabilities> {
        Err(PrimerError::Speech(
            "JniSpeechBridge is not yet implemented (Task 4)".into(),
        ))
    }
}
