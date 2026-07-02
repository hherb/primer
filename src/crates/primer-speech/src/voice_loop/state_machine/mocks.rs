//! Test-only mocks and unit tests for the voice-loop state machine.
//!
//! Relocated verbatim from `state_machine.rs` (declared there as
//! `#[cfg(test)] mod mocks;`) to keep that file focused on the loop itself.
//! `super::` resolves to the `state_machine` module, exactly as when this code
//! lived inline — so the mock backends and the `#[tokio::test]` cases reach the
//! loop's public types (`LoopBackends`, `run_loop_borrowed`, …) unchanged.

use std::sync::{Arc, Mutex};

use crate::voice_loop::observer::{ExitReason, LoopObserver, TurnCompletePayload, VoiceState};

use primer_core::error::Result;
use primer_core::speech::{
    AudioChunk, Named, StreamingSpeechToText, StreamingTextToSpeech, SynthesisEvent,
    SynthesisSession, TranscriptSegment, TranscriptionSession, VoiceProfile,
};

/// Mock streaming STT: emits a fixed transcript on `finalize`.
pub struct MockStreamingStt {
    finalize_text: String,
}

impl MockStreamingStt {
    pub fn new(finalize_text: impl Into<String>) -> Self {
        Self {
            finalize_text: finalize_text.into(),
        }
    }
}

impl Named for MockStreamingStt {
    fn name(&self) -> &str {
        "mock-stt"
    }
}

impl StreamingSpeechToText for MockStreamingStt {
    fn sample_rate(&self) -> u32 {
        16_000
    }
    fn open_session(&self) -> Result<Box<dyn TranscriptionSession>> {
        Ok(Box::new(MockSttSession {
            final_text: self.finalize_text.clone(),
        }))
    }
}

struct MockSttSession {
    final_text: String,
}

impl TranscriptionSession for MockSttSession {
    fn push_audio(&mut self, _samples: &[f32]) -> Result<Vec<TranscriptSegment>> {
        Ok(vec![])
    }
    fn finalize(self: Box<Self>) -> Result<Vec<TranscriptSegment>> {
        Ok(vec![TranscriptSegment {
            text: self.final_text,
            start_ms: 0,
            end_ms: 1_000,
        }])
    }
}

/// Scriptable streaming STT: yields a different `finalize` text on
/// each successive `open_session()` call, draining a FIFO queue. Once
/// the queue is exhausted any further session finalizes to the empty
/// string. Required to exercise the cancel-and-retry path where the
/// first and second STT sessions return different partial transcripts
/// — see issue #103 / `cancel_and_retry_stitches_full_transcript`.
pub struct ScriptedStreamingStt {
    pending: Arc<Mutex<std::collections::VecDeque<String>>>,
}

impl ScriptedStreamingStt {
    pub fn new<I, S>(texts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            pending: Arc::new(Mutex::new(texts.into_iter().map(Into::into).collect())),
        }
    }
}

impl Named for ScriptedStreamingStt {
    fn name(&self) -> &str {
        "scripted-stt"
    }
}

impl StreamingSpeechToText for ScriptedStreamingStt {
    fn sample_rate(&self) -> u32 {
        16_000
    }
    fn open_session(&self) -> Result<Box<dyn TranscriptionSession>> {
        let next = self.pending.lock().unwrap().pop_front().unwrap_or_default();
        Ok(Box::new(MockSttSession { final_text: next }))
    }
}

/// Mock streaming TTS: emits one fixed AudioChunk per `push_text` call.
pub struct MockStreamingTts {
    chunk_samples: usize,
}

impl MockStreamingTts {
    pub fn new(chunk_samples: usize) -> Self {
        Self { chunk_samples }
    }
}

impl Named for MockStreamingTts {
    fn name(&self) -> &str {
        "mock-tts"
    }
}

impl StreamingTextToSpeech for MockStreamingTts {
    fn sample_rate(&self) -> u32 {
        22_050
    }
    fn open_session(&self, _voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>> {
        Ok(Box::new(MockTtsSession {
            chunk_samples: self.chunk_samples,
        }))
    }
}

struct MockTtsSession {
    chunk_samples: usize,
}

impl SynthesisSession for MockTtsSession {
    fn push_text(&mut self, text: &str, on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        on_event(SynthesisEvent::Audio(AudioChunk {
            samples: vec![0.5; self.chunk_samples],
            sample_rate: 22_050,
        }));
        on_event(SynthesisEvent::PhraseEnd);
        Ok(())
    }

    fn finalize(self: Box<Self>, _on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()> {
        Ok(())
    }
}

/// Sample count emitted per Audio event by [`TimedMockTts`]. Small
/// enough that on_committed_audio doesn't do meaningful work per
/// call; large enough that the consumer's audio plumbing accepts
/// each push without backpressure.
const TIMED_MOCK_SAMPLES_PER_CHUNK: usize = 64;
/// Sample rate of the timed-mock chunks. Matches [`MockStreamingTts`]
/// (Piper-class voice rate).
const TIMED_MOCK_SAMPLE_RATE: u32 = 22_050;
/// Wallclock delay between successive Audio events emitted by
/// [`TimedMockTts`]. The TTFA test relies on a real wallclock gap so
/// the consumer's per-event `Instant::now()` records show separation;
/// `std::thread::sleep` is correct here because `push_text` is
/// synchronous.
const TIMED_MOCK_INTER_CHUNK_MS: u64 = 50;

/// Streaming TTS mock that injects real wallclock delays between
/// Audio events, used to verify the consumer doesn't buffer chunks
/// before forwarding them to `on_committed_audio`.
///
/// Each non-empty `push_text` emits three Audio events at
/// [`TIMED_MOCK_INTER_CHUNK_MS`]-millisecond intervals (with sample
/// values 0.1, 0.2, 0.3 as identity markers), then `PhraseEnd`.
/// Total per-push wallclock ≈ 2 × interval = 100 ms. Empty input
/// emits no events (same shape as the production sessions).
///
/// **Why `std::thread::sleep` inside an `async`-ish call path is OK
/// here:** `SynthesisSession::push_text` is a synchronous trait
/// method by design (production backends do CPU-heavy ONNX
/// inference inline; see the trait's `# Blocking` note). The TTFA
/// test needs a *real* wallclock gap between Audio events so the
/// consumer's per-event `Instant::now()` records can show
/// separation — `tokio::time::sleep().await` would be inappropriate
/// because the trait isn't async, and `std::thread::yield_now()`
/// gives no measurable gap. Total wallclock cost per push is
/// ~100 ms, briefly blocking one tokio worker; tolerable in a unit
/// test mock.
pub struct TimedMockTts;

impl Named for TimedMockTts {
    fn name(&self) -> &str {
        "timed-mock-tts"
    }
}

impl StreamingTextToSpeech for TimedMockTts {
    fn sample_rate(&self) -> u32 {
        TIMED_MOCK_SAMPLE_RATE
    }
    fn open_session(&self, _voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>> {
        Ok(Box::new(TimedMockTtsSession))
    }
}

struct TimedMockTtsSession;

impl SynthesisSession for TimedMockTtsSession {
    fn push_text(&mut self, text: &str, on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        for (i, marker) in [0.1_f32, 0.2, 0.3].iter().enumerate() {
            on_event(SynthesisEvent::Audio(AudioChunk {
                samples: vec![*marker; TIMED_MOCK_SAMPLES_PER_CHUNK],
                sample_rate: TIMED_MOCK_SAMPLE_RATE,
            }));
            if i < 2 {
                std::thread::sleep(std::time::Duration::from_millis(TIMED_MOCK_INTER_CHUNK_MS));
            }
        }
        on_event(SynthesisEvent::PhraseEnd);
        Ok(())
    }

    fn finalize(self: Box<Self>, _on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()> {
        Ok(())
    }
}

/// Observer-event record used by the unit tests. The original
/// `speech_loop.rs` asserted side-effects via captured channels;
/// after the observer refactor each test inspects the recorded
/// event stream against the expected sequence. `TurnCompletePayload`
/// has no `PartialEq` so the enum uses pattern-matching assertions
/// instead of `==`.
///
/// `#[allow(dead_code)]` is applied because individual tests only
/// pattern-match a subset of fields; the unused-field analysis
/// fires across the whole enum.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum MockEvent {
    StateChange {
        state: VoiceState,
        hint: Option<String>,
    },
    Transcript(String),
    Chunk {
        primer_turn_index: usize,
        text: String,
    },
    Complete(TurnCompletePayload),
    InferenceError(String),
    Exit(ExitReason),
}

/// Test observer that records every callback into a shared `Vec`.
#[derive(Clone, Default)]
pub struct MockObserver(pub Arc<Mutex<Vec<MockEvent>>>);

impl MockObserver {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }
    pub fn events(&self) -> Vec<MockEvent> {
        self.0.lock().unwrap().clone()
    }
}

impl LoopObserver for MockObserver {
    fn on_state_change(&mut self, state: VoiceState, hint: Option<&str>) {
        self.0.lock().unwrap().push(MockEvent::StateChange {
            state,
            hint: hint.map(String::from),
        });
    }
    fn on_transcript_finalized(&mut self, text: &str) {
        self.0
            .lock()
            .unwrap()
            .push(MockEvent::Transcript(text.to_string()));
    }
    fn on_response_chunk(&mut self, primer_turn_index: usize, chunk: &str) {
        self.0.lock().unwrap().push(MockEvent::Chunk {
            primer_turn_index,
            text: chunk.to_string(),
        });
    }
    fn on_response_complete(&mut self, payload: TurnCompletePayload) {
        self.0.lock().unwrap().push(MockEvent::Complete(payload));
    }
    fn on_inference_error(&mut self, err: &primer_core::error::InferenceError) {
        self.0
            .lock()
            .unwrap()
            .push(MockEvent::InferenceError(format!("{err:?}")));
    }
    fn on_exit(&mut self, reason: ExitReason) {
        self.0.lock().unwrap().push(MockEvent::Exit(reason));
    }
}

#[test]
fn mock_streaming_stt_finalizes_canned_text() {
    let stt = MockStreamingStt::new("hello world");
    let session = stt.open_session().unwrap();
    let segs = session.finalize().unwrap();
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].text, "hello world");
}

#[test]
fn mock_streaming_tts_emits_one_chunk_per_text() {
    let tts = MockStreamingTts::new(100);
    let voice = VoiceProfile::default();
    let mut session = tts.open_session(&voice).unwrap();

    let mut count_non_empty: u32 = 0;
    session
        .push_text("hi.", &mut |e| {
            if let SynthesisEvent::Audio(_) = e {
                count_non_empty += 1;
            }
        })
        .unwrap();
    assert_eq!(count_non_empty, 1);

    let mut count_empty: u32 = 0;
    session.push_text("", &mut |_| count_empty += 1).unwrap();
    assert_eq!(count_empty, 0);
}

/// Pin the state machine's consumer-side guarantee that PCM events
/// reach `on_committed_audio` AS THEY ARRIVE — not buffered until
/// after `push_text` returns.
///
/// [`TimedMockTts`] emits three Audio events at 50 ms intervals
/// ([`TIMED_MOCK_INTER_CHUNK_MS`]). We record `Instant::now()` at
/// each `on_committed_audio` call and assert the FIRST 0.1-marker's
/// timestamp precedes the LAST 0.3-marker's timestamp by ≥80 ms (vs.
/// the 100 ms total inter-event budget — 20 ms slack absorbs CI
/// noise). A consumer that buffered all chunks before forwarding
/// would see all three timestamps clustered within microseconds and
/// fail this assertion.
#[tokio::test]
async fn streaming_chunks_reach_speaker_before_phrase_completes() {
    use std::sync::Mutex;
    use std::time::Instant;

    use primer_core::speech::VadEvent;

    /// Lower bound on the wallclock gap between the first and last
    /// committed Audio sample. Allows ~20 ms of slack on top of the
    /// nominal `2 × TIMED_MOCK_INTER_CHUNK_MS = 100 ms` budget.
    const STREAMING_GAP_FLOOR_MS: u64 = 80;
    /// Float-equality tolerance for matching the 0.1 / 0.3 identity
    /// markers in the committed sample stream. The markers are
    /// exact `f32` literals; the tolerance only guards against any
    /// future resampler that might be inserted between mock and
    /// consumer.
    const MARKER_EPS: f32 = 0.01;

    let backends = super::LoopBackends::single_locale(
        Arc::new(MockStreamingStt::new("hello primer")),
        Arc::new(TimedMockTts),
        primer_core::speech::VoiceProfile::default(),
        primer_core::i18n::Locale::English,
    );

    let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
    event_tx.try_send(VadEvent::SpeechStart).unwrap();
    event_tx.try_send(VadEvent::SpeechEnd).unwrap();
    drop(event_tx);

    /// `(wallclock, first-sample-value)` for each non-empty
    /// on_audio call. The first sample uniquely identifies which
    /// TimedMockTts marker (0.1 / 0.2 / 0.3) drove the commit.
    type TimelineEntry = (Instant, Option<f32>);

    // Record per non-empty on_audio call. Empty pushes (inter-phrase
    // silence frames) are filtered out so the markers' positions are
    // unambiguous.
    let timeline: Arc<Mutex<Vec<TimelineEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let timeline_cb = Arc::clone(&timeline);
    let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(move |samples| {
        if !samples.is_empty() {
            timeline_cb
                .lock()
                .unwrap()
                .push((Instant::now(), samples.first().copied()));
        }
    });

    struct EchoResponder;
    impl super::Responder for EchoResponder {
        fn respond<'a>(
            &'a mut self,
            transcript: &'a str,
            mut on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = primer_core::error::Result<String>> + Send + 'a>,
        > {
            let owned = transcript.to_string();
            Box::pin(async move {
                on_chunk(&owned);
                Ok(owned)
            })
        }
    }

    let observer = MockObserver::new();
    super::run_loop_borrowed(
        backends,
        event_rx,
        Box::new(EchoResponder),
        on_audio,
        None,
        false,
        None,
        observer.clone(),
    )
    .await
    .expect("loop runs to completion");

    let recorded = timeline.lock().unwrap();
    let first = recorded
        .iter()
        .find(|(_, v)| v.map(|x| (x - 0.1).abs() < MARKER_EPS).unwrap_or(false))
        .expect("0.1 marker was committed");
    let last = recorded
        .iter()
        .rfind(|(_, v)| v.map(|x| (x - 0.3).abs() < MARKER_EPS).unwrap_or(false))
        .expect("0.3 marker was committed");
    let gap = last.0.duration_since(first.0);
    assert!(
        gap >= std::time::Duration::from_millis(STREAMING_GAP_FLOOR_MS),
        "expected ≥{STREAMING_GAP_FLOOR_MS}ms between first and last Audio commit \
         (true streaming); got {gap:?}"
    );
}

#[test]
fn scripted_stt_drains_queue_then_returns_empty() {
    let stt = ScriptedStreamingStt::new(vec!["first", "second"]);
    let s1 = stt.open_session().unwrap().finalize().unwrap();
    assert_eq!(s1[0].text, "first");
    let s2 = stt.open_session().unwrap().finalize().unwrap();
    assert_eq!(s2[0].text, "second");
    // Queue exhausted: subsequent sessions finalize to the empty
    // string (the run-loop's empty-check handles this gracefully).
    let s3 = stt.open_session().unwrap().finalize().unwrap();
    assert_eq!(s3[0].text, "");
}

/// Test 1 — happy path: scripted SpeechEnd → LLM called with expected
/// transcript → audio chunks committed → run_loop returns transcripts.
#[tokio::test]
async fn happy_path_records_one_round_trip() {
    use std::sync::Mutex;

    use primer_core::speech::VadEvent;

    let backends = super::LoopBackends::single_locale(
        Arc::new(MockStreamingStt::new("hello primer")),
        Arc::new(MockStreamingTts::new(64)),
        primer_core::speech::VoiceProfile::default(),
        primer_core::i18n::Locale::English,
    );

    let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
    event_tx.try_send(VadEvent::SpeechStart).unwrap();
    event_tx.try_send(VadEvent::SpeechEnd).unwrap();
    drop(event_tx);

    let captured_transcript = Arc::new(Mutex::new(String::new()));
    let captured_clone = Arc::clone(&captured_transcript);
    struct ScriptedResponder {
        captured_transcript: Arc<Mutex<String>>,
    }
    impl super::Responder for ScriptedResponder {
        fn respond<'a>(
            &'a mut self,
            transcript: &'a str,
            mut on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = primer_core::error::Result<String>> + Send + 'a>,
        > {
            *self.captured_transcript.lock().unwrap() = transcript.to_string();
            Box::pin(async move {
                on_chunk("Hello, child.");
                Ok("Hello, child.".to_string())
            })
        }
    }
    let responder = Box::new(ScriptedResponder {
        captured_transcript: captured_clone,
    });

    let committed = Arc::new(Mutex::new(Vec::<f32>::new()));
    let committed_clone = Arc::clone(&committed);
    let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(move |samples| {
        committed_clone.lock().unwrap().extend(samples);
    });

    let observer = MockObserver::new();
    let result = super::run_loop_borrowed(
        backends,
        event_rx,
        responder,
        on_audio,
        None,
        false,
        None,
        observer.clone(),
    )
    .await;
    let transcripts = result.expect("loop ok");
    assert_eq!(transcripts, vec!["hello primer".to_string()]);
    assert_eq!(*captured_transcript.lock().unwrap(), "hello primer");
    assert!(!committed.lock().unwrap().is_empty(), "audio was committed");

    // Verify the observer saw a complete state journey: enter LISTEN,
    // finalize the transcript, transition through LATENT_THINK and
    // SPEAK, then back to LISTEN for the next utterance.
    let events = observer.events();
    assert!(
        events.iter().any(|e| matches!(
            e,
            MockEvent::StateChange {
                state: VoiceState::Listen,
                ..
            }
        )),
        "saw at least one Listen state: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, MockEvent::Transcript(t) if t == "hello primer")),
        "transcript finalized event: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            MockEvent::StateChange {
                state: VoiceState::Speak,
                ..
            }
        )),
        "entered SPEAK: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, MockEvent::Complete(_))),
        "fired Complete: {events:?}"
    );
}

/// Test 4 — quit phrase short-circuits SPEAK: child says "goodbye",
/// the responder returns an empty string. The loop pushes the
/// transcript, hits the quit-phrase branch, and exits before
/// reaching SPEAK — so no audio is committed regardless of the
/// (empty) responder output.
#[tokio::test]
async fn quit_phrase_short_circuits_speak() {
    use primer_core::speech::VadEvent;

    let backends = super::LoopBackends::single_locale(
        Arc::new(MockStreamingStt::new("goodbye")),
        Arc::new(MockStreamingTts::new(64)),
        primer_core::speech::VoiceProfile::default(),
        primer_core::i18n::Locale::English,
    );

    let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
    event_tx.try_send(VadEvent::SpeechStart).unwrap();
    event_tx.try_send(VadEvent::SpeechEnd).unwrap();
    drop(event_tx);

    struct EmptyResponder;
    impl super::Responder for EmptyResponder {
        fn respond<'a>(
            &'a mut self,
            _transcript: &'a str,
            mut on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = primer_core::error::Result<String>> + Send + 'a>,
        > {
            Box::pin(async move {
                on_chunk("");
                Ok(String::new())
            })
        }
    }

    let committed: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let committed_clone = Arc::clone(&committed);
    let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(move |samples| {
        committed_clone.lock().unwrap().extend(samples);
    });

    let observer = MockObserver::new();
    let result = super::run_loop_borrowed(
        backends,
        event_rx,
        Box::new(EmptyResponder),
        on_audio,
        None,
        false,
        None,
        observer.clone(),
    )
    .await;
    let transcripts = result.expect("loop ok");
    assert_eq!(transcripts, vec!["goodbye".to_string()]);
    assert!(
        committed.lock().unwrap().is_empty(),
        "quit phrase exits before SPEAK"
    );
    // Quit phrase fires Exit(Keyword) and never enters SPEAK.
    let events = observer.events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, MockEvent::Exit(ExitReason::Keyword))),
        "Exit(Keyword) fired: {events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            MockEvent::StateChange {
                state: VoiceState::Speak,
                ..
            }
        )),
        "did NOT enter SPEAK: {events:?}"
    );
}

/// Test 2 — cancel on resumed speech: SpeechEnd, then SpeechStart
/// before LLM completes. The LLM is cancelled. When the next
/// SpeechEnd arrives, the responder is called again with the
/// concatenated transcript. Audio commits on the second attempt.
#[tokio::test]
async fn cancel_on_resumed_speech_retries_after_continuation() {
    use primer_core::speech::VadEvent;

    // The MockStreamingStt always finalizes the SAME canned text. To
    // simulate "first attempt: 'why does'; second: 'why does the sky
    // look blue'", we need a smarter mock — but for the unit test
    // we accept that both attempts return the same canned text. The
    // assertion is about cancellation, not transcript stitching.
    let backends = super::LoopBackends::single_locale(
        Arc::new(MockStreamingStt::new("why does the sky look blue")),
        Arc::new(MockStreamingTts::new(64)),
        primer_core::speech::VoiceProfile::default(),
        primer_core::i18n::Locale::English,
    );

    let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
    // First SpeechStart → SpeechEnd: triggers LATENT_THINK.
    event_tx.try_send(VadEvent::SpeechStart).unwrap();
    event_tx.try_send(VadEvent::SpeechEnd).unwrap();
    // Then SpeechStart mid-LATENT_THINK: triggers cancel.
    event_tx.try_send(VadEvent::SpeechStart).unwrap();
    // Then SpeechEnd: retry LATENT_THINK.
    event_tx.try_send(VadEvent::SpeechEnd).unwrap();
    drop(event_tx);

    let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let cc_clone = Arc::clone(&call_count);
    // Cancel-drop counter — bumped only by the guard inside the
    // PARKING branch. The succeeding future never enters that
    // branch, so this counter discriminates "cancelled future was
    // dropped" from "any future was dropped." `== 1` is the exact
    // cancellation contract: the in-flight LLM call's destructor
    // runs, releasing resources.
    let cancel_drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let cd_clone = Arc::clone(&cancel_drops);
    struct CountingResponder {
        count: Arc<std::sync::atomic::AtomicUsize>,
        cancel_drops: Arc<std::sync::atomic::AtomicUsize>,
    }
    struct CancelGuard {
        drops: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl Drop for CancelGuard {
        fn drop(&mut self) {
            self.drops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    impl super::Responder for CountingResponder {
        fn respond<'a>(
            &'a mut self,
            _transcript: &'a str,
            mut on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = primer_core::error::Result<String>> + Send + 'a>,
        > {
            let n = self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let cancel_drops = Arc::clone(&self.cancel_drops);
            Box::pin(async move {
                if n == 0 {
                    // First call: park forever so the cancel arm wins.
                    // CancelGuard scoped to this branch only — its
                    // Drop only runs when the parking future itself
                    // is dropped (i.e. cancelled).
                    let _guard = CancelGuard {
                        drops: cancel_drops,
                    };
                    std::future::pending::<()>().await;
                    unreachable!()
                }
                // Second call: respond promptly. No CancelGuard
                // here — only the cancellation path counts.
                on_chunk("Because of Rayleigh scattering.");
                Ok("Because of Rayleigh scattering.".to_string())
            })
        }
    }

    let committed: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let committed_clone = Arc::clone(&committed);
    let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(move |samples| {
        committed_clone.lock().unwrap().extend(samples);
    });

    let observer = MockObserver::new();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        super::run_loop_borrowed(
            backends,
            event_rx,
            Box::new(CountingResponder {
                count: cc_clone,
                cancel_drops: cd_clone,
            }),
            on_audio,
            None,
            false,
            None,
            observer.clone(),
        ),
    )
    .await
    .expect("did not deadlock")
    .expect("loop ok");

    // run_loop pushes one transcript per outer-loop iteration. Cancel-and-retry
    // is internal to one iteration. So we expect exactly one transcript.
    assert_eq!(result.len(), 1, "one commit cycle, one transcript");
    // Responder was called twice (first cancelled, second succeeded).
    assert_eq!(
        call_count.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "responder called twice"
    );
    // The cancelled (parking) future MUST have its destructor run —
    // exactly once — that's the cancellation guarantee. CancelGuard
    // is scoped to the parking branch only; the succeeding future
    // doesn't construct one, so this asserts the leak-vs-drop
    // contract precisely.
    assert_eq!(
        cancel_drops.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "cancelled LLM future was dropped exactly once (not leaked)"
    );
    // Audio committed (from second responder call).
    assert!(
        !committed.lock().unwrap().is_empty(),
        "audio committed on retry"
    );
    // The cancel-by-VAD path fires a Listen state_change with the
    // "child_resumed" hint. That's the observable contract for
    // GUIs/CLIs that want to surface the cancellation.
    let events = observer.events();
    assert!(
        events.iter().any(|e| matches!(
            e,
            MockEvent::StateChange {
                state: VoiceState::Listen,
                hint: Some(h),
            } if h == "child_resumed"
        )),
        "observer saw Listen state with child_resumed hint: {events:?}"
    );
}

/// Regression test for issue #103 — cancel-and-retry must stitch the
/// transcript across the two STT sessions rather than discarding the
/// pre-cancel half. Drives the same SpeechEnd → SpeechStart-cancel →
/// SpeechEnd flow as the test above but with a scripted STT whose
/// successive sessions return *different* partial transcripts. The
/// final transcript surfaced to the observer (and to the LLM on
/// retry) must be the concatenation of both halves.
#[tokio::test]
async fn cancel_and_retry_stitches_full_transcript() {
    use primer_core::speech::VadEvent;

    // Session #0 (opened during LISTEN) finalizes to "why does".
    // Session #1 (opened at the end of the latent-think iter 0)
    // finalizes to "the sky look blue". Session #2 (opened at the
    // end of iter 1) is unused; the queue is sized exactly so an
    // accidental third finalize would surface as an empty string.
    let backends = super::LoopBackends::single_locale(
        Arc::new(ScriptedStreamingStt::new(vec![
            "why does",
            "the sky look blue",
        ])),
        Arc::new(MockStreamingTts::new(64)),
        primer_core::speech::VoiceProfile::default(),
        primer_core::i18n::Locale::English,
    );

    let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
    // First SpeechStart → SpeechEnd: triggers LATENT_THINK (finalize
    // session #0 → "why does").
    event_tx.try_send(VadEvent::SpeechStart).unwrap();
    event_tx.try_send(VadEvent::SpeechEnd).unwrap();
    // SpeechStart mid-LATENT_THINK: cancels the LLM.
    event_tx.try_send(VadEvent::SpeechStart).unwrap();
    // Then SpeechEnd: retries LATENT_THINK (finalize session #1 →
    // "the sky look blue"). The accumulated transcript at this point
    // must be "why does the sky look blue".
    event_tx.try_send(VadEvent::SpeechEnd).unwrap();
    drop(event_tx);

    // Capture the transcript the responder sees on each call so the
    // assertions can pin "second call saw the full stitched text"
    // rather than guessing from the final result alone.
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = Arc::clone(&captured);
    struct CapturingResponder {
        captured: Arc<Mutex<Vec<String>>>,
        count: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl super::Responder for CapturingResponder {
        fn respond<'a>(
            &'a mut self,
            transcript: &'a str,
            mut on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = primer_core::error::Result<String>> + Send + 'a>,
        > {
            self.captured.lock().unwrap().push(transcript.to_string());
            let n = self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async move {
                if n == 0 {
                    // First call: park forever so the cancel arm wins.
                    std::future::pending::<()>().await;
                    unreachable!()
                }
                on_chunk("Because of Rayleigh scattering.");
                Ok("Because of Rayleigh scattering.".to_string())
            })
        }
    }
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let responder = Box::new(CapturingResponder {
        captured: captured_clone,
        count: Arc::clone(&count),
    });

    let committed: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let committed_clone = Arc::clone(&committed);
    let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(move |samples| {
        committed_clone.lock().unwrap().extend(samples);
    });

    let observer = MockObserver::new();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        super::run_loop_borrowed(
            backends,
            event_rx,
            responder,
            on_audio,
            None,
            false,
            None,
            observer.clone(),
        ),
    )
    .await
    .expect("did not deadlock")
    .expect("loop ok");

    // Exactly one turn (cancel-and-retry stays inside a single outer
    // iteration) and the surfaced transcript is the full stitched
    // utterance — the headline assertion of issue #103.
    assert_eq!(
        result,
        vec!["why does the sky look blue".to_string()],
        "transcript stitched across both STT sessions"
    );

    // Responder was called twice; the second call saw the full
    // stitched text. Before the fix, the second call received only
    // "the sky look blue" and the test would fail here.
    let calls = captured.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        2,
        "responder called once before cancel, once on retry"
    );
    assert_eq!(
        calls[1], "why does the sky look blue",
        "retry LLM call received the full stitched transcript, not the tail"
    );

    // Observer saw the full stitched transcript via
    // `on_transcript_finalized` — the bubble-emission path that the
    // issue called out as user-visibly broken before the fix.
    let events = observer.events();
    assert!(
        events.iter().any(|e| matches!(
            e,
            MockEvent::Transcript(t) if t == "why does the sky look blue"
        )),
        "observer received the full stitched transcript: {events:?}"
    );

    // Sanity: audio reaches the speaker on the second attempt.
    assert!(
        !committed.lock().unwrap().is_empty(),
        "audio committed on retry"
    );
}

/// Defends the `!new_text.is_empty()` gate inside the LATENT_THINK
/// accumulator. If a future refactor removed the gate, an iteration
/// that finalizes to empty would append a phantom trailing space to
/// the running transcript, and a subsequent non-empty finalize would
/// surface as `"first  second"` (double space) rather than the
/// correct `"first second"`. Drives three STT sessions —
/// `["first", "", "second"]` — across two cancel-and-retry cycles
/// and asserts the final stitched transcript has the right shape.
#[tokio::test]
async fn cancel_and_retry_skips_empty_finalize_no_phantom_space() {
    use primer_core::speech::VadEvent;

    let backends = super::LoopBackends::single_locale(
        Arc::new(ScriptedStreamingStt::new(vec!["first", "", "second"])),
        Arc::new(MockStreamingTts::new(64)),
        primer_core::speech::VoiceProfile::default(),
        primer_core::i18n::Locale::English,
    );

    let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
    // Iter 0: SpeechStart → SpeechEnd → finalize "first" → LLM call 0.
    event_tx.try_send(VadEvent::SpeechStart).unwrap();
    event_tx.try_send(VadEvent::SpeechEnd).unwrap();
    // Cancel call 0; iter 1: SpeechEnd → finalize "" → LLM call 1
    // (with the carried-over "first").
    event_tx.try_send(VadEvent::SpeechStart).unwrap();
    event_tx.try_send(VadEvent::SpeechEnd).unwrap();
    // Cancel call 1; iter 2: SpeechEnd → finalize "second" → LLM
    // call 2 (with the stitched "first second", a single space).
    event_tx.try_send(VadEvent::SpeechStart).unwrap();
    event_tx.try_send(VadEvent::SpeechEnd).unwrap();
    drop(event_tx);

    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = Arc::clone(&captured);
    struct CapturingResponder {
        captured: Arc<Mutex<Vec<String>>>,
        count: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl super::Responder for CapturingResponder {
        fn respond<'a>(
            &'a mut self,
            transcript: &'a str,
            mut on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = primer_core::error::Result<String>> + Send + 'a>,
        > {
            self.captured.lock().unwrap().push(transcript.to_string());
            let n = self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async move {
                if n < 2 {
                    // First two calls park so the cancel arms win.
                    std::future::pending::<()>().await;
                    unreachable!()
                }
                on_chunk("ok.");
                Ok("ok.".to_string())
            })
        }
    }
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let responder = Box::new(CapturingResponder {
        captured: captured_clone,
        count: Arc::clone(&count),
    });

    let committed: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let committed_clone = Arc::clone(&committed);
    let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(move |samples| {
        committed_clone.lock().unwrap().extend(samples);
    });

    let observer = MockObserver::new();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        super::run_loop_borrowed(
            backends,
            event_rx,
            responder,
            on_audio,
            None,
            false,
            None,
            observer.clone(),
        ),
    )
    .await
    .expect("did not deadlock")
    .expect("loop ok");

    // The final transcript is "first second" with a single separating
    // space. With the empty-gate removed, this would be "first  second"
    // (double space) because the empty middle finalize would append a
    // phantom trailing space onto "first" before "second" is appended.
    assert_eq!(
        result,
        vec!["first second".to_string()],
        "empty middle finalize must not introduce a phantom space"
    );

    // The responder saw three calls; the third saw the stitched text
    // with a single space. The second call saw the carried-over
    // "first" alone (no trailing space) — the strongest direct check
    // on the empty-text gate.
    let calls = captured.lock().unwrap().clone();
    assert_eq!(calls.len(), 3, "three LLM attempts (two cancelled, one ok)");
    assert_eq!(
        calls[1], "first",
        "empty iteration must not add a trailing space to the carried transcript"
    );
    assert_eq!(
        calls[2], "first second",
        "third call sees the stitched transcript with a single space"
    );
}

/// Test 3 — commit on first audio: synthesis fires before any
/// resumed speech. Audio reaches the speaker callback; subsequent
/// VAD events arriving after commit do not affect the in-flight
/// SPEAK phase.
#[tokio::test]
async fn commit_on_first_chunk_proceeds_to_speak() {
    use primer_core::speech::VadEvent;

    let backends = super::LoopBackends::single_locale(
        Arc::new(MockStreamingStt::new("hi primer")),
        Arc::new(MockStreamingTts::new(64)),
        primer_core::speech::VoiceProfile::default(),
        primer_core::i18n::Locale::English,
    );

    let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
    event_tx.try_send(VadEvent::SpeechStart).unwrap();
    event_tx.try_send(VadEvent::SpeechEnd).unwrap();
    // Crucially: NO SpeechStart between SpeechEnd and the LLM future
    // resolving. Commit should proceed.
    drop(event_tx);

    struct PromptResponder;
    impl super::Responder for PromptResponder {
        fn respond<'a>(
            &'a mut self,
            _transcript: &'a str,
            mut on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = primer_core::error::Result<String>> + Send + 'a>,
        > {
            Box::pin(async move {
                on_chunk("Hello!");
                Ok("Hello!".to_string())
            })
        }
    }

    let committed: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let committed_clone = Arc::clone(&committed);
    let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(move |samples| {
        committed_clone.lock().unwrap().extend(samples);
    });

    let observer = MockObserver::new();
    let result = super::run_loop_borrowed(
        backends,
        event_rx,
        Box::new(PromptResponder),
        on_audio,
        None,
        false,
        None,
        observer.clone(),
    )
    .await
    .expect("loop ok");

    assert_eq!(result, vec!["hi primer".to_string()]);
    assert!(!committed.lock().unwrap().is_empty(), "audio committed");
}

/// Test 5 — is_speaking gate observed during SPEAK: when the loop
/// is given an `Arc<AtomicBool>` gate, the on_audio callback fires
/// while the gate is set true (proving the audio thread would discard
/// mic samples), and the gate clears to false before run_loop returns.
#[tokio::test]
async fn is_speaking_gate_flips_around_speak() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use primer_core::speech::VadEvent;

    let backends = super::LoopBackends::single_locale(
        Arc::new(MockStreamingStt::new("hi")),
        Arc::new(MockStreamingTts::new(64)),
        primer_core::speech::VoiceProfile::default(),
        primer_core::i18n::Locale::English,
    );

    let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
    event_tx.try_send(VadEvent::SpeechStart).unwrap();
    event_tx.try_send(VadEvent::SpeechEnd).unwrap();
    drop(event_tx);

    struct PromptResponder;
    impl super::Responder for PromptResponder {
        fn respond<'a>(
            &'a mut self,
            _transcript: &'a str,
            _on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = primer_core::error::Result<String>> + Send + 'a>,
        > {
            Box::pin(async move { Ok("Hi back.".to_string()) })
        }
    }

    let is_speaking = Arc::new(AtomicBool::new(false));
    let observed_true = Arc::new(AtomicBool::new(false));
    let observed_clone = Arc::clone(&observed_true);
    let speaking_for_cb = Arc::clone(&is_speaking);
    let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(move |_samples| {
        if speaking_for_cb.load(Ordering::SeqCst) {
            observed_clone.store(true, Ordering::SeqCst);
        }
    });

    // 2 s timeout: the mock on_audio doesn't poll for drain so the
    // gate clears as soon as the synth chunks have been "consumed"
    // (a few ms). Generous cap, kept high to surface deadlocks
    // rather than match expected runtime.
    let observer = MockObserver::new();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        super::run_loop_borrowed(
            backends,
            event_rx,
            Box::new(PromptResponder),
            on_audio,
            None,
            false,
            Some(Arc::clone(&is_speaking)),
            observer.clone(),
        ),
    )
    .await
    .expect("did not deadlock")
    .expect("loop ok");

    assert_eq!(result, vec!["hi".to_string()]);
    assert!(
        observed_true.load(Ordering::SeqCst),
        "is_speaking flag was true during SPEAK audio commit"
    );
    assert!(
        !is_speaking.load(Ordering::SeqCst),
        "is_speaking flag is cleared after SPEAK"
    );
}

/// Test 6 — LLM error fallback: when the responder returns Err,
/// run_loop synthesises FALLBACK_LINE rather than propagating the
/// error. Asserts the loop returns Ok(child transcript) and that
/// the TTS received exactly the FALLBACK_LINE string (proving the
/// child hears the apology).
#[tokio::test]
async fn llm_error_synthesises_fallback_line() {
    use std::sync::Mutex;

    use primer_core::speech::{
        AudioChunk, Named, StreamingTextToSpeech, SynthesisEvent, SynthesisSession, VadEvent,
        VoiceProfile,
    };

    // TTS that records every text fed to its session.
    struct CapturingTts {
        captured: Arc<Mutex<Vec<String>>>,
    }
    impl Named for CapturingTts {
        fn name(&self) -> &str {
            "capturing-tts"
        }
    }
    impl StreamingTextToSpeech for CapturingTts {
        fn sample_rate(&self) -> u32 {
            22_050
        }
        fn open_session(
            &self,
            _voice: &VoiceProfile,
        ) -> primer_core::error::Result<Box<dyn SynthesisSession>> {
            Ok(Box::new(CapturingSession {
                captured: Arc::clone(&self.captured),
            }))
        }
    }
    struct CapturingSession {
        captured: Arc<Mutex<Vec<String>>>,
    }
    impl SynthesisSession for CapturingSession {
        fn push_text(
            &mut self,
            text: &str,
            on_event: &mut dyn FnMut(SynthesisEvent),
        ) -> primer_core::error::Result<()> {
            if text.is_empty() {
                return Ok(());
            }
            self.captured.lock().unwrap().push(text.to_string());
            on_event(SynthesisEvent::Audio(AudioChunk {
                samples: vec![0.5; 64],
                sample_rate: 22_050,
            }));
            on_event(SynthesisEvent::PhraseEnd);
            Ok(())
        }

        fn finalize(
            self: Box<Self>,
            _on_event: &mut dyn FnMut(SynthesisEvent),
        ) -> primer_core::error::Result<()> {
            Ok(())
        }
    }

    let captured = Arc::new(Mutex::new(Vec::<String>::new()));
    let backends = super::LoopBackends::single_locale(
        Arc::new(MockStreamingStt::new("hello primer")),
        Arc::new(CapturingTts {
            captured: Arc::clone(&captured),
        }),
        VoiceProfile::default(),
        primer_core::i18n::Locale::English,
    );

    let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
    event_tx.try_send(VadEvent::SpeechStart).unwrap();
    event_tx.try_send(VadEvent::SpeechEnd).unwrap();
    drop(event_tx);

    struct ErrResponder;
    impl super::Responder for ErrResponder {
        fn respond<'a>(
            &'a mut self,
            _transcript: &'a str,
            _on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = primer_core::error::Result<String>> + Send + 'a>,
        > {
            Box::pin(async move {
                Err(primer_core::error::PrimerError::Inference(
                    "rate limit".into(),
                ))
            })
        }
    }

    let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(|_samples| {});
    let observer = MockObserver::new();
    let result = super::run_loop_borrowed(
        backends,
        event_rx,
        Box::new(ErrResponder),
        on_audio,
        None,
        false,
        None,
        observer.clone(),
    )
    .await
    .expect("loop returns Ok despite responder error");

    assert_eq!(result, vec!["hello primer".to_string()]);
    let texts = captured.lock().unwrap();
    assert_eq!(texts.len(), 1, "TTS got exactly one push_text call");
    assert_eq!(
        texts[0],
        super::FALLBACK_LINE,
        "fallback line was synthesised after LLM error"
    );
    // Observer surfaces the inference error so a GUI banner can fire.
    let events = observer.events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, MockEvent::InferenceError(_))),
        "InferenceError observed: {events:?}"
    );
}

#[test]
fn ensure_active_locale_coverage_ok_after_single_locale_constructor() {
    let backends = super::LoopBackends::single_locale(
        Arc::new(MockStreamingStt::new("")),
        Arc::new(MockStreamingTts::new(64)),
        primer_core::speech::VoiceProfile::default(),
        primer_core::i18n::Locale::German,
    );
    backends
        .ensure_active_locale_coverage()
        .expect("single_locale must satisfy the coverage invariant");
}

#[test]
fn ensure_active_locale_coverage_errors_when_tts_missing() {
    // Hand-roll the maps to simulate a future caller that builds
    // them directly (e.g. from a voice-pack scan) and forgot to
    // include the active locale's voice. v1's `single_locale`
    // can't reach this state.
    let backends = super::LoopBackends {
        stt: Arc::new(MockStreamingStt::new("")),
        tts_by_locale: std::collections::HashMap::new(),
        voice_by_locale: std::collections::HashMap::new(),
        active_locale: primer_core::i18n::Locale::German,
    };
    let err = backends.ensure_active_locale_coverage().unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("'de'"),
        "must name the missing locale by pack id: {msg}"
    );
    assert!(
        msg.contains("--voice-onnx"),
        "must point the user at the corrective flags: {msg}"
    );
    assert!(
        msg.contains("piper-voices"),
        "must point the user at where to find a voice: {msg}"
    );
}

#[test]
fn ensure_active_locale_coverage_errors_when_only_voice_missing() {
    let mut tts_by_locale: std::collections::HashMap<
        primer_core::i18n::Locale,
        Arc<dyn primer_core::speech::StreamingTextToSpeech>,
    > = std::collections::HashMap::new();
    tts_by_locale.insert(
        primer_core::i18n::Locale::English,
        Arc::new(MockStreamingTts::new(64)),
    );
    let backends = super::LoopBackends {
        stt: Arc::new(MockStreamingStt::new("")),
        tts_by_locale,
        voice_by_locale: std::collections::HashMap::new(),
        active_locale: primer_core::i18n::Locale::English,
    };
    let err = backends.ensure_active_locale_coverage().unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("voice profile"),
        "must distinguish voice-profile miss from TTS miss: {msg}"
    );
    assert!(msg.contains("'en'"), "must name the missing locale: {msg}");
}
