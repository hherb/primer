//! Shared streaming reasoning-filter step for byte-stream inference backends.
//!
//! `OllamaBackend` (NDJSON) and `OpenAiCompatBackend` (SSE) parse their wire
//! formats differently, but once a chunk of model text is parsed the
//! reasoning-marker handling is identical: run the text through a
//! [`ReasoningFilter`], forward only visible output, and at end-of-stream
//! decide (via [`finalize_visible`]) whether a visible answer was produced or
//! the model only emitted suppressed reasoning. This module is that shared
//! step, so the two backends share one verified implementation.

use primer_core::error::{InferenceError, PrimerError, Result};
use primer_core::inference::TokenChunk;
use primer_core::reasoning::{ReasoningFilter, finalize_visible};

/// What the caller should do with the result of filtering one parsed chunk.
pub(crate) enum FilterAction {
    /// Nothing to forward (filtered text was empty on a non-final chunk).
    Nothing,
    /// Forward this result; if the send fails (consumer dropped) the caller
    /// should stop the stream, but this is not itself a terminal chunk.
    Forward(Result<TokenChunk>),
    /// Terminal: forward this result, then stop the stream.
    Final(Result<TokenChunk>),
}

/// Run one parsed chunk through the reasoning filter and decide what to emit.
///
/// `had_visible` is set once any visible (non-reasoning) text has been
/// forwarded this stream; it is consumed only as an "is anything visible?"
/// signal by [`finalize_visible`]. `backend` labels the debug log of any
/// suppressed reasoning.
///
/// On the final (`chunk.done`) chunk: the chunk's own content is filtered,
/// the filter is flushed via `finish()`, and `finalize_visible` decides
/// between a final visible [`TokenChunk`] and an
/// [`InferenceError::ReasoningWithoutAnswer`] (model reasoned but produced no
/// visible answer). On a non-final chunk: visible text is forwarded only when
/// non-empty.
pub(crate) fn process_filtered_chunk(
    filter: &mut ReasoningFilter,
    chunk: TokenChunk,
    had_visible: &mut bool,
    backend: &'static str,
) -> FilterAction {
    if chunk.done {
        let mut visible = filter.push(&chunk.text);
        *had_visible |= !visible.is_empty();
        visible.push_str(&filter.finish());
        log_suppressed(filter, backend);
        match finalize_visible(*had_visible, &visible, filter.did_suppress()) {
            Some(text) => FilterAction::Final(Ok(TokenChunk {
                text,
                done: true,
                ..Default::default()
            })),
            None => FilterAction::Final(Err(PrimerError::Inference(
                InferenceError::ReasoningWithoutAnswer,
            ))),
        }
    } else {
        let visible = filter.push(&chunk.text);
        log_suppressed(filter, backend);
        if visible.is_empty() {
            FilterAction::Nothing
        } else {
            *had_visible = true;
            FilterAction::Forward(Ok(TokenChunk {
                text: visible,
                done: false,
                ..Default::default()
            }))
        }
    }
}

/// Drain any captured reasoning from the filter and emit it at debug level.
fn log_suppressed(filter: &mut ReasoningFilter, backend: &'static str) {
    let r = filter.drain_suppressed();
    if !r.is_empty() {
        tracing::debug!(target: "primer::reasoning", backend, suppressed = %r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primer_core::reasoning::{ReasoningMarker, default_markers};

    fn chunk(text: &str, done: bool) -> TokenChunk {
        TokenChunk {
            text: text.into(),
            done,
            ..Default::default()
        }
    }

    /// Drive a sequence of (text, done) parsed chunks through the REAL
    /// `process_filtered_chunk` and collect (visible, emitted_error).
    fn drive(markers: Vec<ReasoningMarker>, chunks: &[(&str, bool)]) -> (String, bool) {
        let mut filter = ReasoningFilter::new(markers);
        let mut had_visible = false;
        let mut visible = String::new();
        let mut err = false;
        for (text, done) in chunks {
            match process_filtered_chunk(&mut filter, chunk(text, *done), &mut had_visible, "test")
            {
                FilterAction::Nothing => {}
                FilterAction::Forward(r) => visible.push_str(&r.unwrap().text),
                FilterAction::Final(r) => {
                    match r {
                        Ok(c) => visible.push_str(&c.text),
                        Err(_) => err = true,
                    }
                    break;
                }
            }
        }
        (visible, err)
    }

    #[test]
    fn strips_think_block_with_default_markers() {
        let (visible, err) = drive(
            default_markers(),
            &[
                ("<think>plan</think>", false),
                ("Hi there", false),
                ("", true),
            ],
        );
        assert_eq!(visible, "Hi there");
        assert!(!err);
    }

    #[test]
    fn only_reasoning_yields_error() {
        let (visible, err) = drive(
            default_markers(),
            &[("<think>only thinking", false), ("", true)],
        );
        assert_eq!(visible, "");
        assert!(err);
    }

    #[test]
    fn empty_model_response_is_not_an_error() {
        // No suppression at all + no visible output ⇒ NOT ReasoningWithoutAnswer.
        let (visible, err) = drive(default_markers(), &[("", true)]);
        assert_eq!(visible, "");
        assert!(!err);
    }

    #[test]
    fn non_final_empty_filtered_chunk_forwards_nothing() {
        let mut filter = ReasoningFilter::new(default_markers());
        let mut had_visible = false;
        // A chunk that is entirely an (incomplete) marker prefix yields no
        // visible text and must not be forwarded.
        let action = process_filtered_chunk(
            &mut filter,
            chunk("<think>", false),
            &mut had_visible,
            "test",
        );
        assert!(matches!(action, FilterAction::Nothing));
        assert!(!had_visible);
    }

    #[test]
    fn custom_marker_appended() {
        let mut markers = default_markers();
        markers.push(ReasoningMarker::new("[[r]]", "[[/r]]"));
        let (visible, err) = drive(markers, &[("a[[r]]hidden[[/r]]b", false), ("", true)]);
        assert_eq!(visible, "ab");
        assert!(!err);
    }

    #[test]
    fn done_chunk_with_own_content_is_filtered_and_flushed() {
        // Final chunk carries content that completes a block then has answer.
        let (visible, err) = drive(default_markers(), &[("<think>x</think>answer", true)]);
        assert_eq!(visible, "answer");
        assert!(!err);
    }

    #[test]
    fn abrupt_eof_flush_emits_held_back_visible_tail() {
        // Models stream visible text whose trailing bytes look like the start of
        // an open marker ("<thi" ⊂ "<think>"), so the filter holds them back. If
        // the stream then ends WITHOUT a `done` chunk (abrupt EOF), the backend's
        // `None` arm feeds a synthetic `("", true)` through this helper to flush.
        // That held-back tail must be emitted as real text, not dropped.
        let (visible, err) = drive(default_markers(), &[("answer<thi", false), ("", true)]);
        assert_eq!(visible, "answer<thi");
        assert!(!err);
    }

    #[test]
    fn abrupt_eof_flush_surfaces_error_when_only_reasoning_seen() {
        // Mid-reasoning abrupt EOF: the synthetic flush chunk drops the
        // unterminated block (no leak) and surfaces ReasoningWithoutAnswer
        // rather than ending the turn silently.
        let (visible, err) = drive(
            default_markers(),
            &[("<think>still thinking", false), ("", true)],
        );
        assert_eq!(visible, "");
        assert!(err);
    }
}
