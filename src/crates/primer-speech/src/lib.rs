//! # primer-speech
//!
//! Speech backend implementations.
//!
//! For Phase 0, speech I/O can be bypassed entirely — the CLI binary
//! accepts text input and produces text output. Speech integration
//! happens in Phase 1 when hardware audio is connected.

pub mod stub;
pub mod vad_debounce;

pub use stub::{StubStt, StubTts};
pub use vad_debounce::{VadDebouncer, ms_to_chunks};

#[cfg(feature = "silero")]
pub mod silero;
#[cfg(feature = "silero")]
pub use silero::{SileroVad, SileroVadParams};
