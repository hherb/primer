//! Locale → default voice/STT model mapping.
//!
//! Each locale pack ships a default voice and Whisper model. When the
//! user has not explicitly overridden in Settings → Speech, asset
//! resolution looks here for the canonical Hugging Face URLs +
//! cache-relative paths.
//!
//! Adding a new locale: append a new tuple. The `whisper_model_id`
//! convention follows the `whisper.cpp` filenames (`ggml-<size>.bin`
//! for multilingual, `ggml-<size>.en.bin` for English-only).

use primer_core::i18n::Locale;

/// Default voice + STT pinning for one locale pack.
///
/// Not `Copy` on purpose: a future override field that carries an owned
/// type (e.g. `Option<String>`) would break callers that implicitly copy.
/// `voice_default_for` returns `&'static LocaleDefault` so no caller
/// needs to copy today.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocaleDefault {
    /// Piper voice id matching the .onnx filename stem.
    pub piper_voice_id: &'static str,
    /// Direct download URL for the .onnx weights from Hugging Face.
    pub piper_onnx_url: &'static str,
    /// Direct download URL for the matching .onnx.json config.
    pub piper_config_url: &'static str,
    /// Whisper model id (matches the file name in
    /// `~/.cache/primer/models/whisper/`).
    pub whisper_model_id: &'static str,
    /// Direct download URL for the Whisper .bin from Hugging Face.
    pub whisper_url: &'static str,
    /// Sum of Piper + Whisper file sizes, in megabytes, rounded.
    /// Used by the consent dialog to show "Download (≈540 MB)".
    pub approx_total_mb: u32,
}

/// Mapping from `Locale::pack_id()` to its default voice/STT bundle.
pub const LOCALE_DEFAULTS: &[(&str, LocaleDefault)] = &[
    (
        "en",
        LocaleDefault {
            piper_voice_id: "en_GB-alba-medium",
            piper_onnx_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_GB/alba/medium/en_GB-alba-medium.onnx",
            piper_config_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_GB/alba/medium/en_GB-alba-medium.onnx.json",
            whisper_model_id: "ggml-small.en.bin",
            whisper_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin",
            approx_total_mb: 530,
        },
    ),
    (
        "de",
        LocaleDefault {
            piper_voice_id: "de_DE-thorsten-medium",
            piper_onnx_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/de/de_DE/thorsten/medium/de_DE-thorsten-medium.onnx",
            piper_config_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/de/de_DE/thorsten/medium/de_DE-thorsten-medium.onnx.json",
            whisper_model_id: "ggml-small.bin",
            whisper_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
            approx_total_mb: 540,
        },
    ),
];

/// Look up the default voice/STT bundle for `locale`, if one is pinned.
/// Returns `None` for locales that don't ship a default — the caller
/// must surface an "explicit Settings → Speech path required" error
/// to the user in that case.
pub fn voice_default_for(locale: &Locale) -> Option<&'static LocaleDefault> {
    LOCALE_DEFAULTS
        .iter()
        .find(|(id, _)| *id == locale.pack_id())
        .map(|(_, d)| d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_default_is_alba_plus_small_en() {
        let d = voice_default_for(&Locale::English).expect("en is pinned");
        assert_eq!(d.piper_voice_id, "en_GB-alba-medium");
        assert_eq!(d.whisper_model_id, "ggml-small.en.bin");
    }

    #[test]
    fn german_default_is_thorsten_plus_small_multilingual() {
        let d = voice_default_for(&Locale::German).expect("de is pinned");
        assert_eq!(d.piper_voice_id, "de_DE-thorsten-medium");
        // Multilingual Whisper, not the .en-only variant — German is
        // not in small.en's training set.
        assert_eq!(d.whisper_model_id, "ggml-small.bin");
    }

    #[test]
    fn all_urls_resolve_under_huggingface_co() {
        // Pin the source so a future "use a mirror" PR is explicit
        // rather than a silent URL swap that escapes review.
        for (_, d) in LOCALE_DEFAULTS {
            assert!(d.piper_onnx_url.starts_with("https://huggingface.co/"));
            assert!(d.piper_config_url.starts_with("https://huggingface.co/"));
            assert!(d.whisper_url.starts_with("https://huggingface.co/"));
        }
    }

    #[test]
    fn approx_total_mb_is_sane() {
        // A defensive lower bound: a Whisper small is ~470 MB by itself,
        // a Piper medium voice is ~60 MB. Any default below 400 MB is
        // a typo.
        for (id, d) in LOCALE_DEFAULTS {
            assert!(
                d.approx_total_mb >= 400,
                "{} default total of {} MB is suspiciously low",
                id,
                d.approx_total_mb,
            );
            // Whisper large-v3 is ~1.5 GB; a Piper medium voice is ~60 MB.
            // A 1600 MB ceiling covers any plausible bundle while flagging
            // an accidental order-of-magnitude typo.
            assert!(
                d.approx_total_mb <= 1600,
                "{} default total of {} MB is suspiciously high",
                id,
                d.approx_total_mb,
            );
        }
    }
}
