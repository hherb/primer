//! macOS 26 SpeechAnalyzer-backed STT + VAD.
//!
//! See [`docs/superpowers/specs/2026-05-20-macos-native-26-design.md`].
//!
//! **Today this module is macOS-only.** The parent cfg gate in
//! [`crate::lib`] is `target_os = "macos"`, and the TTS re-export below
//! depends on [`crate::macos`] which is itself macOS-gated. The
//! [`build.rs`] also hardcodes the `apple-macos26.0` Swift target triple.
//!
//! Internal files use `target_vendor = "apple"` cfg gates as structural
//! preparation for an eventual iOS-26 host: the underlying APIs
//! (`SpeechAnalyzer`, `SpeechTranscriber`, `SpeechDetector`,
//! `AVSpeechSynthesizer`) are identical across all Apple platforms
//! (iOS 26+, iPadOS 26+, visionOS 26+, tvOS 26+, macOS 26+). Files that
//! genuinely diverge between macOS and iOS concentrate that divergence
//! in [`audio_session`] (its iOS branch errors loudly so a developer
//! can't ship an unconfigured build).
//!
//! Lifting the parent gate to `target_vendor = "apple"` is a follow-up
//! that requires: (a) parameterising the build.rs target triple by
//! `CARGO_CFG_TARGET_OS`, (b) implementing the iOS AVAudioSession setup
//! in [`audio_session`], (c) extending [`crate::macos`]'s parent gate to
//! `target_vendor = "apple"` so the AVSpeechSynthesizer TTS re-export
//! resolves on iOS. Module rename to `apple26/` is a mechanical
//! follow-up at that point.

pub mod analyzer;
pub mod audio_session;
pub mod bridge;
pub mod locale;
pub mod stt;
pub mod vad;

// Re-use the AVSpeechSynthesizer TTS — no new TTS surface in macOS 26.
pub use crate::macos::MacosTextToSpeech;

// Public surface for builders to consume.
pub use crate::macos26::analyzer::TextMessage;
pub use crate::macos26::stt::Macos26Stt;
