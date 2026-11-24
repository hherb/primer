//! Pure guard against STT output that is not real speech (issue #326).
//!
//! Whisper does not return an empty string on silence, a breath or room
//! noise. It returns a confident hallucination ("Thank you.",
//! `[BLANK_AUDIO]`, a subtitle credit). Without this guard that text
//! became a persisted child turn and fed the concept extractor and the
//! comprehension classifier, whose depth promotion is monotonic, so a
//! phantom exchange could never be undone. The state machine treats a
//! transcript rejected here exactly like an empty one.
//!
//! The guard is string-based so it also covers the Android and
//! macOS-native recognisers, which expose no no-speech probability.

use primer_core::consts::speech::NOISE_ANNOTATION_MAX_CHARS;
use primer_core::i18n::Locale;

use super::quit_detect::normalise_for_match;

/// Whole-utterance STT silence artefacts per locale, in the
/// [`normalise_for_match`] form. Matched by equality, so "thank you for
/// the story" is kept.
///
/// Shortcut: "thank you" / "vielen dank" are also things a child really
/// says, and such a turn is dropped. Upgrade to Whisper's per-segment
/// `no_speech_prob` if that proves annoying in practice.
fn silence_phrases_for(locale: &Locale) -> &'static [&'static str] {
    match locale.pack_id() {
        "de" => &[
            "vielen dank",
            "danke",
            "danke schön",
            "tschüss und bis zum nächsten mal",
        ],
        _ => &[
            "thank you",
            "thanks for watching",
            "thank you for watching",
            "you",
        ],
    }
}

/// Substrings of the subtitle credits Whisper learned from its training
/// data. No child says these, so any transcript containing one is noise,
/// whatever the locale.
const SUBTITLE_CREDIT_MARKERS: &[&str] = &[
    "amara.org",
    "subtitles by",
    "untertitel im auftrag",
    "untertitelung des",
    "untertitel der",
];

/// True when `transcript` is STT noise rather than something the child
/// said:
/// - it has no letter or digit at all (`"..."`, `"♪♪"`);
/// - it is one short bracketed or asterisked annotation
///   (`[BLANK_AUDIO]`, `(no speech)`, `*sigh*`);
/// - it equals one of the locale's known silence artefacts;
/// - it contains a subtitle credit.
///
/// Short genuine answers ("yes", "no", "4") are kept.
pub(super) fn is_noise_transcript(transcript: &str, locale: &Locale) -> bool {
    let normalised = normalise_for_match(transcript);
    normalised.is_empty()
        || is_annotation(transcript.trim())
        || silence_phrases_for(locale).contains(&normalised.as_str())
        || SUBTITLE_CREDIT_MARKERS
            .iter()
            .any(|m| normalised.contains(m))
}

/// One bracket pair (or one `*…*`) wrapping the whole string, with no
/// closer inside, so `"(I think) yes (maybe)"` is not an annotation.
fn is_annotation(s: &str) -> bool {
    if s.chars().count() > NOISE_ANNOTATION_MAX_CHARS {
        return false;
    }
    let mut chars = s.chars();
    let (Some(first), Some(last)) = (chars.next(), chars.next_back()) else {
        return false;
    };
    let closer = match first {
        '[' => ']',
        '(' => ')',
        '*' => '*',
        _ => return false,
    };
    last == closer && !chars.as_str().contains(closer)
}

#[cfg(test)]
mod tests {
    use super::is_noise_transcript;
    use primer_core::consts::speech::NOISE_ANNOTATION_MAX_CHARS;
    use primer_core::i18n::Locale;

    fn noise_en(s: &str) -> bool {
        is_noise_transcript(s, &Locale::English)
    }

    #[test]
    fn rejects_whisper_annotations() {
        for s in [
            "[BLANK_AUDIO]",
            "(no speech)",
            "*sigh*",
            " [Music] ",
            "(background noise)",
        ] {
            assert!(noise_en(s), "{s:?} should be noise");
        }
    }

    #[test]
    fn rejects_transcripts_without_letters_or_digits() {
        for s in ["...", "♪♪", " - ", "?!"] {
            assert!(noise_en(s), "{s:?} should be noise");
        }
    }

    #[test]
    fn rejects_known_english_silence_phrases() {
        for s in [
            "Thank you.",
            "Thanks for watching!",
            "you",
            "  Thank you for watching. ",
        ] {
            assert!(noise_en(s), "{s:?} should be noise");
        }
    }

    #[test]
    fn rejects_subtitle_credits_in_any_locale() {
        assert!(noise_en("Subtitles by the Amara.org community"));
        assert!(is_noise_transcript(
            "Untertitel im Auftrag des ZDF für funk, 2017",
            &Locale::German
        ));
        assert!(is_noise_transcript(
            "Untertitelung des ZDF, 2020",
            &Locale::German
        ));
    }

    #[test]
    fn german_silence_phrases_are_per_locale() {
        assert!(is_noise_transcript("Vielen Dank.", &Locale::German));
        assert!(!noise_en("Vielen Dank."));
        assert!(!is_noise_transcript(
            "Danke, warum ist der Himmel blau?",
            &Locale::German
        ));
    }

    #[test]
    fn keeps_real_child_speech() {
        for s in [
            "yes",
            "no",
            "4",
            "why is the sky blue",
            "thank you for the story",
            "you are funny",
            "(I think) it is the moon (maybe)",
            "*really* big",
        ] {
            assert!(!noise_en(s), "{s:?} should be kept");
        }
    }

    #[test]
    fn long_bracketed_aside_is_kept() {
        let long = format!("({})", "a".repeat(NOISE_ANNOTATION_MAX_CHARS));
        assert!(!noise_en(&long));
        let short = format!("({})", "a".repeat(NOISE_ANNOTATION_MAX_CHARS - 2));
        assert!(noise_en(&short));
    }
}
