//! Stub speech backends for text-mode development.

use async_trait::async_trait;
use primer_core::error::Result;
use primer_core::speech::*;

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

#[async_trait]
impl TextToSpeech for StubTts {
    async fn synthesize(&self, text: &str, _voice: &VoiceProfile) -> Result<AudioBuffer> {
        tracing::info!(text_len = text.len(), "StubTts: would synthesize");
        Ok(AudioBuffer {
            samples: vec![0.0; 16000], // 1 second of silence
            sample_rate: 16000,
        })
    }
}
