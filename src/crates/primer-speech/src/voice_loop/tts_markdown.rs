//! Pure helpers that strip markdown emphasis before text reaches the
//! TTS engine.
//!
//! Extracted verbatim from `state_machine.rs` so the markdown-stripping
//! logic (and its characterisation tests) lives next to nothing but
//! itself — the state machine simply calls [`strip_markdown_for_tts`] on
//! the commit boundary. Keeping this a leaf of pure functions also keeps
//! `state_machine.rs` under the 500-line-ish guideline.

use crate::phrase_split::PhraseSplitter;

/// Strip markdown emphasis markers so Piper's espeak phonemizer doesn't
/// pronounce them ("*why*" → "asterisks why asterisks"). Paired
/// `*emphasis*` and `**strong**` markers are removed; paired
/// `` `code` `` markers are removed. Bare unmatched `*` or `` ` `` are
/// left in place. A `*` (or run of `*`) sandwiched between digits is
/// treated as multiplication and replaced with " times " so `5*3=15`
/// reads as "5 times 3=15" instead of "53=15". Underscore-emphasis is
/// rare and ambiguous (shows up in identifiers too) — left alone.
///
/// Recursion: the function recurses into the inner content of paired
/// markers (e.g. the `5*3=15` inside `**5*3=15**`). Each recursive call
/// receives a strict substring, so depth is bounded by `input.len()/2`
/// and stack overflow is impossible for any realistic Primer turn.
pub(super) fn strip_markdown_for_tts(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '*' {
            if let Some(end) = consume_digit_times(&chars, i) {
                out.push_str(" times ");
                i = end;
                continue;
            }
            let marker = if i + 1 < chars.len() && chars[i + 1] == '*' {
                2
            } else {
                1
            };
            if let Some(close) = find_paired_marker(&chars, i + marker, marker, '*') {
                let inner: String = chars[i + marker..close].iter().collect();
                out.push_str(&strip_markdown_for_tts(&inner));
                i = close + marker;
                continue;
            }
            out.push('*');
            i += 1;
        } else if c == '`' {
            if let Some(close) = find_paired_marker(&chars, i + 1, 1, '`') {
                let inner: String = chars[i + 1..close].iter().collect();
                out.push_str(&strip_markdown_for_tts(&inner));
                i = close + 1;
                continue;
            }
            out.push('`');
            i += 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// If `chars[i]` is the start of a digit-bounded run of `*` (e.g. `*`
/// or `**` flanked by digits), return the index just past the run.
/// Otherwise return `None`. The caller emits " times " in that case.
///
/// Only ASCII integer boundaries match: `1.5*2`, `1,000*5`, and any
/// non-ASCII numeral won't trigger the rewrite. This is the right
/// trade-off for a children's tutor (integer multiplication dominates),
/// and keeps the heuristic narrow enough that it never fires on prose.
/// Split a reply into the phrases SPEAK pushes, one `push_text` each
/// (#325 stage 2), using the same [`PhraseSplitter`] the TTS backends use.
/// Each phrase is markdown-stripped and ends in a space: the backends'
/// own splitters only accept a terminator followed by whitespace, so
/// without it every phrase would be held until `finalize`. Stripping per
/// phrase means an emphasis pair spanning two sentences stays unmatched
/// and its markers are spoken (owner decision D3).
pub(super) fn tts_phrases(text: &str) -> Vec<String> {
    raw_phrases(text)
        .iter()
        .filter_map(|p| tts_phrase(p))
        .collect()
}

/// `text` split into trimmed phrases, unstripped, tail included.
pub(super) fn raw_phrases(text: &str) -> Vec<String> {
    let mut splitter = PhraseSplitter::new();
    let mut phrases = splitter.push(text);
    phrases.extend(splitter.flush());
    phrases
}

/// One splitter phrase as SPEAK pushes it: markdown-stripped with a
/// trailing space, or `None` when nothing speakable is left.
pub(super) fn tts_phrase(phrase: &str) -> Option<String> {
    let stripped = strip_markdown_for_tts(phrase);
    (!stripped.trim().is_empty()).then(|| stripped + " ")
}

fn consume_digit_times(chars: &[char], i: usize) -> Option<usize> {
    if i == 0 || !chars[i - 1].is_ascii_digit() {
        return None;
    }
    let mut j = i;
    while j < chars.len() && chars[j] == '*' {
        j += 1;
    }
    if j < chars.len() && chars[j].is_ascii_digit() {
        Some(j)
    } else {
        None
    }
}

/// Find the next run of exactly `marker_len` consecutive `marker`
/// characters starting at or after `start`, not adjacent to another
/// `marker` (so a `*` inside a `**` run never matches a single-`*`
/// search and vice versa). Returns the start index of that run.
fn find_paired_marker(
    chars: &[char],
    start: usize,
    marker_len: usize,
    marker: char,
) -> Option<usize> {
    let n = chars.len();
    let mut i = start;
    while i + marker_len <= n {
        let matches = (0..marker_len).all(|k| chars[i + k] == marker);
        let prev_ok = i == 0 || chars[i - 1] != marker;
        let next_ok = i + marker_len >= n || chars[i + marker_len] != marker;
        if matches && prev_ok && next_ok {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{strip_markdown_for_tts, tts_phrases};

    #[test]
    fn tts_phrases_splits_strips_and_appends_a_space() {
        assert_eq!(
            tts_phrases("Hi there. How are *you*? Good!"),
            vec!["Hi there. ", "How are you? ", "Good! "]
        );
    }

    #[test]
    fn tts_phrases_keeps_an_unterminated_tail_and_decimals() {
        assert_eq!(
            tts_phrases("It is 3.5 metres. Dr. Lee says so"),
            vec!["It is 3.5 metres. ", "Dr. Lee says so "]
        );
    }

    #[test]
    fn tts_phrases_drops_empty_input() {
        assert!(tts_phrases("").is_empty());
        assert!(tts_phrases("   ").is_empty());
    }

    /// Owner decision D3 (#325 design note): an emphasis pair that spans a
    /// sentence boundary is no longer matched, so its markers are spoken.
    #[test]
    fn tts_phrases_leaves_a_pair_spanning_sentences_unmatched() {
        assert_eq!(tts_phrases("*Oh. Wow*"), vec!["*Oh. ", "Wow* "]);
    }

    #[test]
    fn strips_paired_emphasis_and_strong() {
        assert_eq!(strip_markdown_for_tts("*why*"), "why");
        assert_eq!(strip_markdown_for_tts("**important**"), "important");
        assert_eq!(
            strip_markdown_for_tts("a *little* bit of **emphasis**"),
            "a little bit of emphasis"
        );
    }

    #[test]
    fn preserves_multiplication_between_digits() {
        assert_eq!(strip_markdown_for_tts("5*3=15"), "5 times 3=15");
        assert_eq!(strip_markdown_for_tts("2 * 3"), "2 * 3");
        assert_eq!(strip_markdown_for_tts("5*3*2"), "5 times 3 times 2");
    }

    #[test]
    fn preserves_exponent_double_star_between_digits() {
        assert_eq!(strip_markdown_for_tts("5**2"), "5 times 2");
    }

    #[test]
    fn leaves_unmatched_star_alone() {
        assert_eq!(strip_markdown_for_tts("a* footnote"), "a* footnote");
        assert_eq!(strip_markdown_for_tts("value *= 5"), "value *= 5");
    }

    #[test]
    fn strips_paired_backticks_only() {
        assert_eq!(strip_markdown_for_tts("`code`"), "code");
        assert_eq!(
            strip_markdown_for_tts("a single ` backtick"),
            "a single ` backtick"
        );
    }

    #[test]
    fn handles_mixed_markdown_and_math() {
        assert_eq!(
            strip_markdown_for_tts("the answer is **5*3=15** indeed"),
            "the answer is 5 times 3=15 indeed"
        );
    }

    #[test]
    fn no_op_on_plain_text() {
        assert_eq!(
            strip_markdown_for_tts("nothing to strip here"),
            "nothing to strip here"
        );
    }

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(strip_markdown_for_tts(""), "");
    }

    /// Triple-`*` runs (bold-italic markdown) are not currently
    /// recognised — the inner closer is rejected by the
    /// "not adjacent to another marker" guard and the outer pair
    /// finds no match. Pinned here so a future refactor doesn't
    /// silently break the current behaviour. If this assertion
    /// ever needs updating, re-derive the right output from first
    /// principles rather than tweaking the test.
    #[test]
    fn triple_star_passes_through_unchanged_for_now() {
        assert_eq!(strip_markdown_for_tts("***foo***"), "***foo***");
    }
}
