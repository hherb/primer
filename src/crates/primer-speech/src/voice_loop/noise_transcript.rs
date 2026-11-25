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

/// Whole-utterance STT silence artefacts, in the [`normalise_for_match`]
/// form. German has its own list; every other locale uses the English
/// one. Matched by equality, so "thank you for the story" is kept.
///
/// Shortcut: a child who says only one of these ("thank you", "you",
/// "danke") is dropped too. Upgrade to Whisper's per-segment
/// `no_speech_prob` if that proves annoying in practice.
fn silence_phrases_for(locale: &Locale) -> &'static [&'static str] {
    match locale.pack_id() {
        "de" => &[
            "vielen dank",
            "danke",
            "danke schön",
            "dankeschön",
            "vielen dank fürs zuschauen",
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

/// What the child actually said, with STT noise removed. Empty when the
/// whole transcript is noise:
/// - it has no letter or digit at all (`"..."`, `"♪♪"`);
/// - it is one short bracketed or asterisked annotation
///   (`[BLANK_AUDIO]`, `(no speech)`, `*sigh*`), possibly repeated;
/// - every sentence in it is one of the locale's known silence artefacts
///   (`"Thank you. Thank you."`);
/// - it contains a subtitle credit.
///
/// Otherwise short `[…]` / `(…)` annotations at either end are dropped
/// (`"[BLANK_AUDIO] why is the sky blue"`, issue #419) and the rest is
/// kept verbatim. Short genuine answers ("yes", "no", "4") and a silence
/// phrase inside real speech ("Thank you. Why …?") are kept. Whisper's
/// segment boundaries are already gone here (the audio thread joins
/// them), so this works on the joined string.
pub(super) fn strip_noise<'a>(transcript: &'a str, locale: &Locale) -> &'a str {
    let speech = strip_edge_annotations(transcript.trim());
    let phrases = silence_phrases_for(locale);
    let only_silence = speech
        .split(['.', '!', '?'])
        .map(normalise_for_match)
        .filter(|s| !s.is_empty())
        .all(|s| phrases.contains(&s.as_str()));
    let normalised = normalise_for_match(transcript);
    let credit = SUBTITLE_CREDIT_MARKERS
        .iter()
        .any(|m| normalised.contains(m));
    if only_silence || credit || is_annotation(speech) {
        ""
    } else {
        speech
    }
}

/// Drops short `[…]` / `(…)` annotations at either end, repeatedly
/// (`"[BLANK_AUDIO] [BLANK_AUDIO]"`, `"(music) why …"`). `*…*` is left
/// alone: at an edge it is as likely emphasis (`"*really* big"`) as a
/// sound tag. A parenthesised spoken aside at an edge ("(I think) …") is
/// dropped too; Whisper rarely brackets real words.
fn strip_edge_annotations(mut s: &str) -> &str {
    let short = |tag: &str| tag.chars().count() <= NOISE_ANNOTATION_MAX_CHARS;
    loop {
        let before = s;
        for (open, close) in [('[', ']'), ('(', ')')] {
            if s.starts_with(open)
                && let Some(end) = s.find(close)
                && short(&s[..=end])
            {
                s = s[end + 1..].trim_start();
            }
            if s.ends_with(close)
                && let Some(start) = s.rfind(open)
                && short(&s[start..])
            {
                s = s[..start].trim_end();
            }
        }
        if s == before {
            return s;
        }
    }
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
    use super::strip_noise;
    use primer_core::consts::speech::NOISE_ANNOTATION_MAX_CHARS;
    use primer_core::i18n::Locale;

    fn is_noise_transcript(s: &str, locale: &Locale) -> bool {
        strip_noise(s, locale).is_empty()
    }

    fn noise_en(s: &str) -> bool {
        is_noise_transcript(s, &Locale::English)
    }

    #[test]
    fn rejects_repeated_annotations_and_silence_phrases() {
        for s in [
            " [BLANK_AUDIO] [BLANK_AUDIO]",
            "[Music] (upbeat music)",
            "Thank you. Thank you.",
            "Thank you! You.",
            "[BLANK_AUDIO] Thank you.",
        ] {
            assert!(noise_en(s), "{s:?} should be noise");
        }
        assert!(is_noise_transcript("Danke. Vielen Dank.", &Locale::German));
    }

    #[test]
    fn strips_edge_annotations_around_real_speech() {
        for (s, kept) in [
            ("[BLANK_AUDIO] why is the sky blue", "why is the sky blue"),
            ("why is the sky blue [BLANK_AUDIO]", "why is the sky blue"),
            ("(coughs) [Music] is it hot? (sighs)", "is it hot?"),
            ("*really* big", "*really* big"),
            ("why [BLANK_AUDIO] is it", "why [BLANK_AUDIO] is it"),
            ("Thank you. Why is it hot?", "Thank you. Why is it hot?"),
            ("how old are you", "how old are you"),
        ] {
            assert_eq!(strip_noise(s, &Locale::English), kept, "{s:?}");
        }
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
        for s in ["Vielen Dank.", "Dankeschön!", "Vielen Dank fürs Zuschauen."] {
            assert!(
                is_noise_transcript(s, &Locale::German),
                "{s:?} should be noise"
            );
        }
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
