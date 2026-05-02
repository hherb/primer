//! cpal-based audio I/O for the speech REPL.
//!
//! See module-level docs further down. This file currently holds the
//! `Resampler` adapter; mic and speaker types follow.

use primer_core::error::{PrimerError, Result};
use rubato::{FftFixedIn, Resampler as RubatoResampler};

/// Sub-chunk count for `FftFixedIn`. 1 = single FFT per call; higher
/// values trade latency for slightly better frequency resolution.
/// 1 is fine for our coarse 48 kHz → 16 kHz / 22 kHz → 48 kHz needs.
const RESAMPLER_SUB_CHUNKS: usize = 1;

/// Adapter around `rubato::FftFixedIn` that resamples mono f32 audio
/// from one sample rate to another.
///
/// Constructed with a fixed input chunk size; callers feed exactly that
/// many samples per `process` call (or pad / split externally). Output
/// chunk size is derived from the rate ratio.
pub struct Resampler {
    inner: FftFixedIn<f32>,
    input_chunk_samples: usize,
}

impl Resampler {
    /// Create a resampler from `input_rate` to `output_rate`, expecting
    /// exactly `input_chunk_samples` mono f32 samples per `process` call.
    pub fn new(input_rate: u32, output_rate: u32, input_chunk_samples: usize) -> Result<Self> {
        let inner = FftFixedIn::<f32>::new(
            input_rate as usize,
            output_rate as usize,
            input_chunk_samples,
            RESAMPLER_SUB_CHUNKS,
            1, // mono
        )
        .map_err(|e| PrimerError::Speech(format!("rubato init: {e}")))?;
        Ok(Self {
            inner,
            input_chunk_samples,
        })
    }

    /// Resample one chunk of mono f32 audio. Input length must equal
    /// the constructor-time `input_chunk_samples`; otherwise errors.
    pub fn process(&mut self, input: &[f32]) -> Result<Vec<f32>> {
        if input.len() != self.input_chunk_samples {
            return Err(PrimerError::Speech(format!(
                "Resampler expected {} samples, got {}",
                self.input_chunk_samples,
                input.len()
            )));
        }
        let out = self
            .inner
            .process(&[input], None)
            .map_err(|e| PrimerError::Speech(format!("rubato process: {e}")))?;
        // mono input → mono output: take the first (only) channel.
        out.into_iter()
            .next()
            .ok_or_else(|| PrimerError::Speech("rubato returned no channels".into()))
    }
}

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::{HeapCons, HeapRb};

/// Default capacity (in samples) for the mic SPSC ring buffer.
/// Sized to hold ~250 ms of 48 kHz mono audio so a brief consumer
/// stall doesn't drop samples. cpal callbacks fire every few ms.
const MIC_RINGBUF_CAPACITY: usize = 12_000;

/// Microphone capture: opens the default input device, pushes mono f32
/// samples into a ring buffer. The cpal `Stream` is held inside this
/// struct — dropping the struct stops capture.
///
/// Multi-channel devices are summed to mono in the cpal callback (cheap
/// per-frame work; runs on cpal's audio thread so must not allocate).
pub struct MicCapture {
    /// cpal stream — held to keep capture alive. Underscore-named so
    /// the field's drop-on-drop role is explicit.
    _stream: Stream,
    /// Native sample rate of the opened input device. Callers feed this
    /// into [`Resampler::new`] when targeting silero/whisper at 16 kHz.
    pub sample_rate: u32,
    /// Native channel count of the opened input device. Capture sums to
    /// mono, but callers may want this for diagnostics.
    pub channels: u16,
}

impl MicCapture {
    /// Open the default input device and start capture.
    ///
    /// Returns a `(MicCapture, HeapCons<f32>)` pair. The capture struct
    /// must stay alive for samples to keep arriving; the consumer is the
    /// pull-side of the ring buffer.
    pub fn start() -> Result<(Self, HeapCons<f32>)> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| PrimerError::Speech("no default input device".into()))?;
        let supported = device
            .default_input_config()
            .map_err(|e| PrimerError::Speech(format!("default input config: {e}")))?;
        let sample_rate = supported.sample_rate();
        let channels = supported.channels();
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();

        let rb = HeapRb::<f32>::new(MIC_RINGBUF_CAPACITY);
        let (mut prod, cons) = rb.split();

        let err_callback = |err| tracing::error!("cpal input stream error: {err}");

        let stream = match format {
            SampleFormat::F32 => device.build_input_stream(
                &config,
                move |samples: &[f32], _info| push_mono_f32(&mut prod, samples, channels),
                err_callback,
                None,
            ),
            SampleFormat::I16 => device.build_input_stream(
                &config,
                move |samples: &[i16], _info| push_mono_i16(&mut prod, samples, channels),
                err_callback,
                None,
            ),
            SampleFormat::U16 => device.build_input_stream(
                &config,
                move |samples: &[u16], _info| push_mono_u16(&mut prod, samples, channels),
                err_callback,
                None,
            ),
            other => {
                return Err(PrimerError::Speech(format!(
                    "unsupported sample format: {other:?}"
                )))
            }
        }
        .map_err(|e| PrimerError::Speech(format!("build input stream: {e}")))?;

        stream
            .play()
            .map_err(|e| PrimerError::Speech(format!("start input stream: {e}")))?;

        Ok((
            Self {
                _stream: stream,
                sample_rate,
                channels,
            },
            cons,
        ))
    }
}

/// Push interleaved f32 samples into the ring buffer, summing channels
/// to mono. Drops samples on overflow (cpal callback cannot block).
fn push_mono_f32(prod: &mut ringbuf::HeapProd<f32>, samples: &[f32], channels: u16) {
    let n = channels.max(1) as usize;
    for frame in samples.chunks_exact(n) {
        let mono = frame.iter().sum::<f32>() / n as f32;
        let _ = prod.try_push(mono);
    }
}

fn push_mono_i16(prod: &mut ringbuf::HeapProd<f32>, samples: &[i16], channels: u16) {
    let n = channels.max(1) as usize;
    let scale = 1.0 / (i16::MAX as f32);
    for frame in samples.chunks_exact(n) {
        let mono: f32 = frame.iter().map(|&s| s as f32 * scale).sum::<f32>() / n as f32;
        let _ = prod.try_push(mono);
    }
}

fn push_mono_u16(prod: &mut ringbuf::HeapProd<f32>, samples: &[u16], channels: u16) {
    let n = channels.max(1) as usize;
    let scale = 1.0 / (u16::MAX as f32 / 2.0);
    for frame in samples.chunks_exact(n) {
        let mono: f32 = frame
            .iter()
            .map(|&s| (s as f32 - (u16::MAX as f32 / 2.0)) * scale)
            .sum::<f32>()
            / n as f32;
        let _ = prod.try_push(mono);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity: 48 kHz → 16 kHz resampling produces ~1/3 the sample count.
    /// FFT buffering may cause the first call to return 0 samples; feed two
    /// chunks to verify the rate is correct.
    #[test]
    fn resampler_48k_to_16k_reduces_sample_count_by_about_three() {
        let chunk = 1024;
        let mut r = Resampler::new(48_000, 16_000, chunk).expect("construct");
        let input = vec![0.0f32; chunk];
        // Prime the buffer with a dummy chunk.
        let _primed = r.process(&input).expect("process (prime)");
        // Second chunk; should contain real output.
        let out = r.process(&input).expect("process (main)");
        // Allow ±20% slack for FFT edge effects.
        let expected = chunk / 3;
        let lower = expected * 4 / 5;
        let upper = expected * 6 / 5;
        assert!(
            out.len() >= lower && out.len() <= upper,
            "expected ~{expected} samples, got {}",
            out.len()
        );
    }

    #[test]
    fn resampler_rejects_wrong_input_length() {
        let mut r = Resampler::new(48_000, 16_000, 1024).expect("construct");
        let too_short = vec![0.0f32; 512];
        let err = r.process(&too_short).expect_err("should reject");
        assert!(
            matches!(err, PrimerError::Speech(_)),
            "expected PrimerError::Speech, got {err:?}"
        );
    }

    #[test]
    fn push_mono_f32_sums_stereo_to_mono() {
        let rb = HeapRb::<f32>::new(64);
        let (mut prod, mut cons) = rb.split();
        // Stereo frames: (1.0, 0.5), (-0.5, 0.5)
        let stereo: Vec<f32> = vec![1.0, 0.5, -0.5, 0.5];
        push_mono_f32(&mut prod, &stereo, 2);
        let frame0 = cons.try_pop().expect("frame 0");
        let frame1 = cons.try_pop().expect("frame 1");
        assert!((frame0 - 0.75).abs() < 1e-6);
        assert!((frame1 - 0.0).abs() < 1e-6);
    }

    #[test]
    fn push_mono_f32_passes_through_mono() {
        let rb = HeapRb::<f32>::new(64);
        let (mut prod, mut cons) = rb.split();
        let mono: Vec<f32> = vec![0.1, 0.2, 0.3];
        push_mono_f32(&mut prod, &mono, 1);
        assert!((cons.try_pop().unwrap() - 0.1).abs() < 1e-6);
        assert!((cons.try_pop().unwrap() - 0.2).abs() < 1e-6);
        assert!((cons.try_pop().unwrap() - 0.3).abs() < 1e-6);
    }
}
