//! Streaming chain-of-thought ("reasoning") suppression.
//!
//! Reasoning-mode models (DeepSeek-R1, QwQ, Qwen3, Gemma4-thinking, …) wrap
//! their internal reasoning in control markers, then emit the visible answer.
//! Those markers arrive token-by-token, so a single block is split across many
//! stream chunks (`<thi` | `nk>Let me` | `</thi` | `nk>answer`). A per-chunk
//! `str::replace` therefore cannot work — this is a stateful filter.
//!
//! # Safety invariant
//! No byte between an open marker and its matching close marker is ever
//! returned to the caller — across split markers, multiple blocks, and an
//! unbalanced block left open at end-of-stream.

use crate::consts;

/// One reasoning-marker pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasoningMarker {
    /// Switches the filter into the suppressing state.
    pub open: String,
    /// Switches the filter back to passthrough.
    pub close: String,
}

impl ReasoningMarker {
    /// Convenience constructor from string-likes.
    pub fn new(open: impl Into<String>, close: impl Into<String>) -> Self {
        Self {
            open: open.into(),
            close: close.into(),
        }
    }
}

/// The built-in default marker set from [`consts::reasoning::DEFAULT_MARKERS`].
pub fn default_markers() -> Vec<ReasoningMarker> {
    consts::reasoning::DEFAULT_MARKERS
        .iter()
        .map(|(o, c)| ReasoningMarker::new(*o, *c))
        .collect()
}

#[derive(Debug)]
enum State {
    Outside,
    Inside { close: String },
}

/// Stateful streaming filter. Feed chunks via [`push`](Self::push); flush at
/// end of stream via [`finish`](Self::finish).
#[derive(Debug)]
pub struct ReasoningFilter {
    markers: Vec<ReasoningMarker>,
    state: State,
    /// Cross-chunk remainder we cannot yet classify (possible split marker).
    buf: String,
    /// Captured reasoning, drained by the caller for logging.
    suppressed: String,
    /// Sticky: true once any reasoning byte has been suppressed this stream.
    did_suppress: bool,
}

impl ReasoningFilter {
    /// Build a filter over a set of marker pairs. Empty ⇒ identity passthrough.
    pub fn new(markers: Vec<ReasoningMarker>) -> Self {
        Self {
            markers,
            state: State::Outside,
            buf: String::new(),
            suppressed: String::new(),
            did_suppress: false,
        }
    }

    /// Feed one streamed chunk; return the visible text to forward (may be empty).
    pub fn push(&mut self, chunk: &str) -> String {
        self.buf.push_str(chunk);
        let mut out = String::new();
        loop {
            match &self.state {
                State::Outside => {
                    // Earliest open marker across all configured pairs.
                    let hit = self
                        .markers
                        .iter()
                        .filter_map(|m| self.buf.find(&m.open).map(|i| (i, m.clone())))
                        .min_by_key(|(i, _)| *i);
                    match hit {
                        Some((i, m)) => {
                            out.push_str(&self.buf[..i]);
                            self.buf.drain(..i + m.open.len());
                            self.state = State::Inside {
                                close: m.close.clone(),
                            };
                            // continue scanning the remainder
                        }
                        None => {
                            // Hold back the longest suffix that could be the
                            // start of some open marker; emit the rest.
                            let hold = longest_open_prefix_suffix(&self.buf, &self.markers);
                            let emit_to = self.buf.len() - hold;
                            out.push_str(&self.buf[..emit_to]);
                            self.buf.drain(..emit_to);
                            break;
                        }
                    }
                }
                State::Inside { close } => {
                    let close = close.clone();
                    match self.buf.find(&close) {
                        Some(i) => {
                            let captured: String = self.buf[..i].to_string();
                            self.capture(&captured);
                            self.buf.drain(..i + close.len());
                            self.state = State::Outside;
                            // continue scanning the remainder
                        }
                        None => {
                            // Suppress everything except the longest suffix that
                            // could be the start of this close marker.
                            let hold = longest_prefix_suffix(&self.buf, &close);
                            let take_to = self.buf.len() - hold;
                            let captured: String = self.buf[..take_to].to_string();
                            self.capture(&captured);
                            self.buf.drain(..take_to);
                            break;
                        }
                    }
                }
            }
        }
        out
    }

    /// Flush at end of stream. Outside ⇒ emit the held buffer (a partial that
    /// never completed a marker is real text). Inside ⇒ DROP the held buffer
    /// (stream ended mid-reasoning) and capture it; emit nothing.
    pub fn finish(&mut self) -> String {
        match &self.state {
            State::Outside => std::mem::take(&mut self.buf),
            State::Inside { .. } => {
                let leftover = std::mem::take(&mut self.buf);
                self.capture(&leftover);
                String::new()
            }
        }
    }

    /// Take the reasoning captured since the last drain (for tracing).
    pub fn drain_suppressed(&mut self) -> String {
        std::mem::take(&mut self.suppressed)
    }

    /// True once any reasoning byte has been suppressed this stream.
    pub fn did_suppress(&self) -> bool {
        self.did_suppress
    }

    /// Append captured reasoning text and set the sticky flag.
    fn capture(&mut self, s: &str) {
        if !s.is_empty() {
            self.suppressed.push_str(s);
            self.did_suppress = true;
        }
    }
}

/// Decide the final (done-chunk) emission for a streaming backend.
///
/// `had_visible` is whether any visible (non-reasoning) text was forwarded
/// BEFORE the done chunk.
/// `tail` = text returned by `filter.finish()` (plus any visible text from the
/// done chunk's own content). Returns `Some(tail)` to emit as the final
/// visible done-chunk text (possibly empty, with `done = true`), or `None` to
/// signal the backend should emit an `InferenceError` "reasoning without
/// answer" variant (added in a later task) instead — i.e. the model
/// reasoned but produced no visible answer.
///
/// A whitespace-only `tail` counts as "no visible answer": many chat
/// templates emit a bare `"\n"` before the opening reasoning marker, and
/// treating that newline as an answer would end an all-reasoning reply
/// as a blank turn instead of the friendly retry error.
pub fn finalize_visible(had_visible: bool, tail: &str, did_suppress: bool) -> Option<String> {
    if !had_visible && tail.trim().is_empty() && did_suppress {
        None
    } else {
        Some(tail.to_string())
    }
}

/// Length of the longest suffix of `buf` that is a proper prefix of `pat`
/// (i.e. `buf` might be the start of `pat`, continued in a later chunk).
/// Returns 0 if no such overlap, capped so a full `pat` match is handled by
/// the caller's `find`, not held back here.
fn longest_prefix_suffix(buf: &str, pat: &str) -> usize {
    let max = buf.len().min(pat.len().saturating_sub(1));
    // Try the longest candidate first.
    for len in (1..=max).rev() {
        let start = buf.len() - len;
        // Respect char boundaries so slicing never panics.
        if !buf.is_char_boundary(start) {
            continue;
        }
        if pat.starts_with(&buf[start..]) {
            return len;
        }
    }
    0
}

/// Longest suffix of `buf` that is a proper prefix of ANY marker's `open`.
fn longest_open_prefix_suffix(buf: &str, markers: &[ReasoningMarker]) -> usize {
    markers
        .iter()
        .map(|m| longest_prefix_suffix(buf, &m.open))
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn think() -> Vec<ReasoningMarker> {
        vec![ReasoningMarker::new("<think>", "</think>")]
    }

    /// Drive a whole stream through the filter and return the concatenated
    /// visible output (push over each chunk, then finish()).
    fn run(markers: Vec<ReasoningMarker>, chunks: &[&str]) -> String {
        let mut f = ReasoningFilter::new(markers);
        let mut out = String::new();
        for c in chunks {
            out.push_str(&f.push(c));
        }
        out.push_str(&f.finish());
        out
    }

    #[test]
    fn no_markers_is_identity() {
        assert_eq!(run(vec![], &["hello ", "world"]), "hello world");
    }

    #[test]
    fn text_without_markers_passes_through() {
        assert_eq!(run(think(), &["just plain text"]), "just plain text");
    }

    #[test]
    fn single_block_in_one_chunk_is_stripped() {
        assert_eq!(
            run(think(), &["before <think>secret</think> after"]),
            "before  after"
        );
    }

    #[test]
    fn open_marker_split_across_chunks() {
        // "<thi" | "nk>secret</think>done"
        assert_eq!(run(think(), &["a<thi", "nk>secret</think>b"]), "ab");
    }

    #[test]
    fn open_marker_split_across_three_chunks() {
        assert_eq!(run(think(), &["a<", "thi", "nk>x</think>b"]), "ab");
    }

    #[test]
    fn close_marker_split_across_chunks() {
        assert_eq!(run(think(), &["a<think>x</thi", "nk>b"]), "ab");
    }

    #[test]
    fn multiple_blocks_in_one_stream() {
        assert_eq!(
            run(think(), &["a<think>1</think>b<think>2</think>c"]),
            "abc"
        );
    }

    #[test]
    fn unbalanced_block_leaks_nothing() {
        // Stream ends inside a reasoning block.
        assert_eq!(run(think(), &["answer<think>still thinking..."]), "answer");
    }

    #[test]
    fn false_prefix_then_real_text_is_emitted() {
        // "<thinking out loud>" is NOT "<think>".
        assert_eq!(
            run(think(), &["I was <thinking out loud> today"]),
            "I was <thinking out loud> today"
        );
    }

    #[test]
    fn custom_marker_appended_pair_is_stripped() {
        let markers = vec![
            ReasoningMarker::new("<think>", "</think>"),
            ReasoningMarker::new("[[r]]", "[[/r]]"),
        ];
        assert_eq!(run(markers, &["a[[r]]hidden[[/r]]b"]), "ab");
    }

    #[test]
    fn gemma4_asymmetric_channel_stripped_answer_survives() {
        let markers = vec![ReasoningMarker::new("<|channel>", "<channel|>")];
        assert_eq!(
            run(
                markers,
                &["<|channel>thought\nreasoning<channel|>The answer."]
            ),
            "The answer."
        );
    }

    #[test]
    fn drain_suppressed_returns_captured_then_empties() {
        let mut f = ReasoningFilter::new(think());
        let _ = f.push("a<think>secret</think>b");
        let drained = f.drain_suppressed();
        assert!(drained.contains("secret"));
        assert_eq!(f.drain_suppressed(), "");
    }

    #[test]
    fn did_suppress_false_on_clean_passthrough_true_after_block() {
        let mut f = ReasoningFilter::new(think());
        let _ = f.push("plain");
        assert!(!f.did_suppress());
        let _ = f.push("<think>x</think>");
        assert!(f.did_suppress());
    }

    #[test]
    fn finalize_visible_emits_error_only_when_nothing_visible_and_suppressed() {
        assert_eq!(finalize_visible(false, "", true), None);
        assert_eq!(finalize_visible(true, "", true), Some(String::new()));
        assert_eq!(
            finalize_visible(false, "answer", true),
            Some("answer".to_string())
        );
        // No suppression: an empty model response is a different failure.
        assert_eq!(finalize_visible(false, "", false), Some(String::new()));
    }

    #[test]
    fn unicode_content_around_marker_passes_through() {
        // 'ö' is 2 bytes in UTF-8; ensure slicing near it doesn't panic.
        assert_eq!(
            run(think(), &["Schön <think>geheim</think> gut"]),
            "Schön  gut"
        );
    }

    #[test]
    fn split_chunks_with_multi_byte_char_are_safe() {
        // 'ä' = 2 bytes; a chunk boundary adjacent to it must not panic and
        // the marker that follows must still be stripped.
        assert_eq!(
            run(think(), &["a\u{00e4}", "b<think>x</think>c"]),
            "a\u{00e4}bc"
        );
    }
}
