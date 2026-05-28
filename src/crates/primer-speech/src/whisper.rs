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
    AudioBuffer, Named, SpeechToText, StreamingSpeechToText, Transcript, TranscriptSegment,
    TranscriptionSession,
};
use whisper_cpp_plus::{
    FullParams, SamplingStrategy, Segment, TranscriptionParams, WhisperContext, WhisperStream,
};

use crate::time_ms::clamp_signed_ms_to_u64;
use stream_cache::StreamCache;

mod stream_cache {
    //! Single-slot value cache for "construct is expensive, reset is
    //! cheap" types. Used by [`super::WhisperStt`] to reuse a
    //! [`whisper_cpp_plus::WhisperStream`] across utterances (≈500 ms
    //! of KV-cache allocation per cold-start; closes #133).
    //!
    //! The cache is **deliberately not lock-free**: per-instance
    //! contention is impossible by construction (one VAD-driven
    //! session active at a time per backend) so a plain `Mutex` keeps
    //! the type Send+Sync without atomics.
    //!
    //! Generic over `T` so the take/put invariants can be unit-tested
    //! against `i32` — `WhisperStream` requires a real GGML model on
    //! disk to construct, which is outside the unit-test budget.

    use std::sync::Mutex;

    /// Single-slot cache for a value of type `T`.
    pub(super) struct StreamCache<T> {
        slot: Mutex<Option<T>>,
    }

    impl<T> StreamCache<T> {
        /// New empty cache. The first [`Self::take`] returns `None`,
        /// telling the caller to pay the cold-start cost.
        pub(super) fn new() -> Self {
            Self {
                slot: Mutex::new(None),
            }
        }

        /// Take the cached value if present. Returns `None` on a
        /// poisoned mutex too — the caller's cold-start path is the
        /// same recovery, so we don't propagate the poison.
        pub(super) fn take(&self) -> Option<T> {
            self.slot.lock().ok().and_then(|mut g| g.take())
        }

        /// Put a value back into the cache. Silently drops the value
        /// when the mutex is poisoned — symmetric with [`Self::take`]:
        /// the next caller will simply construct a fresh value, which
        /// is functionally identical to a cache miss.
        pub(super) fn put(&self, value: T) {
            if let Ok(mut g) = self.slot.lock() {
                *g = Some(value);
            }
        }
    }
}

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
///
/// Streaming sessions reuse a single [`WhisperStream`] across utterances
/// via [`StreamCache`] — `WhisperStream::new` allocates ≈75 MB of KV
/// cache + GPU compute buffers (≈500 ms wallclock on Apple Silicon),
/// while [`WhisperStream::reset`] just clears per-utterance state
/// (audio_buf, prompt_tokens, n_iter). The single-slot cache is sized
/// to the one-active-session-at-a-time invariant the voice loop
/// imposes — a racing second `open_session` falls back to a fresh
/// construction, which is functionally identical to the cold-start.
/// Closes #133.
pub struct WhisperStt {
    ctx: Arc<WhisperContext>,
    language: String,
    cache: Arc<StreamCache<WhisperStream>>,
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
            cache: Arc::new(StreamCache::new()),
        })
    }

    /// Set the transcription language (ISO 639-1; default `"en"`).
    pub fn with_language(mut self, lang: impl Into<String>) -> Self {
        self.language = lang.into();
        self
    }

    /// Currently-configured transcription language (ISO 639-1).
    /// Returns the default (`"en"`) when [`with_language`] was never called.
    /// Public so callers — most importantly `build_local_backends` — can
    /// be tested for correct locale propagation without loading a real
    /// Whisper context end-to-end.
    pub fn language(&self) -> &str {
        &self.language
    }
}

impl Named for WhisperStt {
    fn name(&self) -> &str {
        BACKEND_NAME
    }
}

#[async_trait]
impl SpeechToText for WhisperStt {
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
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }

    fn open_session(&self) -> Result<Box<dyn TranscriptionSession>> {
        let stream = match self.cache.take() {
            Some(mut s) => {
                // Reset clears `audio_buf`, `pcmf32_old`, `prompt_tokens`,
                // `n_iter`, and `total_samples_processed`. The KV cache
                // and compute buffers (the ≈500 ms-to-allocate state)
                // live inside the underlying `WhisperState` and are
                // preserved across reset — that's the whole point.
                s.reset();
                s
            }
            None => {
                let params = FullParams::new(SamplingStrategy::Greedy {
                    best_of: STREAMING_BEST_OF,
                })
                .language(&self.language);
                WhisperStream::new(&self.ctx, params)
                    .map_err(|e| PrimerError::Speech(format!("open whisper stream: {e}")))?
            }
        };
        Ok(Box::new(WhisperSession {
            stream: Some(stream),
            cache: Arc::clone(&self.cache),
        }))
    }
}

struct WhisperSession {
    /// `Option` only so `finalize` and `Drop` can `take()` ownership of
    /// the stream out of the session and return it to the parent cache.
    /// Outside those two exit paths the slot is always `Some`.
    stream: Option<WhisperStream>,
    cache: Arc<StreamCache<WhisperStream>>,
}

impl TranscriptionSession for WhisperSession {
    fn push_audio(&mut self, samples: &[f32]) -> Result<Vec<TranscriptSegment>> {
        let stream = self
            .stream
            .as_mut()
            .expect("WhisperSession::push_audio after stream was returned to cache");
        stream.feed_audio(samples);
        match stream
            .process_step()
            .map_err(|e| PrimerError::Speech(format!("whisper step: {e}")))?
        {
            None => Ok(vec![]),
            Some(segments) => Ok(segments.into_iter().map(to_transcript_segment).collect()),
        }
    }

    fn finalize(mut self: Box<Self>) -> Result<Vec<TranscriptSegment>> {
        let mut stream = self
            .stream
            .take()
            .expect("WhisperSession::finalize after stream was returned to cache");
        let flush_result = stream.flush();
        // Return the stream to the cache regardless of `flush` outcome:
        // the next `open_session` calls `reset()` which clears any
        // partial state. Holding the stream back on a flush error would
        // force every subsequent utterance to pay the ≈500 ms cold-start
        // cost for the rest of the process lifetime.
        self.cache.put(stream);
        let segments =
            flush_result.map_err(|e| PrimerError::Speech(format!("whisper flush: {e}")))?;
        Ok(segments.into_iter().map(to_transcript_segment).collect())
    }
}

impl Drop for WhisperSession {
    /// Return the stream to the cache if `finalize` was never called
    /// (early-return on a `push_audio` error, panic in the caller, …).
    /// `WhisperStream::reset` in the next `open_session` normalises any
    /// partial `audio_buf` / `prompt_tokens` state, so the dropped
    /// stream is safe to reuse.
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            self.cache.put(stream);
        }
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

#[cfg(test)]
mod tests {
    use primer_core::i18n::Locale;

    /// Pin `Locale::pack_id()` to ISO-639-1 codes that Whisper accepts
    /// as transcription language. `build_local_backends` passes this
    /// value through to [`WhisperStt::with_language`] so multilingual
    /// Whisper models transcribe in the learner's locale instead of the
    /// default `"en"`. Regression guard for the bug where German speech
    /// was transcribed as English under the multilingual `small.bin`
    /// model because the locale was never wired through.
    #[test]
    fn pack_id_is_iso_639_1_for_whisper() {
        assert_eq!(Locale::English.pack_id(), "en");
        assert_eq!(Locale::German.pack_id(), "de");
    }
}

#[cfg(test)]
mod stream_cache_tests {
    //! Unit tests for the [`stream_cache::StreamCache`] type used by
    //! [`WhisperStt`] to reuse a [`WhisperStream`] across utterances
    //! (closes #133). The cache itself is generic so we test against
    //! `i32` — that side-steps `WhisperStream`'s "needs a real GGML
    //! model on disk" construction cost and lets us pin the
    //! take/put invariants without an integration test.
    use super::stream_cache::StreamCache;

    /// A freshly-constructed cache holds nothing — first `take()`
    /// returns `None`, which is what tells [`WhisperStt::open_session`]
    /// to pay the cold-start cost (≈500 ms KV-cache allocation).
    #[test]
    fn new_cache_is_empty() {
        let cache: StreamCache<i32> = StreamCache::new();
        assert!(cache.take().is_none());
    }

    /// After `put`, the next `take` returns the value. This is the
    /// hot path: a finalised session returns its stream and the next
    /// `open_session` picks it up.
    #[test]
    fn put_then_take_returns_value() {
        let cache: StreamCache<i32> = StreamCache::new();
        cache.put(42);
        assert_eq!(cache.take(), Some(42));
    }

    /// `take` consumes the cached value — the second `take` sees
    /// nothing. This is what makes the cache a single-slot reservation:
    /// if two `open_session` calls race, the loser falls back to
    /// constructing a fresh stream rather than sharing a stream that
    /// holds per-utterance state (audio_buf, prompt_tokens, n_iter).
    #[test]
    fn take_empties_cache() {
        let cache: StreamCache<i32> = StreamCache::new();
        cache.put(42);
        let _ = cache.take();
        assert!(cache.take().is_none());
    }

    /// `put` overwrites whatever was there. This is the Drop-after-
    /// finalize safety case: if `WhisperSession::finalize` already put
    /// the stream back and the `Drop` impl also calls `put`, the
    /// later (None) doesn't matter because finalize takes the stream
    /// out of the session first — but we still want the invariant
    /// "last put wins" to hold in case finalize ordering ever changes.
    #[test]
    fn put_replaces_existing_value() {
        let cache: StreamCache<i32> = StreamCache::new();
        cache.put(1);
        cache.put(2);
        assert_eq!(cache.take(), Some(2));
    }
}
