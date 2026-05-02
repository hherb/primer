//! Stub speech backends for text-mode development.

use async_trait::async_trait;
use primer_core::error::Result;
use primer_core::speech::{
    AudioBuffer, AudioChunk, Named, SpeechToText, StreamingTextToSpeech, SynthesisSession,
    TextToSpeech, Transcript, VoiceProfile,
};

use crate::phrase_split::PhraseSplitter;

/// Stub STT — returns the audio buffer length as "transcribed text".
/// Used only for testing the pipeline without actual speech recognition.
pub struct StubStt;

impl Named for StubStt {
    fn name(&self) -> &str {
        "stub-stt"
    }
}

#[async_trait]
impl SpeechToText for StubStt {
    async fn transcribe(&self, audio: &AudioBuffer) -> Result<Transcript> {
        Ok(Transcript {
            text: format!(
                "[audio: {} samples at {}Hz]",
                audio.samples.len(),
                audio.sample_rate
            ),
            language: Some("en".to_string()),
            confidence: Some(1.0),
        })
    }
}

/// Stub TTS — returns a silent audio buffer.
pub struct StubTts;

impl Named for StubTts {
    fn name(&self) -> &str {
        "stub-tts"
    }
}

/// Stub-TTS sample rate; matches the rate the existing one-shot
/// `synthesize` returns.
const STUB_TTS_SAMPLE_RATE: u32 = 16_000;
/// Number of zero samples per emitted stub chunk. One short burst per
/// phrase is enough to exercise the trait surface in tests.
const STUB_TTS_SAMPLES_PER_CHUNK: usize = 1_024;

#[async_trait]
impl TextToSpeech for StubTts {
    async fn synthesize(&self, text: &str, _voice: &VoiceProfile) -> Result<AudioBuffer> {
        tracing::info!(text_len = text.len(), "StubTts: would synthesize");
        Ok(AudioBuffer {
            samples: vec![0.0; STUB_TTS_SAMPLE_RATE as usize], // 1 second of silence
            sample_rate: STUB_TTS_SAMPLE_RATE,
        })
    }
}

impl StreamingTextToSpeech for StubTts {
    fn sample_rate(&self) -> u32 {
        STUB_TTS_SAMPLE_RATE
    }

    fn open_session(&self, _voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>> {
        Ok(Box::new(StubSynthesisSession {
            splitter: PhraseSplitter::new(),
        }))
    }
}

struct StubSynthesisSession {
    splitter: PhraseSplitter,
}

impl StubSynthesisSession {
    fn silent_chunk() -> AudioChunk {
        AudioChunk {
            samples: vec![0.0; STUB_TTS_SAMPLES_PER_CHUNK],
            sample_rate: STUB_TTS_SAMPLE_RATE,
        }
    }
}

impl SynthesisSession for StubSynthesisSession {
    fn push_text(&mut self, text: &str) -> Result<Vec<AudioChunk>> {
        let phrases = self.splitter.push(text);
        Ok(phrases.iter().map(|_| Self::silent_chunk()).collect())
    }

    fn finalize(mut self: Box<Self>) -> Result<Vec<AudioChunk>> {
        match self.splitter.flush() {
            Some(_) => Ok(vec![Self::silent_chunk()]),
            None => Ok(vec![]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primer_core::speech::{StreamingTextToSpeech, VoiceProfile};

    /// `StubTts` claims this rate as a no-op default since it emits silence.
    /// Mirrors the value used in stub.rs's existing one-shot synthesize body.
    const STUB_TTS_SAMPLE_RATE: u32 = 16_000;

    #[tokio::test]
    async fn stub_tts_streaming_emits_chunk_per_phrase() {
        let tts: Box<dyn StreamingTextToSpeech> = Box::new(StubTts);
        assert_eq!(tts.sample_rate(), STUB_TTS_SAMPLE_RATE);
        let mut session = tts.open_session(&VoiceProfile::default()).unwrap();
        let phrases = session.push_text("Hello. World. ").unwrap();
        assert_eq!(phrases.len(), 2);
        for chunk in &phrases {
            assert_eq!(chunk.sample_rate, STUB_TTS_SAMPLE_RATE);
            assert!(!chunk.samples.is_empty());
            assert!(chunk.samples.iter().all(|&s| s == 0.0));
        }
    }

    #[tokio::test]
    async fn stub_tts_streaming_finalize_drains_trailing() {
        let tts: Box<dyn StreamingTextToSpeech> = Box::new(StubTts);
        let mut session = tts.open_session(&VoiceProfile::default()).unwrap();
        let mid = session.push_text("Hello").unwrap();
        assert!(mid.is_empty());
        let trailing = session.finalize().unwrap();
        assert_eq!(trailing.len(), 1);
        assert_eq!(trailing[0].sample_rate, STUB_TTS_SAMPLE_RATE);
    }
}
