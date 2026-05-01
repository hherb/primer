//! Whisper.cpp implementation of [`SpeechToText`] and
//! [`StreamingSpeechToText`].
//!
//! Wraps `whisper-cpp-plus`. A GGML/GGUF model file is loaded from disk on
//! construction (paths like `ggml-small.en.bin`; users typically download
//! these from huggingface.co/ggerganov/whisper.cpp). The same loaded
//! context is shared across all sessions via `Arc`.
//!
//! # Build prerequisites
//!
//! The `whisper` feature compiles `whisper-cpp-plus-sys`, which builds
//! whisper.cpp from its bundled C++ source. The build host needs a C++
//! toolchain (g++ or clang) and CMake. After the first successful build
//! the static library is cached under the cargo target directory.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use primer_core::error::{PrimerError, Result};
use primer_core::speech::{
    AudioBuffer, SpeechToText, StreamingSpeechToText, Transcript, TranscriptSegment,
    TranscriptionSession,
};
use whisper_cpp_plus::{
    FullParams, SamplingStrategy, Segment, TranscriptionParams, WhisperContext, WhisperStream,
};

use crate::time_ms::clamp_signed_ms_to_u64;

/// Sample rate Whisper requires (16 kHz mono f32).
const SAMPLE_RATE: u32 = 16_000;

/// Default transcription language (ISO 639-1). English is the only
/// language Whisper's `.en` distillates support; multilingual models
/// auto-detect when given a non-`en` value but using "en" by default
/// keeps things deterministic for the Primer's English-first audience.
const DEFAULT_LANGUAGE: &str = "en";

/// `best_of` for the streaming sampler. 1 = greedy decoding (fastest,
/// lowest quality drop in our latency-dominated streaming path). Beam
/// search would lift WER on noisy/child audio but at multiplicative
/// cost; revisit only after profiling shows headroom.
const STREAMING_BEST_OF: i32 = 1;

/// Backend identifier returned by [`WhisperStt::name`].
const BACKEND_NAME: &str = "whisper-cpp";

/// Whisper.cpp speech-to-text backend.
///
/// Loads a GGML/GGUF model from disk on construction; the same loaded
/// context is shared across all sessions via `Arc`, which makes one
/// `WhisperStt` cheap to clone and safe to call from many tasks. Both
/// the one-shot [`SpeechToText`] and the streaming
/// [`StreamingSpeechToText`] traits are implemented; pick whichever
/// matches the call site.
pub struct WhisperStt {
    ctx: Arc<WhisperContext>,
    language: String,
}

impl WhisperStt {
    /// Load a whisper.cpp model from `model_path`. The model must be in
    /// GGML/GGUF format (e.g. `ggml-small.en.bin`).
    pub fn new(model_path: impl AsRef<Path>) -> Result<Self> {
        let ctx = WhisperContext::new(model_path.as_ref())
            .map_err(|e| PrimerError::Speech(format!("load whisper model: {e}")))?;
        Ok(Self {
            ctx: Arc::new(ctx),
            language: DEFAULT_LANGUAGE.to_string(),
        })
    }

    /// Set the transcription language (ISO 639-1; default `"en"`).
    pub fn with_language(mut self, lang: impl Into<String>) -> Self {
        self.language = lang.into();
        self
    }
}

#[async_trait]
impl SpeechToText for WhisperStt {
    fn name(&self) -> &str {
        BACKEND_NAME
    }

    async fn transcribe(&self, audio: &AudioBuffer) -> Result<Transcript> {
        if audio.sample_rate != SAMPLE_RATE {
            return Err(PrimerError::Speech(format!(
                "Whisper requires {SAMPLE_RATE}Hz audio, got {}Hz",
                audio.sample_rate
            )));
        }
        let ctx = self.ctx.clone();
        let samples = audio.samples.clone();
        let language = self.language.clone();
        let text = tokio::task::spawn_blocking(move || -> Result<String> {
            let params = TranscriptionParams::builder().language(&language).build();
            let result = ctx
                .transcribe_with_params(&samples, params)
                .map_err(|e| PrimerError::Speech(format!("whisper transcribe: {e}")))?;
            Ok(result.text)
        })
        .await
        .map_err(|e| PrimerError::Speech(format!("whisper join: {e}")))??;

        Ok(Transcript {
            text,
            language: Some(self.language.clone()),
            confidence: None,
        })
    }
}

impl StreamingSpeechToText for WhisperStt {
    fn name(&self) -> &str {
        BACKEND_NAME
    }

    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }

    fn open_session(&self) -> Result<Box<dyn TranscriptionSession>> {
        let params = FullParams::new(SamplingStrategy::Greedy {
            best_of: STREAMING_BEST_OF,
        })
        .language(&self.language);
        let stream = WhisperStream::new(&self.ctx, params)
            .map_err(|e| PrimerError::Speech(format!("open whisper stream: {e}")))?;
        Ok(Box::new(WhisperSession { stream }))
    }
}

struct WhisperSession {
    stream: WhisperStream,
}

impl TranscriptionSession for WhisperSession {
    fn push_audio(&mut self, samples: &[f32]) -> Result<Vec<TranscriptSegment>> {
        self.stream.feed_audio(samples);
        match self
            .stream
            .process_step()
            .map_err(|e| PrimerError::Speech(format!("whisper step: {e}")))?
        {
            None => Ok(vec![]),
            Some(segments) => Ok(segments.into_iter().map(to_transcript_segment).collect()),
        }
    }

    fn finalize(mut self: Box<Self>) -> Result<Vec<TranscriptSegment>> {
        let segments = self
            .stream
            .flush()
            .map_err(|e| PrimerError::Speech(format!("whisper flush: {e}")))?;
        Ok(segments.into_iter().map(to_transcript_segment).collect())
    }
}

/// Map a whisper.cpp segment to our [`TranscriptSegment`].
///
/// Whisper's timestamps are signed; in practice they're non-negative,
/// but the type allows it so we clamp through [`clamp_signed_ms_to_u64`]
/// rather than reinterpret-casting.
fn to_transcript_segment(seg: Segment) -> TranscriptSegment {
    TranscriptSegment {
        text: seg.text,
        start_ms: clamp_signed_ms_to_u64(seg.start_ms),
        end_ms: clamp_signed_ms_to_u64(seg.end_ms),
    }
}
