//! # primer-speech
//!
//! Speech backend implementations.
//!
//! For Phase 0, speech I/O can be bypassed entirely — the CLI binary
//! accepts text input and produces text output. Speech integration
//! happens in Phase 1 when hardware audio is connected.

#[cfg(all(feature = "macos-native", feature = "macos-native-26"))]
compile_error!(
    "`macos-native` and `macos-native-26` are mutually exclusive — pick one \
     (`macos-native-26` for macOS 26+, `macos-native` for older macOS)"
);

pub mod locale_defaults;
pub mod phrase_split;
pub mod stub;
pub mod time_ms;
pub mod vad_debounce;

pub use phrase_split::PhraseSplitter;
pub use stub::{StubStt, StubTts};
pub use time_ms::clamp_signed_ms_to_u64;
pub use vad_debounce::{VadDebouncer, ms_to_chunks};

#[cfg(feature = "silero")]
pub mod silero;
#[cfg(feature = "silero")]
pub use silero::{SileroVad, SileroVadParams};

#[cfg(feature = "whisper")]
pub mod whisper;
#[cfg(feature = "whisper")]
pub use whisper::WhisperStt;

#[cfg(feature = "piper")]
pub mod piper;
#[cfg(feature = "piper")]
pub mod piper_config;
#[cfg(feature = "piper")]
pub use piper::PiperTts;

#[cfg(feature = "cpal")]
pub mod cpal_io;
#[cfg(feature = "cpal")]
pub use cpal_io::{MicCapture, Resampler, SpeakerSink, push_all_with_bail, wait_for_drain};

#[cfg(feature = "voice-loop")]
pub mod voice_loop;

#[cfg(all(target_os = "macos", any(feature = "macos-native", feature = "macos-native-26")))]
pub mod macos;

#[cfg(all(target_vendor = "apple", feature = "macos-native-26"))]
pub mod macos26;
