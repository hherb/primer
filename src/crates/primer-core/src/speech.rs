//! Speech traits — VAD, STT, and TTS abstractions.
//!
//! Concrete implementations live in `primer-speech` behind feature flags:
//! - VAD: Silero (via `silero-vad-rust`), feature `silero`.
//! - STT: whisper.cpp (via `whisper-cpp-plus`), feature `whisper`.
//! - TTS: Piper (planned, via `piper-rs`), feature `piper`.
//!
//! All trait method signatures are deliberately backend-agnostic. A new
//! backend should be a drop-in alternative without touching this file.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Raw audio data with format metadata.
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    /// PCM samples, f32, mono.
    pub samples: Vec<f32>,
    /// Sample rate in Hz (typically 16000 for STT, 22050 for TTS).
    pub sample_rate: u32,
}

/// Result of speech-to-text transcription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    /// The transcribed text.
    pub text: String,
    /// Language detected (ISO 639-1 code).
    pub language: Option<String>,
    /// Confidence score (0.0 – 1.0), if available.
    pub confidence: Option<f32>,
}

/// Configuration for a TTS voice.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceProfile {
    /// Voice model identifier (e.g., "en_US-amy-medium").
    pub model_id: String,
    /// Speaking rate multiplier (1.0 = normal).
    pub rate: f32,
    /// Pitch adjustment (-1.0 to 1.0).
    pub pitch: f32,
}

impl Default for VoiceProfile {
    fn default() -> Self {
        Self {
            model_id: "en_US-amy-medium".to_string(),
            rate: 1.0,
            pitch: 0.0,
        }
    }
}

/// Common identifier for any speech backend.
///
/// Every speech trait inherits from `Named` so a single struct that
/// implements both the one-shot and streaming variants of STT or TTS
/// only writes its `name()` impl once. Without this, both leaf traits
/// would each declare a `fn name(&self) -> &str` of their own, and a
/// backend implementing both would end up with two same-signature
/// methods that callers would have to disambiguate via UFCS
/// (`<T as SpeechToText>::name(...)`) at every direct call site.
pub trait Named {
    fn name(&self) -> &str;
}

/// State-transition event emitted by a [`VoiceActivityDetector`] for a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadEvent {
    /// No transition; the previous speech/silence state continues.
    None,
    /// First chunk of a new speech segment (silence → speech).
    SpeechStart,
    /// First chunk of silence after a speech segment (speech → silence).
    SpeechEnd,
}

/// Per-chunk output from a [`VoiceActivityDetector`].
#[derive(Debug, Clone, Copy)]
pub struct VadFrame {
    /// Speech probability for this chunk (0.0 – 1.0).
    pub speech_probability: f32,
    /// State transition (if any) observed at this chunk.
    pub event: VadEvent,
}

/// Streaming voice-activity detector.
///
/// VAD is inherently stateful: the detector keeps internal history (model
/// hidden state plus the silence-debounce counter). Each conversation stream
/// owns its own detector — implementations are `Send` but not `Sync`.
///
/// Callers feed exactly [`Self::chunk_samples`] f32 mono samples per call,
/// at the rate reported by [`Self::sample_rate`]. The detector returns a
/// per-chunk probability and a state-transition event the caller can use to
/// trigger STT, end-of-utterance handling, etc.
pub trait VoiceActivityDetector: Named + Send {
    /// Sample rate the detector expects (Hz).
    fn sample_rate(&self) -> u32;

    /// Number of f32 samples per chunk fed to [`Self::process_chunk`].
    fn chunk_samples(&self) -> usize;

    /// Process exactly [`Self::chunk_samples`] mono f32 samples.
    ///
    /// Implementations must return `Err` if `samples.len()` differs from
    /// [`Self::chunk_samples`] — callers rely on a deterministic frame
    /// size for timing arithmetic.
    fn process_chunk(&mut self, samples: &[f32]) -> Result<VadFrame>;

    /// Reset internal state (between utterances or sessions).
    fn reset(&mut self);
}

/// One-shot speech-to-text backend.
///
/// Used for offline transcription of complete audio buffers. For live
/// interactive use prefer [`StreamingSpeechToText`] — it surfaces partial
/// segments as they arrive instead of blocking until the entire buffer
/// is processed.
#[async_trait]
pub trait SpeechToText: Named + Send + Sync {
    /// Transcribe an audio buffer to text.
    async fn transcribe(&self, audio: &AudioBuffer) -> Result<Transcript>;
}

/// One transcribed segment from a streaming session.
///
/// Whisper-class models naturally emit text in segment-sized chunks (one
/// sentence or ~5–10 s of audio); we surface those chunks rather than
/// pretending to deliver token-by-token output, since underlying ASR rarely
/// is. Concatenate the `text` of every segment to reconstruct the utterance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    /// Recognised text for this segment. Concatenate every segment's
    /// text in order to reconstruct the full utterance.
    pub text: String,
    /// Start of this segment relative to session open (ms).
    pub start_ms: u64,
    /// End of this segment relative to session open (ms).
    pub end_ms: u64,
}

/// A single streaming-transcription session.
///
/// Created by [`StreamingSpeechToText::open_session`]. Push f32 mono samples
/// at the parent backend's [`StreamingSpeechToText::sample_rate`] via
/// [`Self::push_audio`]; each push may emit zero or more segments as soon
/// as the underlying model has enough context. Call [`Self::finalize`] once
/// the utterance is complete (typically on `VadEvent::SpeechEnd`) to drain
/// the trailing buffer.
///
/// Sessions are `Send` but not `Sync`: each utterance owns its own session.
pub trait TranscriptionSession: Send {
    /// Push samples; receive any segments that became available as a result.
    fn push_audio(&mut self, samples: &[f32]) -> Result<Vec<TranscriptSegment>>;

    /// Drain remaining buffered audio and finalize. Consumes the session.
    fn finalize(self: Box<Self>) -> Result<Vec<TranscriptSegment>>;
}

/// Streaming speech-to-text backend.
///
/// Open one [`TranscriptionSession`] per child utterance. The backend itself
/// is shareable across sessions (`Send + Sync`); per-session state lives
/// inside the session handle.
///
/// A backend may also implement the one-shot [`SpeechToText`] trait —
/// `name()` lives on the [`Named`] super-trait so it's only written once
/// per backend struct.
pub trait StreamingSpeechToText: Named + Send + Sync {
    /// Sample rate the backend expects (Hz). Sessions reject mismatched audio.
    fn sample_rate(&self) -> u32;

    /// Open a fresh transcription session. Each utterance gets its own.
    fn open_session(&self) -> Result<Box<dyn TranscriptionSession>>;
}

/// One-shot text-to-speech backend.
///
/// Synthesises a complete utterance into a single [`AudioBuffer`]. For
/// interactive voice loops a streaming variant will be added in the next
/// step of the speech pipeline (see `docs/primer_TTS_next_step.md`).
#[async_trait]
pub trait TextToSpeech: Named + Send + Sync {
    /// Synthesize text to an audio buffer.
    async fn synthesize(&self, text: &str, voice: &VoiceProfile) -> Result<AudioBuffer>;
}

/// One PCM chunk emitted by a [`SynthesisSession`] during streaming.
///
/// Concatenate the `samples` of every chunk in order to reconstruct the
/// full utterance. `sample_rate` is carried per-chunk even though every
/// chunk in one session shares one — keeps this type usable by audio
/// sinks that don't hold a reference to the originating backend.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    /// PCM samples, f32, mono.
    pub samples: Vec<f32>,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

/// One event emitted by a streaming [`SynthesisSession`].
///
/// `Audio(chunk)` carries PCM data — consumers forward it to the
/// speaker immediately. `PhraseEnd` is a boundary marker — consumers
/// typically insert a brief inter-phrase pause (see
/// [`crate::consts::speech::DEFAULT_INTER_PHRASE_SILENCE_MS`])
/// before the next phrase's audio.
///
/// Separating audio from boundaries (rather than packing a
/// `bool is_phrase_end` onto chunks) keeps phrase-detection knowledge
/// where it belongs — in the producer's `PhraseSplitter` — and avoids
/// per-chunk overhead in the steady state.
#[derive(Debug, Clone)]
pub enum SynthesisEvent {
    /// PCM data became available.
    Audio(AudioChunk),
    /// End of one phrase. Consumer typically inserts a brief inter-phrase
    /// pause — see
    /// [`crate::consts::speech::DEFAULT_INTER_PHRASE_SILENCE_MS`].
    PhraseEnd,
}

/// A single streaming-synthesis session.
///
/// Created by [`StreamingTextToSpeech::open_session`]. Push partial
/// text from the LLM via [`Self::push_text`]; the session invokes
/// `on_event` for each PCM chunk and phrase boundary as soon as it's
/// available — without buffering an entire phrase. Call
/// [`Self::finalize`] when the LLM stream has ended to drain the
/// trailing buffer. `Send` but not `Sync`: each Primer turn owns its
/// own session.
///
/// # Blocking
///
/// Both [`Self::push_text`] and [`Self::finalize`] are synchronous and
/// may run CPU-heavy synthesis (e.g. ONNX inference) on the calling
/// thread. Async callers MUST wrap session calls in
/// `tokio::task::spawn_blocking` (or an equivalent) to keep the
/// runtime free. The trait stays sync so pure backends don't pay an
/// async surface tax.
pub trait SynthesisSession: Send {
    /// Push text. The synthesiser invokes `on_event` for each PCM
    /// chunk and phrase boundary as soon as it's available — without
    /// buffering an entire phrase.
    ///
    /// `on_event` may be invoked zero or more times during one call.
    /// May block the calling thread for the duration of synthesis —
    /// see the trait-level `# Blocking` note.
    fn push_text(&mut self, text: &str, on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()>;

    /// Drain remaining buffered text and finalize. Consumes the
    /// session. Fires the same events as [`Self::push_text`] for any
    /// trailing partial phrase.
    ///
    /// May block for one final synthesis call — see the trait-level
    /// `# Blocking` note.
    fn finalize(self: Box<Self>, on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()>;
}

/// Streaming text-to-speech backend.
///
/// Open one [`SynthesisSession`] per Primer turn. The backend itself is
/// shareable across sessions (`Send + Sync`); per-session state lives
/// inside the session handle. A backend may also implement the one-shot
/// [`TextToSpeech`] trait — `name()` lives on the [`Named`] super-trait
/// so it's only written once per backend struct.
pub trait StreamingTextToSpeech: Named + Send + Sync {
    /// Sample rate of audio chunks this backend will emit (Hz). Carried
    /// on each [`AudioChunk`] as well so downstream sinks don't need to
    /// hold a reference to this backend.
    fn sample_rate(&self) -> u32;

    /// Open a fresh synthesis session for the given voice profile.
    ///
    /// May error if the backend cannot serve `voice` (for example, the
    /// loaded model has a different `model_id` than the requested voice).
    fn open_session(&self, voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>>;
}

#[cfg(test)]
mod tests;
