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
    use std::sync::Mutex;

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
