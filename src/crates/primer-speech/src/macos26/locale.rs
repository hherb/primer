//! Locale ↔ BCP47 mapping for the `macos-native-26` STT path.
//! Single source of truth; `Macos26Stt::new` calls `to_bcp47` once
//! at construction so a wrong locale is a startup-time error, not a
//! mid-conversation one.

#![cfg(all(target_vendor = "apple", feature = "macos-native-26"))]

use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;

/// Translate a primer `Locale` into the BCP47 string `SpeechTranscriber`
/// expects. Errors loudly on locales SpeechTranscriber doesn't support
/// (Hindi on macOS 26.5).
pub fn to_bcp47(locale: Locale) -> Result<&'static str> {
    match locale {
        Locale::English => Ok("en-US"),
        Locale::German => Ok("de-DE"),
        Locale::Hindi => Err(PrimerError::Speech(
            "Hindi (hi-IN) not yet supported by SpeechTranscriber on macOS 26.5; \
             use --features primer-cli/speech without macos-native-26 for the Whisper path"
                .into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_maps_to_en_us() {
        assert_eq!(to_bcp47(Locale::English).unwrap(), "en-US");
    }

    #[test]
    fn german_maps_to_de_de() {
        assert_eq!(to_bcp47(Locale::German).unwrap(), "de-DE");
    }

    #[test]
    fn hindi_errors_with_helpful_message() {
        let err = to_bcp47(Locale::Hindi).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("Hindi") && msg.contains("hi-IN") && msg.contains("Whisper"),
            "error message should name Hindi, hi-IN, and Whisper as the workaround; got: {msg}"
        );
    }
}
