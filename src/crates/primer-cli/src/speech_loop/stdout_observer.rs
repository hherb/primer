//! CLI-side [`LoopObserver`] that preserves the existing stdout/stderr
//! print formatting of the pre-refactor speech_loop.rs.
//!
//! Output contract (do not change without bumping the user-visible
//! verbose docs):
//!   stdout: `[child] <transcript>` and `[primer] <reply>` lines
//!   stderr (only if verbose): `[state] <from> -> <to>` lines
//!
//! Note: the pre-refactor CLI printed `[primer] {accumulated}` as a
//! single line AFTER the LLM call completed (it never streamed chunks
//! to stdout in real time). We preserve that: chunks are accumulated
//! across `on_response_chunk` calls and flushed on `on_response_complete`.
//! If a future PR wants real-time CLI streaming, switch to per-chunk
//! print here.

use primer_speech::voice_loop::{ExitReason, LoopObserver, TurnCompletePayload, VoiceState};

pub struct StdoutObserver {
    verbose: bool,
    last_state: Option<VoiceState>,
    /// Accumulated text for the current Primer turn. Flushed as one
    /// line on `on_response_complete`.
    primer_buffer: String,
}

impl StdoutObserver {
    pub fn new(verbose: bool) -> Self {
        Self {
            verbose,
            last_state: None,
            primer_buffer: String::new(),
        }
    }
}

impl LoopObserver for StdoutObserver {
    fn on_state_change(&mut self, state: VoiceState, _hint: Option<&str>) {
        if self.verbose {
            if let Some(prev) = self.last_state {
                eprintln!("[state] {} -> {}", prev.name(), state.name());
            } else {
                eprintln!("[state] -> {}", state.name());
            }
        }
        self.last_state = Some(state);
    }

    fn on_transcript_finalized(&mut self, text: &str) {
        println!("[child] {text}");
    }

    fn on_response_chunk(&mut self, _primer_turn_index: usize, chunk: &str) {
        self.primer_buffer.push_str(chunk);
    }

    fn on_response_complete(&mut self, _payload: TurnCompletePayload) {
        if !self.primer_buffer.is_empty() {
            println!("[primer] {}", self.primer_buffer);
            self.primer_buffer.clear();
        }
    }

    fn on_inference_error(&mut self, err: &primer_core::error::InferenceError) {
        // Drop any partial buffer; the loop will synthesise FALLBACK_LINE
        // through TTS and an on_response_complete will follow shortly.
        self.primer_buffer.clear();
        eprintln!("[primer] (inference error: {err:?})");
    }

    fn on_exit(&mut self, reason: ExitReason) {
        if self.verbose {
            eprintln!("[state] exiting ({})", reason.name());
        }
    }
}
