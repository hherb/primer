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
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

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
    /// Set to `true` by the cpal `err_callback` if the output stream
    /// reports an error (device removed, host audio service crashed,
    /// etc.). Producers should bail their retry loops when this flips
    /// — otherwise `push_slice` returns 0 forever and the producer
    /// hangs waiting for a drain that will never come.
    errored: Arc<AtomicBool>,
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

        let errored = Arc::new(AtomicBool::new(false));

        // Release on the producer side pairs with Acquire on the consumer
        // (push_all_with_bail / wait_for_drain). SeqCst would also work
        // but is stricter than this single-flag handshake needs.
        let make_err_callback = || {
            let flag = Arc::clone(&errored);
            move |err| {
                tracing::error!("cpal output stream error: {err}");
                flag.store(true, Ordering::Release);
            }
        };

        let stream = match format {
            SampleFormat::F32 => device.build_output_stream(
                &config,
                move |out: &mut [f32], _info| pull_to_device_f32(&mut cons, out, channels),
                make_err_callback(),
                None,
            ),
            SampleFormat::I16 => device.build_output_stream(
                &config,
                move |out: &mut [i16], _info| pull_to_device_i16(&mut cons, out, channels),
                make_err_callback(),
                None,
            ),
            SampleFormat::U16 => device.build_output_stream(
                &config,
                move |out: &mut [u16], _info| pull_to_device_u16(&mut cons, out, channels),
                make_err_callback(),
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
                errored,
            },
            prod,
        ))
    }

    /// Clone of the "stream errored" flag. Producers call this once at
    /// startup and pass the clone into [`push_all_with_bail`] so the
    /// retry loop terminates if the cpal output stream reports an error.
    pub fn errored_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.errored)
    }
}

/// Push every sample of `samples` into `prod`, retrying with `retry_sleep`
/// between attempts when the ring buffer is momentarily full. Returns the
/// total number of samples written.
///
/// The retry loop bails — without writing the rest — if `errored` flips
/// to `true`. This is the failsafe for a cpal output stream that has
/// stopped draining (device removed, host audio service crashed): without
/// it, `push_slice` returns 0 forever and the caller hangs mid-response.
/// In production, callers wire `errored` from [`SpeakerSink::errored_flag`].
///
/// `retry_sleep` is parameterised so tests can pass `Duration::ZERO`. The
/// production caller should pass ~5 ms — long enough to yield CPU between
/// drain cycles, short enough to keep produce-side latency negligible.
///
/// Acquire on the load pairs with the Release store in cpal's err_callback.
pub fn push_all_with_bail(
    prod: &mut HeapProd<f32>,
    samples: &[f32],
    errored: &AtomicBool,
    retry_sleep: Duration,
) -> usize {
    let mut written = 0;
    while written < samples.len() {
        if errored.load(Ordering::Acquire) {
            let dropped = samples.len() - written;
            tracing::warn!(
                "cpal output stream errored; discarding {dropped} samples (of {} total)",
                samples.len()
            );
            return written;
        }
        let n = prod.push_slice(&samples[written..]);
        written += n;
        if written < samples.len() && !retry_sleep.is_zero() {
            std::thread::sleep(retry_sleep);
        }
    }
    written
}

/// Block until cpal has fully drained the speaker ringbuf, plus a small
/// grace period for the cpal output buffer to flush past the ringbuf.
/// Returns `true` if the drain completed cleanly; `false` if the wait
/// hit `max_wait` or the stream errored mid-wait. Bails early on
/// `errored` because a dead stream will never drain.
///
/// Why we poll `occupied_len()` instead of trusting the heuristic
/// `samples / sample_rate + 0.4s`: the producer only knows the *queued*
/// duration, not the *played* duration. On a slower or contended host,
/// cpal may take longer than the safety margin to drain — leaking the
/// tail of the Primer's voice into the mic for transcription as a
/// "child utterance". Polling the actual ringbuf is exact.
///
/// `consecutive_zero_checks` requires that many sequential polls observe
/// an empty buffer before declaring drain complete; this defends against
/// a transient zero between two cpal callbacks. `grace` covers the cpal
/// output device's own internal buffering (typically ~10–30 ms) and is
/// included in `max_wait` — total wallclock spent in this call is at
/// most `max_wait`, so callers can trust it as a true upper bound. If
/// `grace >= max_wait` the observation window is zero and the call
/// times out without observing.
///
/// Acquire on the errored load pairs with the Release store in cpal's
/// err_callback.
pub fn wait_for_drain(
    prod: &HeapProd<f32>,
    errored: &AtomicBool,
    poll_interval: Duration,
    consecutive_zero_checks: u32,
    grace: Duration,
    max_wait: Duration,
) -> bool {
    let start = std::time::Instant::now();
    let observation_budget = max_wait.saturating_sub(grace);
    let mut zero_streak: u32 = 0;
    while start.elapsed() < observation_budget {
        if errored.load(Ordering::Acquire) {
            tracing::warn!("wait_for_drain: cpal output errored mid-drain; bailing");
            return false;
        }
        if prod.occupied_len() == 0 {
            zero_streak += 1;
            if zero_streak >= consecutive_zero_checks {
                std::thread::sleep(grace);
                return true;
            }
        } else {
            zero_streak = 0;
        }
        std::thread::sleep(poll_interval);
    }
    tracing::warn!(
        "wait_for_drain: ringbuf still has {} samples after {} ms; giving up",
        prod.occupied_len(),
        max_wait.as_millis()
    );
    false
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
mod tests;
