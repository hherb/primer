//! Apple-native speech backends for macOS evaluation distributions.
//!
//! Drops the espeak-ng / whisper / piper / silero stack in favour of
//! SFSpeechRecognizer + AVSpeechSynthesizer. Silero is kept for VAD
//! to preserve the no-barge-in semantics from
//! `[[project_no_barge_in_pedagogy]]` — Apple's SpeechDetector is
//! macOS-26-only and we target macOS 13.
//!
//! Closed under `cfg(target_os = "macos", feature = "macos-native")`.

pub mod locale;
pub mod permissions;
pub mod runloop;
pub mod stt;
pub mod tts;
pub mod voice;

pub use runloop::run_main_loop_until;
pub use stt::MacosSpeechToText;
pub use tts::MacosTextToSpeech;

/// Backend family identifier surfaced by `Named::name()` and used in logs.
pub const FEATURE_NAME: &str = "macos-native";
