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
/// typically insert a brief inter-phrase pause (~200 ms of silence)
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
    /// End of one phrase. Consumer typically inserts ~200 ms of silence.
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
mod tests {
    use super::*;

    /// Minimal in-memory VAD that replays a canned probability sequence.
    /// Validates the trait surface compiles and dyn-dispatches without
    /// pulling in any backend dependency.
    struct CannedVad {
        probs: std::vec::IntoIter<f32>,
        in_speech: bool,
    }

    impl CannedVad {
        fn new(probs: Vec<f32>) -> Self {
            Self {
                probs: probs.into_iter(),
                in_speech: false,
            }
        }
    }

    /// Mirrors a Silero-class detector's frame layout (16 kHz / 512-sample
    /// chunks); production threshold is 0.5.
    const CANNED_VAD_SAMPLE_RATE: u32 = 16_000;
    const CANNED_VAD_CHUNK_SAMPLES: usize = 512;
    const CANNED_VAD_THRESHOLD: f32 = 0.5;

    impl Named for CannedVad {
        fn name(&self) -> &str {
            "canned"
        }
    }

    impl VoiceActivityDetector for CannedVad {
        fn sample_rate(&self) -> u32 {
            CANNED_VAD_SAMPLE_RATE
        }
        fn chunk_samples(&self) -> usize {
            CANNED_VAD_CHUNK_SAMPLES
        }
        fn process_chunk(&mut self, _samples: &[f32]) -> Result<VadFrame> {
            let p = self.probs.next().unwrap_or(0.0);
            let is_speech = p >= CANNED_VAD_THRESHOLD;
            let event = match (self.in_speech, is_speech) {
                (false, true) => {
                    self.in_speech = true;
                    VadEvent::SpeechStart
                }
                (true, false) => {
                    self.in_speech = false;
                    VadEvent::SpeechEnd
                }
                _ => VadEvent::None,
            };
            Ok(VadFrame {
                speech_probability: p,
                event,
            })
        }
        fn reset(&mut self) {
            self.in_speech = false;
        }
    }

    #[test]
    fn vad_trait_object_dispatches() {
        let mut vad: Box<dyn VoiceActivityDetector> = Box::new(CannedVad::new(vec![0.1, 0.9, 0.1]));
        assert_eq!(vad.sample_rate(), CANNED_VAD_SAMPLE_RATE);
        assert_eq!(vad.chunk_samples(), CANNED_VAD_CHUNK_SAMPLES);
        let chunk = vec![0.0_f32; vad.chunk_samples()];
        let f0 = vad.process_chunk(&chunk).unwrap();
        let f1 = vad.process_chunk(&chunk).unwrap();
        let f2 = vad.process_chunk(&chunk).unwrap();
        assert_eq!(f0.event, VadEvent::None);
        assert_eq!(f1.event, VadEvent::SpeechStart);
        assert_eq!(f2.event, VadEvent::SpeechEnd);
    }

    /// Test sample rate matching the production setting; named so the
    /// timestamp arithmetic in the streaming-STT test below is obvious.
    const TEST_SAMPLE_RATE: u32 = 16_000;
    /// Milliseconds per second — used in the elapsed-ms calculation.
    const MS_PER_SECOND: u64 = 1_000;

    /// Mock streaming STT that emits one segment per push call from a
    /// canned script. Each push advances elapsed time by the duration
    /// of the supplied audio at [`TEST_SAMPLE_RATE`].
    struct CannedStreamStt;

    struct CannedSession {
        scripted: std::vec::IntoIter<&'static str>,
        elapsed_ms: u64,
    }

    impl TranscriptionSession for CannedSession {
        fn push_audio(&mut self, samples: &[f32]) -> Result<Vec<TranscriptSegment>> {
            let chunk_ms = (samples.len() as u64 * MS_PER_SECOND) / TEST_SAMPLE_RATE as u64;
            let start_ms = self.elapsed_ms;
            self.elapsed_ms += chunk_ms;
            match self.scripted.next() {
                Some(text) => Ok(vec![TranscriptSegment {
                    text: text.to_string(),
                    start_ms,
                    end_ms: self.elapsed_ms,
                }]),
                None => Ok(vec![]),
            }
        }
        fn finalize(self: Box<Self>) -> Result<Vec<TranscriptSegment>> {
            Ok(vec![])
        }
    }

    impl Named for CannedStreamStt {
        fn name(&self) -> &str {
            "canned-stream-stt"
        }
    }

    impl StreamingSpeechToText for CannedStreamStt {
        fn sample_rate(&self) -> u32 {
            TEST_SAMPLE_RATE
        }
        fn open_session(&self) -> Result<Box<dyn TranscriptionSession>> {
            Ok(Box::new(CannedSession {
                scripted: vec!["hello", " world"].into_iter(),
                elapsed_ms: 0,
            }))
        }
    }

    /// Mock one-shot STT that returns a canned transcript. Used only
    /// by the Named super-trait canary test below.
    struct CannedOneShotStt;

    impl Named for CannedOneShotStt {
        fn name(&self) -> &str {
            "canned-oneshot-stt"
        }
    }

    #[async_trait]
    impl SpeechToText for CannedOneShotStt {
        async fn transcribe(&self, _audio: &AudioBuffer) -> Result<Transcript> {
            Ok(Transcript {
                text: String::new(),
                language: None,
                confidence: None,
            })
        }
    }

    /// Mock TTS that returns a canned silent buffer. Used only by the
    /// Named super-trait canary test below.
    struct CannedTts;

    impl Named for CannedTts {
        fn name(&self) -> &str {
            "canned-tts"
        }
    }

    #[async_trait]
    impl TextToSpeech for CannedTts {
        async fn synthesize(&self, _text: &str, _voice: &VoiceProfile) -> Result<AudioBuffer> {
            Ok(AudioBuffer {
                samples: Vec::new(),
                sample_rate: CANNED_VAD_SAMPLE_RATE,
            })
        }
    }

    #[test]
    fn streaming_stt_session_yields_segments_and_finalizes() {
        let stt: Box<dyn StreamingSpeechToText> = Box::new(CannedStreamStt);
        assert_eq!(stt.sample_rate(), TEST_SAMPLE_RATE);
        let mut session = stt.open_session().unwrap();
        // One second of audio per push at the sample rate above.
        let one_second_chunk = vec![0.0_f32; TEST_SAMPLE_RATE as usize];
        let s0 = session.push_audio(&one_second_chunk).unwrap();
        let s1 = session.push_audio(&one_second_chunk).unwrap();
        let s2 = session.push_audio(&one_second_chunk).unwrap();
        assert_eq!(s0.len(), 1);
        assert_eq!(s0[0].text, "hello");
        assert_eq!(s0[0].start_ms, 0);
        assert_eq!(s0[0].end_ms, MS_PER_SECOND);
        assert_eq!(s1[0].text, " world");
        assert!(s2.is_empty());
        let trailing = session.finalize().unwrap();
        assert!(trailing.is_empty());
    }

    /// Canary that the `Named` super-trait is the single source of `name()`
    /// across every speech trait — `VoiceActivityDetector`, `SpeechToText`,
    /// `StreamingSpeechToText`, `TextToSpeech`, `StreamingTextToSpeech`.
    /// Adding a sixth leaf trait should add a sixth assertion here.
    #[test]
    fn named_super_trait_resolves_via_each_speech_trait() {
        let vad: Box<dyn VoiceActivityDetector> = Box::new(CannedVad::new(vec![]));
        assert_eq!(Named::name(&*vad), "canned");

        let stream_stt: Box<dyn StreamingSpeechToText> = Box::new(CannedStreamStt);
        assert_eq!(Named::name(&*stream_stt), "canned-stream-stt");

        let oneshot_stt: Box<dyn SpeechToText> = Box::new(CannedOneShotStt);
        assert_eq!(Named::name(&*oneshot_stt), "canned-oneshot-stt");

        let tts: Box<dyn TextToSpeech> = Box::new(CannedTts);
        assert_eq!(Named::name(&*tts), "canned-tts");

        let streaming_tts: Box<dyn StreamingTextToSpeech> = Box::new(CannedStreamingTts);
        assert_eq!(Named::name(&*streaming_tts), "canned-stream-tts");
    }

    /// Mock streaming-TTS that emits one canned `AudioChunk` per push.
    struct CannedStreamingTts;

    /// Sample rate used by the canned mock — matches a Piper-class voice
    /// so the value isn't a magic literal in the canary test.
    const CANNED_TTS_SAMPLE_RATE: u32 = 22_050;
    /// Each push from the canned mock yields this many samples.
    const CANNED_TTS_SAMPLES_PER_CHUNK: usize = 64;

    struct CannedSynthesisSession {
        scripted: std::vec::IntoIter<&'static str>,
        sample_rate: u32,
    }

    impl SynthesisSession for CannedSynthesisSession {
        fn push_text(
            &mut self,
            _text: &str,
            on_event: &mut dyn FnMut(SynthesisEvent),
        ) -> Result<()> {
            if self.scripted.next().is_some() {
                on_event(SynthesisEvent::Audio(AudioChunk {
                    samples: vec![0.0; CANNED_TTS_SAMPLES_PER_CHUNK],
                    sample_rate: self.sample_rate,
                }));
                on_event(SynthesisEvent::PhraseEnd);
            }
            Ok(())
        }

        fn finalize(self: Box<Self>, _on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()> {
            Ok(())
        }
    }

    impl Named for CannedStreamingTts {
        fn name(&self) -> &str {
            "canned-stream-tts"
        }
    }

    impl StreamingTextToSpeech for CannedStreamingTts {
        fn sample_rate(&self) -> u32 {
            CANNED_TTS_SAMPLE_RATE
        }
        fn open_session(&self, _voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>> {
            Ok(Box::new(CannedSynthesisSession {
                scripted: vec!["alpha", "beta"].into_iter(),
                sample_rate: CANNED_TTS_SAMPLE_RATE,
            }))
        }
    }

    #[test]
    fn streaming_tts_session_yields_chunks_and_finalizes() {
        let tts: Box<dyn StreamingTextToSpeech> = Box::new(CannedStreamingTts);
        assert_eq!(Named::name(&*tts), "canned-stream-tts");
        assert_eq!(tts.sample_rate(), CANNED_TTS_SAMPLE_RATE);
        let voice = VoiceProfile::default();
        let mut session = tts.open_session(&voice).unwrap();

        // Push 1: script has "alpha" — expect Audio + PhraseEnd.
        let mut events: Vec<SynthesisEvent> = Vec::new();
        session
            .push_text("hello.", &mut |e| events.push(e))
            .unwrap();
        assert_eq!(events.len(), 2, "one Audio + one PhraseEnd per push");
        assert!(matches!(events[0], SynthesisEvent::Audio(_)));
        assert!(matches!(events[1], SynthesisEvent::PhraseEnd));
        if let SynthesisEvent::Audio(ref chunk) = events[0] {
            assert_eq!(chunk.samples.len(), CANNED_TTS_SAMPLES_PER_CHUNK);
            assert_eq!(chunk.sample_rate, CANNED_TTS_SAMPLE_RATE);
        }

        // Push 2: script has "beta" — expect Audio + PhraseEnd.
        let mut events2: Vec<SynthesisEvent> = Vec::new();
        session
            .push_text(" world.", &mut |e| events2.push(e))
            .unwrap();
        assert_eq!(events2.len(), 2);

        // Push 3: script exhausted — expect no events.
        let mut events3: Vec<SynthesisEvent> = Vec::new();
        session.push_text("", &mut |e| events3.push(e)).unwrap();
        assert!(
            events3.is_empty(),
            "empty push (script exhausted) emits no events"
        );

        // finalize: scripted iterator already exhausted; no trailing events expected.
        let mut events4: Vec<SynthesisEvent> = Vec::new();
        session.finalize(&mut |e| events4.push(e)).unwrap();
        assert!(events4.is_empty());
    }

    /// Pin the exact `[Audio, PhraseEnd]` event order for one push. The
    /// existing test asserts shape; this one asserts ordering as a
    /// separate signal so a future regression that reverses the order
    /// (or drops `PhraseEnd`) fails with a clearly diagnostic message.
    #[test]
    fn synthesis_session_fires_audio_before_phrase_end() {
        let tts: Box<dyn StreamingTextToSpeech> = Box::new(CannedStreamingTts);
        let mut session = tts.open_session(&VoiceProfile::default()).unwrap();
        let mut order: Vec<&'static str> = Vec::new();
        let mut sink = |e: SynthesisEvent| {
            order.push(match e {
                SynthesisEvent::Audio(_) => "audio",
                SynthesisEvent::PhraseEnd => "phrase_end",
            });
        };
        session.push_text("ping.", &mut sink).unwrap();
        assert_eq!(order, vec!["audio", "phrase_end"]);
    }

    /// Compile-time canary: `SynthesisSession` must remain object-safe so
    /// `Box<dyn SynthesisSession>` (the return type of
    /// `StreamingTextToSpeech::open_session`) keeps working. The
    /// `&mut dyn FnMut(SynthesisEvent)` callback is the load-bearing
    /// constraint — any future trait method that takes `impl FnMut`,
    /// returns `impl Trait`, or otherwise loses object safety will fail
    /// to compile here.
    #[test]
    fn synthesis_session_is_object_safe() {
        fn _accepts_boxed(_s: Box<dyn SynthesisSession>) {}
    }
}
