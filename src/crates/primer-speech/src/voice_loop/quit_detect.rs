//! Pure helpers for locale-aware voice quit-phrase detection.
//!
//! Extracted verbatim from `state_machine.rs`: the state machine calls
//! [`is_quit_phrase`] on each finalized transcript to decide whether the
//! child asked to end the session. Keeping the locale phrase table, the
//! normalisation, and the equality check together (with their tests) in
//! one leaf module keeps `state_machine.rs` focused on the loop itself.

/// Per-locale quit phrases. If heard in the child's transcript, the
/// session ends. Case-insensitive, word-boundary match (see
/// [`is_quit_phrase`]). Each locale ships its own set so a child can
/// quit in the language they speak — without a locale-aware list, the
/// German voice mode silently lacks any voice-keyword end affordance.
///
/// Adding a new locale: append a `(pack_id, &[phrase, ...])` row. The
/// pack_id must match `Locale::pack_id()` for the corresponding locale.
fn quit_phrases_for(locale: &primer_core::i18n::Locale) -> &'static [&'static str] {
    match locale.pack_id() {
        "de" => &[
            // "Tschüss" (informal goodbye) — the most natural for a child.
            "tschüss",
            // Formal goodbye.
            "auf wiedersehen",
            // Primer-direct variants, mirroring the EN set.
            "bye primer",
            "stop primer",
        ],
        // English is the default for any unrecognised locale.
        _ => &["goodbye", "bye primer", "stop primer"],
    }
}

/// Returns true if `transcript`, after trimming surrounding whitespace
/// and punctuation, **equals** one of `locale`'s quit phrases
/// (case-insensitive).
///
/// Why exact-equality rather than `contains` or word-boundary: the
/// pre-fix `contains` would end the session on *"I don't want to stop
/// primer"* because the substring `"stop primer"` matched. Word-boundary
/// matching alone doesn't help — end-of-string is itself a word boundary.
/// The only safe contract for an auto-quit voice keyword is "the child
/// said exactly the keyword, nothing else." Children can always say
/// "goodbye" by itself; this also matches the way real children end
/// conversations.
///
/// Trimming punctuation handles Whisper's habit of producing trailing
/// `.` / `!` / `?` on a finalized utterance: `"Goodbye!"` still ends the
/// session. Internal whitespace is normalised to single spaces so the
/// transcript `"bye   primer"` matches `"bye primer"`.
pub(super) fn is_quit_phrase(transcript: &str, locale: &primer_core::i18n::Locale) -> bool {
    let normalised = normalise_for_match(transcript);
    quit_phrases_for(locale)
        .iter()
        .any(|p| normalised == normalise_for_match(p))
}

/// Lowercase, strip leading/trailing whitespace + punctuation, collapse
/// internal whitespace to single spaces. The result is the canonical
/// form used for quit-phrase equality.
///
/// `char::is_alphanumeric` is Unicode-aware so German `ü`/`ö`/`ä` are
/// preserved; only punctuation and non-letter symbols get trimmed.
fn normalise_for_match(s: &str) -> String {
    let lower = s.to_lowercase();
    let trimmed = lower.trim_matches(|c: char| !c.is_alphanumeric());
    let mut out = String::with_capacity(trimmed.len());
    let mut prev_was_space = false;
    for c in trimmed.chars() {
        if c.is_whitespace() {
            if !prev_was_space {
                out.push(' ');
                prev_was_space = true;
            }
        } else {
            out.push(c);
            prev_was_space = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::is_quit_phrase;
    use primer_core::i18n::Locale;

    #[test]
    fn detects_goodbye_case_insensitive() {
        assert!(is_quit_phrase("Goodbye!", &Locale::English));
        assert!(is_quit_phrase("GOODBYE", &Locale::English));
    }

    #[test]
    fn detects_bye_primer() {
        assert!(is_quit_phrase("bye primer", &Locale::English));
        assert!(is_quit_phrase("Bye Primer.", &Locale::English));
    }

    #[test]
    fn ignores_unrelated_transcripts() {
        assert!(!is_quit_phrase("why is the sky blue", &Locale::English));
        assert!(!is_quit_phrase("hello", &Locale::English));
        // "bye" alone is NOT a quit phrase — only "bye primer".
        assert!(!is_quit_phrase("bye", &Locale::English));
    }

    /// Embedded-phrase guard: a quit phrase embedded inside a longer
    /// utterance must NOT terminate the session. The pre-fix substring
    /// `contains` would have ended the session on either of these —
    /// exactly the opposite of the child's intent. Word-boundary
    /// matching alone wouldn't fix this (end-of-string is itself a
    /// word boundary); equality-after-normalisation does.
    #[test]
    fn embedded_quit_phrase_does_not_end_session() {
        assert!(!is_quit_phrase(
            "I don't want to stop primer",
            &Locale::English
        ));
        assert!(!is_quit_phrase("alright goodbye then", &Locale::English));
        // ... but the phrase as a complete utterance ends the session.
        assert!(is_quit_phrase("stop primer", &Locale::English));
        // Punctuation around the phrase is fine (Whisper often appends).
        assert!(is_quit_phrase("Stop primer!", &Locale::English));
        // Collapsed internal whitespace is fine too.
        assert!(is_quit_phrase("  bye   primer  ", &Locale::English));
    }

    /// German locale ships its own quit phrases. A German-speaking child
    /// who says "tschüss" or "auf wiedersehen" must be able to end the
    /// session by voice — and an English "goodbye" should NOT match in
    /// a German session.
    #[test]
    fn german_locale_uses_german_quit_phrases() {
        assert!(is_quit_phrase("tschüss", &Locale::German));
        assert!(is_quit_phrase("Tschüss!", &Locale::German));
        assert!(is_quit_phrase("auf wiedersehen", &Locale::German));
        assert!(is_quit_phrase("Auf Wiedersehen.", &Locale::German));
        // English-only phrases don't end a German session.
        assert!(!is_quit_phrase("goodbye", &Locale::German));
        // Primer-direct variants are universal (English loanwords).
        assert!(is_quit_phrase("bye primer", &Locale::German));
    }

    /// English locale must NOT match German-only phrases.
    #[test]
    fn english_locale_rejects_german_only_phrases() {
        assert!(!is_quit_phrase("tschüss", &Locale::English));
        assert!(!is_quit_phrase("auf wiedersehen", &Locale::English));
    }
}
