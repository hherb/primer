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
    fn push_text(&mut self, _text: &str, on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()> {
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
