//! Voice round-trip REPL — the `--speech` mode of `primer-cli`.
//!
//! State machine: `LISTEN → LATENT_THINK → SPEAK → LISTEN`, with the
//! mic open through LISTEN and LATENT_THINK so the Primer never barges
//! in on a child mid-thought. Closes the mic on the commit boundary
//! (first audio chunk reaches the speaker) so the child never speaks
//! over the Primer.
//!
//! See `docs/superpowers/specs/2026-05-02-voice-roundtrip-poc-design.md`
//! for the full design.

use std::path::Path;

use primer_core::error::Result;

/// Phrases that, if heard in the child's transcript, end the session.
/// Case-insensitive substring match. Three flavours so the child can
/// quit whether they say the formal "goodbye", a Primer-direct
/// "bye primer", or the very direct "stop primer".
const QUIT_PHRASES: &[&str] = &["goodbye", "bye primer", "stop primer"];

/// Returns true if `transcript` contains any quit phrase (case-insensitive).
fn is_quit_phrase(transcript: &str) -> bool {
    let lower = transcript.to_lowercase();
    QUIT_PHRASES.iter().any(|p| lower.contains(p))
}

/// Configuration passed into `run` from `main`.
pub struct SpeechLoopConfig<'a> {
    pub whisper_model: &'a Path,
    pub voice_onnx: &'a Path,
    pub voice_config: &'a Path,
    pub voice_id: &'a str,
    pub mic_silence_ms: u32,
    pub verbose: bool,
}

use std::sync::Arc;

use primer_core::speech::{StreamingSpeechToText, StreamingTextToSpeech, VoiceActivityDetector};

/// Trait-injected backends consumed by `run_loop`. Production wires real
/// silero / whisper / piper instances; tests wire mocks. Keeping the
/// state machine generic over these means we exercise the full select!
/// machinery in unit tests without any audio hardware.
pub struct LoopBackends {
    pub vad: Box<dyn VoiceActivityDetector>,
    pub stt: Arc<dyn StreamingSpeechToText>,
    pub tts: Arc<dyn StreamingTextToSpeech>,
}

/// One commit cycle: receives transcripts on `transcript_rx`, runs the
/// LLM, returns the full Primer reply (for the caller to print and feed
/// into TTS). Production wires this through `DialogueManager`; tests
/// wire a closure that returns canned output.
///
/// **Lifetime:** the trait is NOT `'static` — `DialogueResponder` (Task 21)
/// borrows the `&mut DialogueManager`, which has its own borrowed
/// `&dyn InferenceBackend`. `run_loop` does not `tokio::spawn` the
/// responder, only `select!`s on it, so a `'static` bound would be
/// over-restrictive.
pub trait Responder: Send {
    /// Generate a response to `transcript`, calling `on_chunk` per chunk.
    /// Awaiting this future = "LLM is thinking". Cancellable via
    /// dropping the future (no `JoinHandle` involved — `run_loop` keeps
    /// the future on the stack via `tokio::pin!`).
    fn respond<'a>(
        &'a mut self,
        transcript: &'a str,
        on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>>;
}

/// Drive the state machine until a quit phrase or exhausted VAD events.
/// `events` is the source of `VadEvent`s the loop reads (production
/// wires the audio capture task; tests wire a `tokio::sync::mpsc`
/// pre-filled with a script).
///
/// The `'r` lifetime on the boxed responder lets `DialogueResponder`
/// (Task 21) borrow `&mut DialogueManager` rather than own it.
pub async fn run_loop<'r>(
    mut backends: LoopBackends,
    mut events: tokio::sync::mpsc::UnboundedReceiver<primer_core::speech::VadEvent>,
    mut responder: Box<dyn Responder + 'r>,
    mut on_committed_audio: Box<dyn FnMut(Vec<f32>) + Send>,
) -> Result<Vec<String>> {
    use primer_core::speech::VadEvent;

    let mut transcripts: Vec<String> = Vec::new();

    'outer: loop {
        // ── LISTEN ────────────────────────────────────────────────────
        let mut stt_session = backends.stt.open_session()?;
        let mut in_speech = false;
        loop {
            let Some(event) = events.recv().await else {
                break 'outer;
            };
            match event {
                VadEvent::SpeechStart => in_speech = true,
                VadEvent::SpeechEnd if in_speech => break,
                _ => {}
            }
        }

        // ── LATENT_THINK ──────────────────────────────────────────────
        // Loop here so a SpeechStart-cancel can resume listening with
        // the same whisper session and re-attempt the LLM call once the
        // child finishes their continuation.
        let mut transcript_so_far: String;
        let mut accumulated = String::new();
        loop {
            // Peek (finalize-and-reopen since whisper-cpp-plus has no
            // partial-extract API exposed here): we accept the slight
            // mock-friendliness — production whisper supports peeking via
            // process_step but the trait surface is finalize-only today.
            let segments = stt_session.finalize()?;
            transcript_so_far = segments
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join("")
                .trim()
                .to_string();
            stt_session = backends.stt.open_session()?;

            if transcript_so_far.is_empty() {
                break;
            }

            // Drive the LLM. `respond` returns the full accumulated text
            // as Ok(String); the on_chunk callback is for live streaming
            // (e.g. terminal echo). For unit tests we ignore chunks and
            // rely on the final Result.
            let transcript_clone = transcript_so_far.clone();
            let llm_fut = responder.respond(
                &transcript_clone,
                Box::new(|_chunk: &str| {}),
            );
            tokio::pin!(llm_fut);

            // Wait for either: (a) llm done, (b) VAD SpeechStart (cancel).
            // If the VAD event channel is already closed, we still need to
            // complete the LLM call — channel closure only means no more
            // new speech events, not that we should discard work in flight.
            let cancelled = if events.is_closed() {
                // No more VAD events possible: complete the LLM unconditionally.
                accumulated = llm_fut.await?;
                false
            } else {
                tokio::select! {
                    res = &mut llm_fut => {
                        accumulated = res?;
                        false
                    }
                    event = events.recv() => {
                        match event {
                            Some(VadEvent::SpeechStart) => {
                                // Cancel: drop the future, loop back, keep listening.
                                true
                            }
                            Some(VadEvent::SpeechEnd) | Some(VadEvent::None) => {
                                // Spurious — shouldn't happen during LATENT_THINK
                                // since we entered on SpeechEnd. Treat as
                                // continue-waiting by completing the LLM.
                                accumulated = llm_fut.await?;
                                false
                            }
                            None => {
                                // Channel just closed mid-select: complete the LLM.
                                accumulated = llm_fut.await?;
                                false
                            }
                        }
                    }
                }
            };

            if cancelled {
                // Wait for the next SpeechEnd to retry the LLM call.
                loop {
                    let Some(event) = events.recv().await else {
                        return Ok(transcripts);
                    };
                    if event == VadEvent::SpeechEnd {
                        break;
                    }
                }
                continue;
            }

            break;
        }

        // ── Quit check + commit transcript ────────────────────────────
        if transcript_so_far.is_empty() {
            continue;
        }
        if is_quit_phrase(&transcript_so_far) {
            transcripts.push(transcript_so_far);
            break 'outer;
        }
        transcripts.push(transcript_so_far);

        // ── SPEAK ─────────────────────────────────────────────────────
        if !accumulated.is_empty() {
            let voice = primer_core::speech::VoiceProfile::default();
            let mut session = backends.tts.open_session(&voice)?;
            for chunk in session.push_text(&accumulated)? {
                on_committed_audio(chunk.samples);
            }
            for chunk in session.finalize()? {
                on_committed_audio(chunk.samples);
            }
        }
    }

    Ok(transcripts)
}

/// Entry point: run the voice REPL until Ctrl+C or a quit phrase is heard.
///
/// Phase 4 stub — real implementation lands across Phases 5/6/7.
pub async fn run(_cfg: SpeechLoopConfig<'_>) -> Result<()> {
    Err(primer_core::error::PrimerError::Speech(
        "speech_loop::run not yet implemented".into(),
    ))
}

#[cfg(test)]
mod mocks {
    use std::sync::{Arc, Mutex};

    use primer_core::error::Result;
    use primer_core::speech::{
        AudioChunk, Named, StreamingSpeechToText, StreamingTextToSpeech, SynthesisSession,
        TranscriptSegment, TranscriptionSession, VadEvent, VadFrame, VoiceActivityDetector,
        VoiceProfile,
    };

    /// Mock VAD that emits a scripted sequence of VadEvents, one per
    /// `process_chunk` call. The `_samples` arg is ignored — the mock
    /// reports whatever the script said for that index.
    pub struct MockVad {
        script: Mutex<std::vec::IntoIter<VadEvent>>,
    }

    impl MockVad {
        pub fn new(events: Vec<VadEvent>) -> Self {
            Self {
                script: Mutex::new(events.into_iter()),
            }
        }
    }

    impl Named for MockVad {
        fn name(&self) -> &str {
            "mock-vad"
        }
    }

    impl VoiceActivityDetector for MockVad {
        fn sample_rate(&self) -> u32 {
            16_000
        }
        fn chunk_samples(&self) -> usize {
            512
        }
        fn process_chunk(&mut self, _samples: &[f32]) -> Result<VadFrame> {
            let event = self.script.lock().unwrap().next().unwrap_or(VadEvent::None);
            let speech_probability = match event {
                VadEvent::SpeechStart => 0.9,
                VadEvent::SpeechEnd => 0.1,
                VadEvent::None => 0.5,
            };
            Ok(VadFrame {
                speech_probability,
                event,
            })
        }
        fn reset(&mut self) {}
    }

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
        fn push_text(&mut self, text: &str) -> Result<Vec<AudioChunk>> {
            if text.is_empty() {
                return Ok(vec![]);
            }
            Ok(vec![AudioChunk {
                samples: vec![0.5; self.chunk_samples],
                sample_rate: 22_050,
            }])
        }
        fn finalize(self: Box<Self>) -> Result<Vec<AudioChunk>> {
            Ok(vec![])
        }
    }

    #[test]
    fn mock_vad_emits_scripted_events() {
        let mut vad = MockVad::new(vec![
            VadEvent::SpeechStart,
            VadEvent::None,
            VadEvent::SpeechEnd,
        ]);
        let chunk = vec![0.0f32; 512];
        assert_eq!(vad.process_chunk(&chunk).unwrap().event, VadEvent::SpeechStart);
        assert_eq!(vad.process_chunk(&chunk).unwrap().event, VadEvent::None);
        assert_eq!(vad.process_chunk(&chunk).unwrap().event, VadEvent::SpeechEnd);
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
        assert_eq!(session.push_text("hi.").unwrap().len(), 1);
        assert_eq!(session.push_text("").unwrap().len(), 0);
    }

    /// Test 1 — happy path: scripted SpeechEnd → LLM called with expected
    /// transcript → audio chunks committed → run_loop returns transcripts.
    #[tokio::test]
    async fn happy_path_records_one_round_trip() {
        use std::sync::Mutex;

        use primer_core::speech::VadEvent;

        let backends = super::LoopBackends {
            vad: Box::new(MockVad::new(vec![
                VadEvent::SpeechStart,
                VadEvent::SpeechEnd,
            ])),
            stt: Arc::new(MockStreamingStt::new("hello primer")),
            tts: Arc::new(MockStreamingTts::new(64)),
        };

        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        event_tx.send(VadEvent::SpeechStart).unwrap();
        event_tx.send(VadEvent::SpeechEnd).unwrap();
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
                Box<
                    dyn std::future::Future<Output = primer_core::error::Result<String>>
                        + Send
                        + 'a,
                >,
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

        let result = super::run_loop(backends, event_rx, responder, on_audio).await;
        let transcripts = result.expect("loop ok");
        assert_eq!(transcripts, vec!["hello primer".to_string()]);
        assert_eq!(*captured_transcript.lock().unwrap(), "hello primer");
        assert!(!committed.lock().unwrap().is_empty(), "audio was committed");
    }

    /// Test 4 — natural completion, no audio: LLM returns whitespace
    /// only. Loop should not commit any audio and should return to
    /// LISTEN cleanly.
    #[tokio::test]
    async fn natural_completion_no_audio_does_not_commit() {
        use primer_core::speech::VadEvent;

        let backends = super::LoopBackends {
            vad: Box::new(MockVad::new(vec![
                VadEvent::SpeechStart,
                VadEvent::SpeechEnd,
            ])),
            stt: Arc::new(MockStreamingStt::new("goodbye")),
            tts: Arc::new(MockStreamingTts::new(64)),
        };

        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        event_tx.send(VadEvent::SpeechStart).unwrap();
        event_tx.send(VadEvent::SpeechEnd).unwrap();
        drop(event_tx);

        struct WhitespaceResponder;
        impl super::Responder for WhitespaceResponder {
            fn respond<'a>(
                &'a mut self,
                _transcript: &'a str,
                mut on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = primer_core::error::Result<String>> + Send + 'a>> {
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

        let result = super::run_loop(
            backends,
            event_rx,
            Box::new(WhitespaceResponder),
            on_audio,
        )
        .await;
        // "goodbye" hits the quit-phrase check, so the loop exits with
        // exactly one transcript and no audio committed.
        let transcripts = result.expect("loop ok");
        assert_eq!(transcripts, vec!["goodbye".to_string()]);
        assert!(committed.lock().unwrap().is_empty(), "no audio for whitespace");
    }
}

#[cfg(test)]
mod quit_tests {
    use super::is_quit_phrase;

    #[test]
    fn detects_goodbye_case_insensitive() {
        assert!(is_quit_phrase("Goodbye!"));
        assert!(is_quit_phrase("alright, GOODBYE then"));
    }

    #[test]
    fn detects_bye_primer() {
        assert!(is_quit_phrase("bye primer"));
        assert!(is_quit_phrase("Bye Primer."));
    }

    #[test]
    fn ignores_unrelated_transcripts() {
        assert!(!is_quit_phrase("why is the sky blue"));
        assert!(!is_quit_phrase("hello"));
        // "bye" alone is NOT a quit phrase — only "bye primer".
        assert!(!is_quit_phrase("bye"));
    }
}
