//! State machine implementation (LISTEN → LATENT_THINK → SPEAK → LISTEN).
//!
//! Lifted from `primer-cli/src/speech_loop.rs` in PR 1 of the GUI
//! voice-mode work. Side-effects now route through [`super::LoopObserver`]
//! instead of inline `println!`s; the CLI's stdout output is preserved
//! by the `StdoutObserver` adapter in `primer-cli`.

mod entry;
mod inner;
mod types;

pub use entry::{run_loop, run_loop_borrowed};
pub use types::{
    DrainHook, LoopBackends, LoopConfig, LoopHandle, Responder, VAD_EVENT_CHANNEL_CAPACITY,
    VoiceLoopError,
};

use super::observer::LoopObserver;
// `mocks.rs` (the test module, a child of this module) reaches these
// observer types via `super::ExitReason` etc.; production code imports
// them directly in `inner.rs`, so the aliases are test-only here.
#[cfg(test)]
use super::observer::{ExitReason, TurnCompletePayload, VoiceState};

/// Spoken when the LLM call fails (rate limit, network, etc.). Goes
/// through Piper just like any normal Primer turn — the child hears
/// the apology, then we loop back to LISTEN.
const FALLBACK_LINE: &str = "Sorry, I had trouble with that. Could you ask again?";

/// Handle an LLM error inside the LATENT_THINK select arms.
///
/// Surfaces the typed error to the observer, then **drops** any chunks
/// the partial attempt managed to push into `chunk_buffer` and replaces
/// them with a single synthetic FALLBACK_LINE chunk. The replay loop
/// downstream will deliver that one chunk to the observer, so the GUI
/// chat bubble shows exactly the text TTS will speak — no truncated
/// pre-error stream stuck on screen.
///
/// Preserves the typed `InferenceError` variant when the responder
/// returned a `PrimerError::Inference(_)` so the i18n layer can render
/// the variant-specific user-facing copy (`Auth` / `RateLimited` /
/// `ServiceUnavailable` / `NetworkUnavailable` / `ModelNotFound`). Only
/// non-Inference `PrimerError` variants fall back to `Other` (carrying
/// the dev-facing display string, which the i18n layer redacts before
/// it ever reaches the user). CLAUDE.md's i18n contract forbids
/// reintroducing a `to_string().into()` wrap on the inference path —
/// it would flatten every typed variant to `Other`.
///
/// Returns the text the caller should set `accumulated` to (which then
/// flows into TTS synthesis).
fn handle_llm_err<O: LoopObserver>(
    err: primer_core::error::PrimerError,
    chunk_buffer: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    observer: &mut O,
) -> String {
    let inference_err = match err {
        primer_core::error::PrimerError::Inference(inf) => inf,
        other => other.to_string().into(),
    };
    observer.on_inference_error(&inference_err);
    let mut chunks = chunk_buffer.lock().unwrap();
    chunks.clear();
    chunks.push(FALLBACK_LINE.to_string());
    FALLBACK_LINE.to_string()
}

// NOTE: The DialogueResponder adapter, ChannelStt adapter,
// `pub async fn run` entry point, and `run_audio_thread` body all stay
// in `primer-cli/src/speech_loop.rs` — they are CLI-specific glue around
// the state machine. This file owns only the state machine itself and
// the public types/traits the CLI and GUI both consume.

#[cfg(test)]
mod mocks;
