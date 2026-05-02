//! Phrase splitter — pure text-segmentation helper for streaming TTS.
//!
//! Walks an accumulated buffer and emits phrases as soon as a terminator
//! (`. ! ?`) is followed by whitespace. Used by `PiperTts` (and the
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

/// ASCII-lowercase abbreviations that should NOT be treated as phrase
/// boundaries. Conservative starting list — extend with evidence.
/// Internal periods (`e.g`, `i.e`, `u.s`) are the trailing-token-only
/// case this list covers. Mid-acronym dots like `U.S.A.` do not split
/// either, but for a different reason: the boundary rule requires
/// whitespace immediately after the terminator, and `U.S.A.` has no
/// whitespace between its interior dots. Acceptable for the children's-
/// conversation register Piper sees today.
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
    /// doesn't yet contain a complete phrase.
    ///
    /// Rules:
    /// 1. `buffer[i]` must be in `PHRASE_TERMINATORS`.
    /// 2. There must exist a char at the position just after any run of
    ///    identical `.` characters starting at `i` (so `...` collapses).
    /// 3. That next char must be whitespace.
    /// 4. If the terminator is a single `.` and the word ending at `i`
    ///    is an abbreviation, no boundary.
    /// 5. Decimal guard is implicit in (3): `3.1` never qualifies because
    ///    `1` isn't whitespace.
    fn find_boundary(&self) -> Option<usize> {
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
