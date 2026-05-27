//! `Macos26Stt` is a thin locale-validating constructor for the
//! macOS 26 SpeechAnalyzer-backed STT path.
//!
//! The real audio path is owned by
//! `voice_loop::backends::build_local_backends_macos_native_26`, which
//! spawns the SpeechAnalyzer task and drives audio in/out directly. This
//! type exists so callers can fail fast on an unsupported locale (e.g.
//! Hindi today) at startup-time rather than at first speech — and so
//! `crate::macos26` has a small, named handle to the locale validation
//! result. It does NOT implement `StreamingSpeechToText`: the trait would
//! require a `TranscriptionSession` whose `push_audio` / `finalize` were
//! never wired to the analyzer task, and surfacing always-erroring
//! methods on the trait surface invited the misreading that "this is
//! where audio flows through". See issue #142.

#![cfg(all(target_vendor = "apple", feature = "macos-native-26"))]

use primer_core::error::Result;
use primer_core::i18n::Locale;
use primer_core::speech::Named;

use crate::macos26::locale::to_bcp47;

const BACKEND_NAME: &str = "macos-26-speech-analyzer";

/// Locale-validated handle for the macOS 26 SpeechAnalyzer STT path.
///
/// Construction fails loudly if the locale isn't supported (Hindi today
/// — `SpeechTranscriber` has no on-device `hi-IN` as of macOS 26.5).
/// Owners of an instance can hand it to logging / observability code
/// alongside other speech backends via the [`Named`] trait, but the
/// audio path is the builder's responsibility, not this struct's.
#[derive(Debug)]
pub struct Macos26Stt {
    locale: Locale,
}

impl Macos26Stt {
    /// Construct. Validates the locale eagerly via `to_bcp47` so an
    /// unsupported locale is a startup-time error.
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
