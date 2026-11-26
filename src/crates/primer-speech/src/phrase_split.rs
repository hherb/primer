//! Phrase splitter — pure text-segmentation helper for streaming TTS.
//!
//! Walks an accumulated buffer and emits phrases as soon as a terminator
//! (`. ! ?`) is followed by whitespace, or at a blank line. Used by `PiperTts` (and the
//! streaming-TTS stub) because piper-rs's synthesis call is one-shot and
//! synchronous — there is no native phrase-boundary callback to hook,
//! so we synthesise streaming at the text layer instead. Kept separate
//! from any specific backend so the boundary state machine can be
//! unit-tested without pulling in piper-rs / ONNX Runtime.
//!
//! The rule set mirrors what a streaming speech synthesiser actually
//! needs: split where a human reader would pause for breath, and
//! suppress false splits that destroy delivery (decimals, abbreviations,
//! ellipses).

const PHRASE_TERMINATORS: &[char] = &['.', '!', '?'];

/// A blank line: a hard phrase boundary even without a terminator (#430:
/// the DM's truncation notice follows a cut-off partial after one). A
/// caller that must have a pushed phrase spoken at once, whatever its
/// ending, appends this.
pub const PHRASE_BREAK: &str = "\n\n";

/// ASCII-lowercase abbreviations that should NOT be treated as phrase
/// boundaries. Conservative starting list — extend with evidence.
/// Used when a single `.` is followed by whitespace (e.g. `Dr. Smith`).
///
/// Acronym handling deserves a separate mention: the *interior* dots of
/// a multi-letter acronym like `U.S.A.` are never boundary candidates,
/// because the boundary rule requires whitespace immediately after the
/// terminator and there's no whitespace between interior dots. The
/// *trailing* `.` (followed by whitespace) DOES split — yielding
/// `"U.S.A."` as one phrase, exactly what we want when the acronym ends
/// a sentence. Sentence-medial cases (`I live in U.S.A. today.`) split
/// after `U.S.A.`; that's a known imperfection, acceptable for the
/// children's-conversation register Piper sees today.
const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "prof", "sr", "jr", "st", "vs", "etc", "ie", "eg", "us", "uk",
];

/// Streaming phrase splitter.
///
/// Append text via [`Self::push`]; receive any phrases that became
/// complete as a result. Call [`Self::flush`] when the upstream stream
/// has closed to drain whatever remains regardless of terminator.
#[derive(Debug, Default)]
pub struct PhraseSplitter {
    buffer: String,
}

impl PhraseSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append `text` and return any phrases that became complete as a
    /// result. Each returned phrase is trimmed.
    pub fn push(&mut self, text: &str) -> Vec<String> {
        self.buffer.push_str(text);
        self.drain_completed()
    }

    /// Drain whatever remains in the buffer, regardless of terminator.
    /// Returns `None` if the buffer is empty or whitespace-only.
    pub fn flush(&mut self) -> Option<String> {
        let trimmed = self.buffer.trim().to_string();
        self.buffer.clear();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }

    /// Whether the buffer ends on a phrase boundary that only lacks the
    /// whitespace [`Self::push`] waits for (`"Why?"`, not `"Dr."` or
    /// `"How"`). A streaming caller can [`Self::flush`] such a tail after
    /// a pause instead of holding it until the stream closes.
    pub fn ends_at_boundary(&self) -> bool {
        if self.buffer.trim().is_empty() {
            return false;
        }
        let probe = Self {
            buffer: format!("{} ", self.buffer),
        };
        probe.find_boundary().is_some()
    }

    fn drain_completed(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(end) = self.find_boundary() {
            let phrase: String = self.buffer[..end].trim().to_string();
            self.buffer.drain(..end);
            if !phrase.is_empty() {
                out.push(phrase);
            }
        }
        out
    }

    /// Find the byte index *after* the next phrase boundary (and any
    /// trailing whitespace consumed with it), or `None` if the buffer
    /// doesn't yet contain a complete phrase: the earlier of a
    /// terminator boundary and a blank-line one.
    fn find_boundary(&self) -> Option<usize> {
        match (self.terminator_boundary(), self.blank_line_boundary()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// End of the first whitespace run holding two `\n`, if any.
    fn blank_line_boundary(&self) -> Option<usize> {
        let mut newlines = 0;
        for (i, c) in self.buffer.char_indices() {
            if !c.is_whitespace() {
                newlines = 0;
            } else if c == '\n' {
                newlines += 1;
                if newlines == 2 {
                    let run: usize = self.buffer[i..]
                        .chars()
                        .take_while(|c| c.is_whitespace())
                        .map(char::len_utf8)
                        .sum();
                    return Some(i + run);
                }
            }
        }
        None
    }

    /// Terminator rules:
    /// 1. `buffer[i]` must be in `PHRASE_TERMINATORS`.
    /// 2. There must exist a char at the position just after any run of
    ///    identical `.` characters starting at `i` (so `...` collapses).
    /// 3. That next char must be whitespace.
    /// 4. If the terminator is a single `.` and the word ending at `i`
    ///    is an abbreviation, no boundary.
    /// 5. Decimal guard is implicit in (3): `3.1` never qualifies because
    ///    `1` isn't whitespace.
    fn terminator_boundary(&self) -> Option<usize> {
        let mut iter = self.buffer.char_indices().peekable();
        while let Some((i, ch)) = iter.next() {
            if !PHRASE_TERMINATORS.contains(&ch) {
                continue;
            }

            // Collapse a run of `.` (handles `...` and longer).
            let mut term_end = i + ch.len_utf8();
            if ch == '.' {
                while let Some(&(_, next_ch)) = iter.peek() {
                    if next_ch == '.' {
                        term_end += '.'.len_utf8();
                        iter.next();
                    } else {
                        break;
                    }
                }
            }

            // Need a char after the terminator run.
            let next_ch = self.buffer[term_end..].chars().next()?;

            if !next_ch.is_whitespace() {
                continue;
            }

            // Abbreviation guard: only relevant for a single-dot terminator.
            let is_single_dot = ch == '.' && term_end == i + 1;
            if is_single_dot && self.is_abbreviation_before(i) {
                continue;
            }

            // Consume trailing whitespace so the next phrase doesn't start
            // with leading space.
            let mut after = term_end;
            for (j, c) in self.buffer[term_end..].char_indices() {
                if c.is_whitespace() {
                    after = term_end + j + c.len_utf8();
                } else {
                    break;
                }
            }
            return Some(after);
        }
        None
    }

    fn is_abbreviation_before(&self, dot_index: usize) -> bool {
        let prefix = &self.buffer[..dot_index];
        let word: String = prefix
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>()
            .to_ascii_lowercase();
        if word.is_empty() {
            return false;
        }
        ABBREVIATIONS.contains(&word.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_nothing() {
        let mut s = PhraseSplitter::new();
        assert!(s.push("").is_empty());
        assert_eq!(s.flush(), None);
    }

    #[test]
    fn simple_two_sentences_emit_two_phrases() {
        let mut s = PhraseSplitter::new();
        let phrases = s.push("Hello. World. ");
        assert_eq!(phrases, vec!["Hello.", "World."]);
        // Buffer should now be empty (trailing whitespace consumed).
        assert_eq!(s.flush(), None);
    }

    #[test]
    fn decimal_does_not_split() {
        let mut s = PhraseSplitter::new();
        // Trailing space confirms the period after "today" is a real boundary.
        let phrases = s.push("It is 3.14 today. ");
        assert_eq!(phrases, vec!["It is 3.14 today."]);
    }

    #[test]
    fn abbreviation_does_not_split() {
        let mut s = PhraseSplitter::new();
        let phrases = s.push("Dr. Smith arrived. ");
        assert_eq!(phrases, vec!["Dr. Smith arrived."]);
    }

    #[test]
    fn multiple_terminators_collapse() {
        let mut s = PhraseSplitter::new();
        let phrases = s.push("Wait... what? ");
        assert_eq!(phrases, vec!["Wait...", "what?"]);
    }

    #[test]
    fn mid_token_push_does_not_eagerly_split() {
        let mut s = PhraseSplitter::new();
        // First push has a terminator but no following whitespace yet — no boundary.
        let p0 = s.push("Hello.");
        assert!(p0.is_empty());
        // Next push starts with whitespace, completing the boundary, and contains its own.
        let p1 = s.push(" World. ");
        assert_eq!(p1, vec!["Hello.", "World."]);
    }

    #[test]
    fn ends_at_boundary_only_for_a_complete_unspaced_sentence() {
        let at = |text: &str| {
            let mut s = PhraseSplitter::new();
            assert!(s.push(text).is_empty(), "{text:?} already split");
            s.ends_at_boundary()
        };
        assert!(at("What makes it hot?"));
        assert!(at("Wow!"));
        assert!(at("Wait..."));
        assert!(!at("Ask Dr."), "abbreviation");
        assert!(!at("It is 3"), "no terminator");
        assert!(!at("How"));
        assert!(!at("   "));
        assert!(!PhraseSplitter::new().ends_at_boundary());
    }

    #[test]
    fn flush_drains_pending_text_without_terminator() {
        let mut s = PhraseSplitter::new();
        assert!(s.push("Hello").is_empty());
        assert_eq!(s.flush(), Some("Hello".to_string()));
    }

    #[test]
    fn flush_returns_none_on_empty_or_whitespace() {
        let mut s = PhraseSplitter::new();
        assert!(s.push("   \n\t").is_empty());
        assert_eq!(s.flush(), None);
    }

    #[test]
    fn non_ascii_in_phrase_does_not_panic() {
        let mut s = PhraseSplitter::new();
        let phrases = s.push("Bonjour Élise. ");
        assert_eq!(phrases, vec!["Bonjour Élise."]);
    }

    #[test]
    fn exclamation_and_question_split() {
        let mut s = PhraseSplitter::new();
        let phrases = s.push("Wow! Really? ");
        assert_eq!(phrases, vec!["Wow!", "Really?"]);
    }

    #[test]
    fn blank_line_is_a_hard_boundary_without_a_terminator() {
        // #430: a truncated partial, the notice separator, then the apology.
        let mut s = PhraseSplitter::new();
        let phrases = s.push("the sun is a star that\n\nSorry, I ran out of room. ");
        assert_eq!(
            phrases,
            vec!["the sun is a star that", "Sorry, I ran out of room."]
        );
    }

    #[test]
    fn blank_line_split_across_pushes_and_with_inner_spaces() {
        let mut s = PhraseSplitter::new();
        assert!(s.push("a star that\n").is_empty());
        assert_eq!(s.push(" \t\nSorry"), vec!["a star that"]);
        assert_eq!(s.flush(), Some("Sorry".to_string()));
        // A single newline is not a boundary.
        assert!(s.push("one\ntwo").is_empty());
    }

    #[test]
    fn phrase_break_completes_any_phrase() {
        let mut s = PhraseSplitter::new();
        let p = format!("no terminator here{PHRASE_BREAK}");
        assert_eq!(s.push(&p), vec!["no terminator here"]);
        assert_eq!(s.push(&format!("Dr.{PHRASE_BREAK}")), vec!["Dr."]);
    }

    #[test]
    fn multi_byte_whitespace_after_terminator_splits_correctly() {
        // U+3000 IDEOGRAPHIC SPACE is 3 bytes in UTF-8; the trailing-
        // whitespace consumer must advance by len_utf8(), not by 1, or
        // the next phrase will start with stray bytes (or panic at a
        // codepoint boundary).
        let mut s = PhraseSplitter::new();
        let phrases = s.push("Hello.\u{3000}World. ");
        assert_eq!(phrases, vec!["Hello.", "World."]);
    }
}
