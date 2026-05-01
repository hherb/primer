//! Silero VAD implementation of [`VoiceActivityDetector`].
//!
//! Wraps the bundled ONNX model from [`silero-vad-rust`]. Audio is fed in
//! 512-sample chunks at 16 kHz; the detector returns each chunk's speech
//! probability and emits `SpeechStart` / `SpeechEnd` events using a
//! threshold + min-silence debounce (see [`VadDebouncer`]).
//!
//! # Build prerequisites
//!
//! Enabling the `silero` feature pulls in the `ort` crate, which downloads
//! a prebuilt ONNX Runtime binary from `cdn.pyke.io` at first build. The
//! Silero model weights themselves are bundled inside `silero-vad-rust` and
//! need no separate download. After the first successful build the binary
//! is cached under the cargo target directory.

use primer_core::error::{PrimerError, Result};
use primer_core::speech::{VadFrame, VoiceActivityDetector};
use silero_vad_rust::load_silero_vad;
use silero_vad_rust::silero_vad::model::OnnxModel;

use crate::vad_debounce::{VadDebouncer, ms_to_chunks};

const SAMPLE_RATE: u32 = 16_000;
const CHUNK_SAMPLES: usize = 512;

/// Default speech-probability threshold. Matches the Silero upstream
/// `VadIterator` default and the value the model was tuned to. Lower
/// thresholds catch quiet child speech but admit more breath/noise.
const DEFAULT_THRESHOLD: f32 = 0.5;

/// Default trailing-silence required before emitting `SpeechEnd` (ms).
/// 300 ms is short enough that the Primer responds promptly after a
/// child stops speaking, but long enough to bridge inter-word pauses.
const DEFAULT_MIN_SILENCE_MS: u32 = 300;

/// Runtime-tunable parameters for the Silero VAD wrapper.
#[derive(Debug, Clone)]
pub struct SileroVadParams {
    /// Probability above which a chunk is treated as speech (0.0–1.0).
    pub threshold: f32,
    /// Trailing silence required to emit `SpeechEnd` (milliseconds).
    pub min_silence_ms: u32,
}

impl Default for SileroVadParams {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_THRESHOLD,
            min_silence_ms: DEFAULT_MIN_SILENCE_MS,
        }
    }
}

/// Streaming VAD backed by the Silero ONNX model.
///
/// One detector per audio stream. Internally holds the model's hidden
/// state plus a [`VadDebouncer`] for the threshold + min-silence state
/// machine. Default-constructed params match the Silero upstream
/// defaults; tune via [`SileroVadParams`] if a deployment needs different
/// sensitivity.
pub struct SileroVad {
    model: OnnxModel,
    debouncer: VadDebouncer,
}

impl SileroVad {
    /// Load the bundled Silero ONNX model and wrap it with the supplied
    /// debounce parameters. Fails if ONNX Runtime cannot initialise.
    pub fn new(params: SileroVadParams) -> Result<Self> {
        let model =
            load_silero_vad().map_err(|e| PrimerError::Speech(format!("load Silero VAD: {e}")))?;
        let silence_chunks = ms_to_chunks(params.min_silence_ms, SAMPLE_RATE, CHUNK_SAMPLES as u32);
        Ok(Self {
            model,
            debouncer: VadDebouncer::new(params.threshold, silence_chunks),
        })
    }

    /// Convenience: load the detector with [`SileroVadParams::default`].
    pub fn with_defaults() -> Result<Self> {
        Self::new(SileroVadParams::default())
    }
}

impl VoiceActivityDetector for SileroVad {
    fn name(&self) -> &str {
        "silero-vad"
    }

    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }

    fn chunk_samples(&self) -> usize {
        CHUNK_SAMPLES
    }

    fn process_chunk(&mut self, samples: &[f32]) -> Result<VadFrame> {
        if samples.len() != CHUNK_SAMPLES {
            return Err(PrimerError::Speech(format!(
                "Silero VAD requires exactly {CHUNK_SAMPLES} samples per chunk, got {}",
                samples.len()
            )));
        }
        let probs = self
            .model
            .forward_chunk(samples, SAMPLE_RATE)
            .map_err(|e| PrimerError::Speech(format!("VAD forward: {e}")))?;
        let speech_probability = probs[[0, 0]];
        let event = self.debouncer.step(speech_probability);
        Ok(VadFrame {
            speech_probability,
            event,
        })
    }

    fn reset(&mut self) {
        self.model.reset_states();
        self.debouncer.reset();
    }
}
