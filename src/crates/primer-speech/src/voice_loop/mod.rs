//! Shared voice loop state machine.
//!
//! State machine: `LISTEN → LATENT_THINK → SPEAK → LISTEN`, with the
//! mic open through LISTEN and LATENT_THINK so the Primer never barges
//! in on a child mid-thought. Closes the mic on the commit boundary
//! (first audio chunk reaches the speaker) so the child never speaks
//! over the Primer.
//!
//! Consumed by `primer-cli` (`--speech` mode) and `primer-gui` (Voice
//! mode toggle) via different [`LoopObserver`] implementations.
//!
//! See `docs/superpowers/specs/2026-05-02-voice-roundtrip-poc-design.md`
//! and `docs/superpowers/specs/2026-05-13-gui-voice-mode-design.md` for
//! the full design.

pub mod observer;
pub mod state_machine;

#[cfg(all(
    feature = "silero",
    feature = "whisper",
    feature = "piper",
    feature = "cpal"
))]
pub mod backends;

// locale_defaults lives at the crate root so its tests run in the default
// (no-feature) workspace build. Re-export here so callers that import via
// `voice_loop::locale_defaults` continue to work unchanged.
/// Re-export the module itself so `voice_loop::locale_defaults::*` paths
/// used in documentation and existing imports resolve.
pub use crate::locale_defaults;
pub use crate::locale_defaults::{LOCALE_DEFAULTS, LocaleDefault, voice_default_for};
pub use observer::{ExitReason, LoopObserver, TurnCompletePayload, VoiceState};
pub use state_machine::{
    DrainHook, LoopBackends, LoopConfig, LoopHandle, Responder, VAD_EVENT_CHANNEL_CAPACITY,
    VoiceLoopError, run_loop, run_loop_borrowed,
};

#[cfg(all(
    feature = "silero",
    feature = "whisper",
    feature = "piper",
    feature = "cpal"
))]
pub use backends::{ChannelStt, LocalBackends, build_local_backends};
