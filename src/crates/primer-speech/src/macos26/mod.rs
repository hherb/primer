//! macOS 26 / iOS 26 SpeechAnalyzer-backed STT + VAD.
//!
//! See [`docs/superpowers/specs/2026-05-20-macos-native-26-design.md`].
//!
//! Cfg gates in this module use `target_vendor = "apple"` rather than
//! `target_os = "macos"` because the underlying APIs are identical
//! across all Apple platforms (iOS 26+, iPadOS 26+, visionOS 26+, tvOS 26+,
//! macOS 26+). Files that genuinely diverge between macOS and iOS
//! concentrate that divergence in [`audio_session`].
//!
//! Module rename to `apple26/` is a mechanical follow-up once an iOS
//! host application actually exists in the repo — see the design doc
//! "Goals" section.

// Sub-modules land in subsequent plan tasks.

pub mod analyzer;
pub mod bridge;
pub mod locale;
pub mod stt;
pub mod vad;

// Re-use the AVSpeechSynthesizer TTS — no new TTS surface in macOS 26.
pub use crate::macos::MacosTextToSpeech;

// Public surface for builders to consume.
pub use crate::macos26::stt::Macos26Stt;
pub use crate::macos26::analyzer::TextMessage;
