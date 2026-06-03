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
    (
        "hi",
        LocaleDefault {
            piper_voice_id: "hi_IN-rohan-medium",
            piper_onnx_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/hi/hi_IN/rohan/medium/hi_IN-rohan-medium.onnx",
            piper_config_url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/hi/hi_IN/rohan/medium/hi_IN-rohan-medium.onnx.json",
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

/// Which Supertonic asset slot a file belongs to. The six model files share
/// one `onnx/` directory (the loader reads them all from a single dir); the
/// voice-style JSON is a sibling under `voice_styles/`. The slot decides how
/// the effective on-disk path is derived when a per-locale override is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupertonicSlot {
    /// A file inside the shared `onnx/` model directory.
    OnnxDir,
    /// The per-voice style JSON.
    VoiceStyle,
}

/// One downloadable Supertonic asset.
///
/// Unlike [`LocaleDefault`], this table is **locale-independent**: Supertonic
/// is one multilingual model (31 languages), so the same bundle serves every
/// locale. Voice styles (F1..F5, M1..M5) are personas, not languages; Stage D
/// ships only the default `F1` voice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupertonicAsset {
    /// `MissingAsset` `kind` string (e.g. `"supertonic_vocoder"`).
    pub kind: &'static str,
    /// File name on disk and the tail of the download URL.
    pub file_name: &'static str,
    /// Which slot (onnx dir vs voice-style file) this asset occupies.
    pub slot: SupertonicSlot,
    /// Direct Hugging Face download URL.
    pub url: &'static str,
    /// Approximate on-disk size in MB. Drives the oversize cap and the
    /// consent-modal budget. Floored at 1 for the tiny JSON files.
    pub approx_size_mb: u32,
}

/// Default voice-style file shipped by Stage D. The default cache path and
/// the [`SUPERTONIC_ASSETS`] voice-style row both reference this so they can
/// never drift.
pub const DEFAULT_SUPERTONIC_VOICE_STYLE_FILE: &str = "F1.json";

/// The 7 files that make up the default Supertonic bundle (F1 voice).
/// Source: `huggingface.co/Supertone/supertonic-3` (sizes rounded up).
pub const SUPERTONIC_ASSETS: &[SupertonicAsset] = &[
    SupertonicAsset {
        kind: "supertonic_vector_estimator",
        file_name: "vector_estimator.onnx",
        slot: SupertonicSlot::OnnxDir,
        url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/onnx/vector_estimator.onnx",
        approx_size_mb: 245,
    },
    SupertonicAsset {
        kind: "supertonic_vocoder",
        file_name: "vocoder.onnx",
        slot: SupertonicSlot::OnnxDir,
        url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/onnx/vocoder.onnx",
        approx_size_mb: 97,
    },
    SupertonicAsset {
        kind: "supertonic_text_encoder",
        file_name: "text_encoder.onnx",
        slot: SupertonicSlot::OnnxDir,
        url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/onnx/text_encoder.onnx",
        approx_size_mb: 35,
    },
    SupertonicAsset {
        kind: "supertonic_duration_predictor",
        file_name: "duration_predictor.onnx",
        slot: SupertonicSlot::OnnxDir,
        url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/onnx/duration_predictor.onnx",
        approx_size_mb: 4,
    },
    SupertonicAsset {
        kind: "supertonic_tts_config",
        file_name: "tts.json",
        slot: SupertonicSlot::OnnxDir,
        url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/onnx/tts.json",
        approx_size_mb: 1,
    },
    SupertonicAsset {
        kind: "supertonic_unicode_indexer",
        file_name: "unicode_indexer.json",
        slot: SupertonicSlot::OnnxDir,
        url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/onnx/unicode_indexer.json",
        approx_size_mb: 1,
    },
    SupertonicAsset {
        kind: "supertonic_voice_style",
        file_name: "F1.json",
        slot: SupertonicSlot::VoiceStyle,
        url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/voice_styles/F1.json",
        approx_size_mb: 1,
    },
];

/// The default Supertonic bundle (locale-independent). See
/// [`SUPERTONIC_ASSETS`].
pub fn supertonic_assets() -> &'static [SupertonicAsset] {
    SUPERTONIC_ASSETS
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
    fn hindi_default_is_rohan_plus_small_multilingual() {
        let d = voice_default_for(&Locale::Hindi).expect("hi is pinned");
        assert_eq!(d.piper_voice_id, "hi_IN-rohan-medium");
        // Multilingual Whisper, not the .en-only variant — Hindi is
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

    #[test]
    fn supertonic_assets_has_seven_entries_six_onnx_one_style() {
        let assets = supertonic_assets();
        assert_eq!(assets.len(), 7, "6 onnx files + 1 voice style");
        let onnx = assets
            .iter()
            .filter(|a| matches!(a.slot, SupertonicSlot::OnnxDir))
            .count();
        let style = assets
            .iter()
            .filter(|a| matches!(a.slot, SupertonicSlot::VoiceStyle))
            .count();
        assert_eq!(onnx, 6, "six files live in the onnx/ dir");
        assert_eq!(style, 1, "one voice-style JSON");
    }

    #[test]
    fn supertonic_asset_urls_are_under_supertonic_3_repo() {
        for a in supertonic_assets() {
            assert!(
                a.url
                    .starts_with("https://huggingface.co/Supertone/supertonic-3/resolve/main/"),
                "{} url not under the supertonic-3 repo: {}",
                a.kind,
                a.url,
            );
            assert!(
                a.url.ends_with(a.file_name),
                "{} url/file_name mismatch",
                a.kind
            );
        }
    }

    #[test]
    fn supertonic_asset_kinds_are_unique_and_prefixed() {
        let assets = supertonic_assets();
        for a in assets {
            assert!(
                a.kind.starts_with("supertonic_"),
                "{} kind unprefixed",
                a.kind
            );
        }
        let mut kinds: Vec<&str> = assets.iter().map(|a| a.kind).collect();
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(kinds.len(), assets.len(), "kinds must be unique");
    }

    #[test]
    fn supertonic_total_size_is_sane() {
        let total: u32 = supertonic_assets().iter().map(|a| a.approx_size_mb).sum();
        assert!(
            (350..=420).contains(&total),
            "supertonic bundle total {total} MB outside expected ~380 MB band",
        );
        for a in supertonic_assets() {
            assert!(a.approx_size_mb >= 1, "{} size floored at 1 MB", a.kind);
        }
    }

    #[test]
    fn default_voice_style_file_is_an_entry() {
        let style = supertonic_assets()
            .iter()
            .find(|a| matches!(a.slot, SupertonicSlot::VoiceStyle))
            .expect("one voice-style asset");
        assert_eq!(style.file_name, DEFAULT_SUPERTONIC_VOICE_STYLE_FILE);
    }
}
