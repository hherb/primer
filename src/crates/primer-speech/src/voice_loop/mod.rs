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

// The re-export gate matches `backends`'s module-level cfg (silero +
// whisper + piper + cpal) because that's where the function lives;
// the function itself only requires silero + cpal + macos-native.
// Once a follow-up PR extracts the macos-native builder into its own
// module gated on the narrower set, this re-export can drop the
// whisper/piper requirements. Tracking: plan task 8 review.
#[cfg(all(
    target_os = "macos",
    feature = "macos-native",
    feature = "silero",
    feature = "whisper",
    feature = "piper",
    feature = "cpal"
))]
pub use backends::build_local_backends_macos_native;
