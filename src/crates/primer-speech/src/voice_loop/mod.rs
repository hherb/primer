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
/// Runtime STT/TTS backend selector enums (pure data; never feature-gated).
/// Single source of truth for the decoupled voice-loop wiring — the CLI uses
/// them directly and the GUI mirrors them in its own config layer.
pub mod selectors;
pub mod state_machine;

/// Shared [`LocalBackends`] / [`ChannelStt`] types — gated only on `cpal`
/// because every concrete backend builder (whisper+piper or macOS-native)
/// needs the cpal-owned `MicCapture`/`SpeakerSink` but nothing else.
#[cfg(feature = "cpal")]
pub mod backends_common;

/// Whisper + piper backend builder — gated on the full
/// `silero + whisper + piper + cpal` set because the function body uses
/// all four. Shares `LocalBackends` / `ChannelStt` with the macOS
/// builders via [`backends_common`].
#[cfg(all(
    feature = "silero",
    feature = "whisper",
    feature = "piper",
    feature = "cpal"
))]
pub mod backends;

/// macOS-native backend builder (SFSpeechRecognizer + Silero VAD). One
/// file per builder so each `pub` item lives behind exactly the narrow
/// feature gate that reflects its body, and each file stays well under
/// the 500-line guideline. Closes #149.
#[cfg(all(target_os = "macos", feature = "cpal", feature = "macos-native"))]
pub mod backends_macos_native;

/// macOS 26 native backend builder (SpeechAnalyzer + derived VAD).
/// Sibling of [`backends_macos_native`]; both share the mic/speaker/
/// closure helpers in [`backends_common`].
#[cfg(all(target_os = "macos", feature = "cpal", feature = "macos-native-26"))]
pub mod backends_macos_native_26;

/// Pure helper for the macos-native-26 audio thread's pre-resample
/// chunk buffer; clears on `is_speaking` to prevent pre-speak audio
/// leaking into the post-speak transcription (closes #139).
#[cfg(all(target_os = "macos", feature = "cpal", feature = "macos-native-26"))]
pub(crate) mod macos26_audio_buffer;

pub use observer::{ExitReason, LoopObserver, TurnCompletePayload, VoiceState};
pub use selectors::{SttBackend, TtsBackend, TtsAssets, build_tts};
pub use state_machine::{
    DrainHook, LoopBackends, LoopConfig, LoopHandle, Responder, VAD_EVENT_CHANNEL_CAPACITY,
    VoiceLoopError, run_loop, run_loop_borrowed,
};

#[cfg(feature = "cpal")]
pub use backends_common::{ChannelStt, LocalBackends};

#[cfg(all(
    feature = "silero",
    feature = "whisper",
    feature = "piper",
    feature = "cpal"
))]
pub use backends::build_local_backends;

#[cfg(all(
    target_os = "macos",
    feature = "cpal",
    feature = "silero",
    feature = "macos-native"
))]
pub use backends_macos_native::build_local_backends_macos_native;

#[cfg(all(target_os = "macos", feature = "cpal", feature = "macos-native-26"))]
pub use backends_macos_native_26::build_local_backends_macos_native_26;
