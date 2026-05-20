//! macOS 26 / iOS 26 SpeechAnalyzer-backed STT + VAD.
//!
//! See [`docs/superpowers/specs/2026-05-20-macos-native-26-design.md`].
//!
//! Cfg gates in this module use `target_vendor = "apple"` rather than
//! `target_os = "macos"` because the underlying APIs are identical
//! across all Apple platforms (iOS 26+, iPadOS 26+, visionOS 26+, tvOS 26+,
//! macOS 26+). Files that genuinely diverge between macOS and iOS
//! concentrate that divergence in [`audio_session`].
//!
//! Module rename to `apple26/` is a mechanical follow-up once an iOS
//! host application actually exists in the repo — see the design doc
//! "Goals" section.

pub mod audio_session;

use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;

/// Placeholder `Macos26Stt` stub for integration smoke tests.
/// Full implementation pending (spec: macos-native-26 tasks 4-14).
#[derive(Debug, Clone)]
pub struct Macos26Stt {
    locale: Locale,
}

impl Macos26Stt {
    /// Construct a new STT instance for the given locale.
    /// Validates locale support (en-US, de-DE only; Hindi deferred).
    pub async fn new(locale: Locale) -> Result<Self> {
        match locale {
            Locale::English | Locale::German => {
                Ok(Macos26Stt { locale })
            }
            Locale::Hindi => {
                Err(PrimerError::Speech(
                    "Hindi (hi-IN) not yet supported by SpeechTranscriber on macOS 26.5; \
                     use --features primer-cli/speech without macos-native-26 for the Whisper path".into()
                ))
            }
        }
    }

    /// Return the locale this instance was constructed with.
    pub fn locale(&self) -> Locale {
        self.locale
    }
}
