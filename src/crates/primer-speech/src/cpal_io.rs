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

    /// Drain rubato's internal latency without feeding more input.
    /// Call once at end of stream; the held-back samples (typically
    /// `output_delay()` worth) come out as the final block.
    /// Without this, the last ~50 ms of input audio is silently
    /// retained inside the FFT machinery and never plays.
    pub fn flush(&mut self) -> Result<Vec<f32>> {
        let out = self
            .inner
            .process_partial::<&[f32]>(None, None)
            .map_err(|e| PrimerError::Speech(format!("rubato flush: {e}")))?;
        Ok(out.into_iter().next().unwrap_or_default())
    }
}

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

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
                )));
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

/// Default capacity (in samples) for the speaker SPSC ring buffer.
/// Sized for ~5 s of 48 kHz mono so a multi-second Piper phrase fits
/// without the producer's `push_slice` retry loop dropping samples.
/// The producer (`run_loop`'s `on_audio` callback) writes whole phrases
/// at once; the cpal output callback drains at hardware rate.
const SPEAKER_RINGBUF_CAPACITY: usize = 240_000;

/// Speaker playback: opens the default output device, drains f32 mono
/// samples from a ring buffer. The cpal `Stream` is held inside this
/// struct — dropping the struct stops playback.
///
/// Output is upsampled / channel-duplicated from the producer's mono
/// f32 stream to the device's native format inside the cpal callback.
/// On underrun, the callback writes silence (no clicks).
pub struct SpeakerSink {
    _stream: Stream,
    /// Sample rate the cpal device is running at. Producers must feed
    /// audio at this rate (resample upstream if needed).
    pub sample_rate: u32,
    pub channels: u16,
}

impl SpeakerSink {
    /// Open the default output device and start playback. Returns the
    /// sink (must stay alive) and the producer side of the sample
    /// ring buffer (push mono f32 at `self.sample_rate`).
    pub fn start() -> Result<(Self, HeapProd<f32>)> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| PrimerError::Speech("no default output device".into()))?;
        let supported = device
            .default_output_config()
            .map_err(|e| PrimerError::Speech(format!("default output config: {e}")))?;
        let sample_rate = supported.sample_rate(); // cpal 0.17: u32 directly, no .0
        let channels = supported.channels();
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();

        let rb = HeapRb::<f32>::new(SPEAKER_RINGBUF_CAPACITY);
        let (prod, mut cons) = rb.split();

        let err_callback = |err| tracing::error!("cpal output stream error: {err}");

        let stream = match format {
            SampleFormat::F32 => device.build_output_stream(
                &config,
                move |out: &mut [f32], _info| pull_to_device_f32(&mut cons, out, channels),
                err_callback,
                None,
            ),
            SampleFormat::I16 => device.build_output_stream(
                &config,
                move |out: &mut [i16], _info| pull_to_device_i16(&mut cons, out, channels),
                err_callback,
                None,
            ),
            SampleFormat::U16 => device.build_output_stream(
                &config,
                move |out: &mut [u16], _info| pull_to_device_u16(&mut cons, out, channels),
                err_callback,
                None,
            ),
            other => {
                return Err(PrimerError::Speech(format!(
                    "unsupported sample format: {other:?}"
                )));
            }
        }
        .map_err(|e| PrimerError::Speech(format!("build output stream: {e}")))?;

        stream
            .play()
            .map_err(|e| PrimerError::Speech(format!("start output stream: {e}")))?;

        Ok((
            Self {
                _stream: stream,
                sample_rate,
                channels,
            },
            prod,
        ))
    }
}

/// Pull one mono f32 sample per output frame; duplicate across channels.
/// On underrun, writes 0.0 (silence). Cannot block / allocate.
fn pull_to_device_f32(cons: &mut ringbuf::HeapCons<f32>, out: &mut [f32], channels: u16) {
    let n = channels.max(1) as usize;
    for frame in out.chunks_exact_mut(n) {
        let mono = cons.try_pop().unwrap_or(0.0);
        for slot in frame.iter_mut() {
            *slot = mono;
        }
    }
}

fn pull_to_device_i16(cons: &mut ringbuf::HeapCons<f32>, out: &mut [i16], channels: u16) {
    let n = channels.max(1) as usize;
    for frame in out.chunks_exact_mut(n) {
        let mono = cons.try_pop().unwrap_or(0.0).clamp(-1.0, 1.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let s = (mono * i16::MAX as f32) as i16;
        for slot in frame.iter_mut() {
            *slot = s;
        }
    }
}

fn pull_to_device_u16(cons: &mut ringbuf::HeapCons<f32>, out: &mut [u16], channels: u16) {
    let n = channels.max(1) as usize;
    let mid = u16::MAX as f32 / 2.0;
    for frame in out.chunks_exact_mut(n) {
        let mono = cons.try_pop().unwrap_or(0.0).clamp(-1.0, 1.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let s = (mono * mid + mid) as u16;
        for slot in frame.iter_mut() {
            *slot = s;
        }
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

    #[test]
    fn pull_to_device_f32_underruns_to_silence() {
        let rb = HeapRb::<f32>::new(8);
        let (mut prod, mut cons) = rb.split();
        let _ = prod.try_push(0.5);
        let mut out = vec![99.0f32; 6]; // 3 stereo frames
        pull_to_device_f32(&mut cons, &mut out, 2);
        // First frame: 0.5 / 0.5 (stereo dup); rest: silence (0.0).
        assert!((out[0] - 0.5).abs() < 1e-6);
        assert!((out[1] - 0.5).abs() < 1e-6);
        assert!((out[2] - 0.0).abs() < 1e-6);
        assert!((out[5] - 0.0).abs() < 1e-6);
    }

    /// Hardware smoke: capture 1 second of mic, immediately play it back to
    /// the speaker. Manual sanity for cpal device defaults; never in CI.
    ///
    /// Run with: `cargo test -p primer-speech --features cpal --release -- --ignored hardware_loopback`
    #[test]
    #[ignore = "needs mic + speaker; mac asks for mic permission on first run"]
    fn hardware_loopback_one_second() {
        // 1 s of capture
        let (mic, mut mic_cons) = MicCapture::start().expect("mic");
        let mic_rate = mic.sample_rate;
        let mut buf: Vec<f32> = Vec::with_capacity(mic_rate as usize);
        let started = std::time::Instant::now();
        while started.elapsed() < std::time::Duration::from_secs(1) {
            while let Some(s) = mic_cons.try_pop() {
                buf.push(s);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        drop(mic);

        // Open speaker; resample mic audio if needed; push.
        let (spk, mut spk_prod) = SpeakerSink::start().expect("spk");
        let spk_rate = spk.sample_rate;

        let to_play: Vec<f32> = if mic_rate == spk_rate {
            buf
        } else {
            let chunk = 1024;
            let mut r = Resampler::new(mic_rate, spk_rate, chunk).expect("resampler");
            let mut out = Vec::new();
            for window in buf.chunks(chunk) {
                if window.len() != chunk {
                    break;
                }
                out.extend(r.process(window).expect("process"));
            }
            out
        };

        use ringbuf::traits::Producer;
        let mut written = 0;
        while written < to_play.len() {
            written += spk_prod.push_slice(&to_play[written..]);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // Let the device drain.
        std::thread::sleep(std::time::Duration::from_millis(500));
        drop(spk);
    }
}
