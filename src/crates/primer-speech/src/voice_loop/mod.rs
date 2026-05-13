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

pub mod locale_defaults;
pub mod observer;
pub mod state_machine;

#[cfg(all(feature = "silero", feature = "whisper", feature = "piper", feature = "cpal"))]
pub mod backends;

pub use locale_defaults::{voice_default_for, LocaleDefault, LOCALE_DEFAULTS};
pub use observer::{ExitReason, LoopObserver, TurnCompletePayload, VoiceState};
pub use state_machine::{
    run_loop, run_loop_borrowed, DrainHook, LoopBackends, LoopConfig, LoopHandle, Responder,
    VoiceLoopError, VAD_EVENT_CHANNEL_CAPACITY,
};

#[cfg(all(feature = "silero", feature = "whisper", feature = "piper", feature = "cpal"))]
pub use backends::{build_local_backends, ChannelStt, LocalBackends};
