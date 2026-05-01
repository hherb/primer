//! Speech traits — VAD, STT, and TTS abstractions.
//!
//! VAD implementations: Silero (via silero-vad-rust).
//! STT implementations: whisper.cpp (via whisper-rs), platform-native APIs.
//! TTS implementations: Piper, platform-native APIs.

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
pub trait VoiceActivityDetector: Send {
    fn name(&self) -> &str;

    /// Sample rate the detector expects (Hz).
    fn sample_rate(&self) -> u32;

    /// Number of f32 samples per chunk fed to [`Self::process_chunk`].
    fn chunk_samples(&self) -> usize;

    /// Process exactly [`Self::chunk_samples`] mono f32 samples.
    fn process_chunk(&mut self, samples: &[f32]) -> Result<VadFrame>;

    /// Reset internal state (between utterances or sessions).
    fn reset(&mut self);
}

/// Speech-to-text backend.
#[async_trait]
pub trait SpeechToText: Send + Sync {
    fn name(&self) -> &str;

    /// Transcribe an audio buffer to text.
    async fn transcribe(&self, audio: &AudioBuffer) -> Result<Transcript>;
}

/// Text-to-speech backend.
#[async_trait]
pub trait TextToSpeech: Send + Sync {
    fn name(&self) -> &str;

    /// Synthesize text to an audio buffer.
    async fn synthesize(&self, text: &str, voice: &VoiceProfile) -> Result<AudioBuffer>;
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

    impl VoiceActivityDetector for CannedVad {
        fn name(&self) -> &str {
            "canned"
        }
        fn sample_rate(&self) -> u32 {
            16_000
        }
        fn chunk_samples(&self) -> usize {
            512
        }
        fn process_chunk(&mut self, _samples: &[f32]) -> Result<VadFrame> {
            let p = self.probs.next().unwrap_or(0.0);
            let is_speech = p >= 0.5;
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
        assert_eq!(vad.sample_rate(), 16_000);
        assert_eq!(vad.chunk_samples(), 512);
        let chunk = vec![0.0_f32; vad.chunk_samples()];
        let f0 = vad.process_chunk(&chunk).unwrap();
        let f1 = vad.process_chunk(&chunk).unwrap();
        let f2 = vad.process_chunk(&chunk).unwrap();
        assert_eq!(f0.event, VadEvent::None);
        assert_eq!(f1.event, VadEvent::SpeechStart);
        assert_eq!(f2.event, VadEvent::SpeechEnd);
    }
}
