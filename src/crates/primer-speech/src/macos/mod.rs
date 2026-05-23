//! Apple-native speech backends for macOS evaluation distributions.
//!
//! Drops the espeak-ng / whisper / piper / silero stack in favour of
//! SFSpeechRecognizer + AVSpeechSynthesizer. Silero is kept for VAD
//! to preserve the no-barge-in semantics from
//! `[[project_no_barge_in_pedagogy]]` — Apple's SpeechDetector is
//! macOS-26-only and we target macOS 13.
//!
//! Closed under `cfg(target_os = "macos", feature = "macos-native")`.

// `tts`, `voice`, and the `stream_drain` helper `tts` uses reuse cleanly
// across macos-native and macos-native-26 — they only depend on
// AVSpeechSynthesizer / AVSpeechSynthesisVoice via objc2-avf-audio
// (+ objc2 + block2), which both features pull in.
#[cfg(any(feature = "macos-native", feature = "macos-native-26"))]
mod stream_drain;
#[cfg(any(feature = "macos-native", feature = "macos-native-26"))]
pub mod tts;
#[cfg(any(feature = "macos-native", feature = "macos-native-26"))]
pub mod voice;

// The remaining submodules use SFSpeechRecognizer (objc2-speech) and
// are exclusive to the legacy `macos-native` path.
#[cfg(feature = "macos-native")]
pub mod locale;
#[cfg(feature = "macos-native")]
pub mod permissions;
#[cfg(feature = "macos-native")]
pub mod stt;

#[cfg(feature = "macos-native")]
pub use stt::MacosSpeechToText;

#[cfg(any(feature = "macos-native", feature = "macos-native-26"))]
pub use tts::MacosTextToSpeech;

/// Backend family identifier surfaced by `Named::name()` and used in logs.
pub const FEATURE_NAME: &str = "macos-native";
