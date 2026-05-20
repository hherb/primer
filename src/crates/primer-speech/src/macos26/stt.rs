//! `Macos26Stt` implements `StreamingSpeechToText`. Each session owns one
//! `Macos26Pipeline` (a fresh SpeechAnalyzer instance). Construction is
//! async because Swift's `Macos26Pipeline.create(locale:)` is async — it
//! awaits the asset-install step.
//!
//! NOTE: this is a thinner shim than the Whisper one because all the
//! heavy lifting happens in
//! `voice_loop::backends::build_local_backends_macos_native_26`; the
//! trait fit is here for symmetry with the rest of the codebase. The
//! session's `push_audio` and `finalize` return errors because audio
//! flows through the analyzer task in the builder, not through the
//! trait surface.

#![cfg(all(target_vendor = "apple", feature = "macos-native-26"))]

use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;
use primer_core::speech::{
    Named, StreamingSpeechToText, TranscriptSegment, TranscriptionSession,
};

use crate::macos26::locale::to_bcp47;

const BACKEND_NAME: &str = "macos-26-speech-analyzer";

/// Streaming STT backend backed by macOS 26's `SpeechAnalyzer`.
/// Locale is fixed at construction time. The actual SpeechAnalyzer
/// instance is owned by the audio task in `build_local_backends_macos_native_26`,
/// not by this struct or its sessions.
pub struct Macos26Stt {
    locale: Locale,
}

impl Macos26Stt {
    /// Construct. Validates the locale eagerly via `to_bcp47` so an
    /// unsupported locale (e.g. Hindi today) is a startup-time error.
    pub async fn new(locale: Locale) -> Result<Self> {
        let _bcp47 = to_bcp47(locale)?;
        Ok(Self { locale })
    }

    pub fn locale(&self) -> Locale {
        self.locale
    }
}

impl Named for Macos26Stt {
    fn name(&self) -> &str {
        BACKEND_NAME
    }
}

impl StreamingSpeechToText for Macos26Stt {
    fn sample_rate(&self) -> u32 {
        // SpeechTranscriber's preferred format is 16 kHz Int16 mono; we
        // feed it Float32 16 kHz and let the Swift sidecar wrap it into
        // an AVAudioPCMBuffer.
        16_000
    }

    fn open_session(&self) -> Result<Box<dyn TranscriptionSession>> {
        // Re-validate the locale at session-open time. The real audio
        // path goes through build_local_backends_macos_native_26, not
        // through this session — see push_audio/finalize below.
        let _bcp47 = to_bcp47(self.locale)?;
        Ok(Box::new(Macos26TranscriptionSession::new()))
    }
}

/// Trait-shape shim. Audio in/out happens in the analyzer task spawned
/// by `build_local_backends_macos_native_26`; this session's methods
/// return errors loudly rather than silently doing nothing.
pub struct Macos26TranscriptionSession {
    _private: (),
}

impl Macos26TranscriptionSession {
    fn new() -> Self {
        Self { _private: () }
    }
}

impl TranscriptionSession for Macos26TranscriptionSession {
    fn push_audio(&mut self, _samples: &[f32]) -> Result<Vec<TranscriptSegment>> {
        Err(PrimerError::Speech(
            "Macos26TranscriptionSession::push_audio: audio flows through \
             the analyzer task, not the session trait. Use \
             build_local_backends_macos_native_26 instead."
                .into(),
        ))
    }

    fn finalize(self: Box<Self>) -> Result<Vec<TranscriptSegment>> {
        Err(PrimerError::Speech(
            "Macos26TranscriptionSession::finalize: see push_audio.".into(),
        ))
    }
}
