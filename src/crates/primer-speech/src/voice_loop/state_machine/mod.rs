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

/// Spoken when the LLM call fails (rate limit, network, etc.). Goes
/// through Piper just like any normal Primer turn — the child hears
/// the apology, then we loop back to LISTEN.
///
/// Lives here (not in `inner.rs` beside its consumer `handle_llm_err`)
/// because `mocks.rs`, a child of this module, asserts against it via
/// `super::FALLBACK_LINE`.
const FALLBACK_LINE: &str = "Sorry, I had trouble with that. Could you ask again?";

// NOTE: The DialogueResponder adapter, ChannelStt adapter,
// `pub async fn run` entry point, and `run_audio_thread` body all stay
// in `primer-cli/src/speech_loop.rs` — they are CLI-specific glue around
// the state machine. This module owns only the state machine itself
// (`inner.rs`), its entry points (`entry.rs`), and the public
// types/traits the CLI and GUI both consume (`types.rs`); `mod.rs` is
// the re-export façade plus `FALLBACK_LINE`.

#[cfg(test)]
mod mocks;
