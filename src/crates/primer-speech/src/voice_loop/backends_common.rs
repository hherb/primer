//! Shared backend types and helpers for the voice loop.
//!
//! Holds [`LocalBackends`] (the bundle every builder returns) and
//! [`ChannelStt`] (the streaming-STT adapter that decouples the audio
//! capture thread from the voice loop), plus a small set of factories
//! ([`SpeakerPipeline`], [`MicPipeline`], [`make_on_audio`],
//! [`make_drain_hook`], [`open_mic_with_resampler`]) that absorb the
//! ~250-line tail every concrete builder used to copy verbatim.
//!
//! All concrete backend builders — whisper+piper in [`super::backends`],
//! SFSpeechRecognizer + Silero in [`super::backends_macos_native`], and
//! SpeechAnalyzer + derived VAD in [`super::backends_macos_native_26`] —
//! share these types and helpers.
//!
//! Gated by the parent `voice_loop` module on `cpal` (the lowest common
//! denominator: every builder needs `MicCapture`/`SpeakerSink`), so the
//! macOS builders don't have to inherit `whisper`/`piper`/`silero`
//! dependencies they never touch.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use ringbuf::HeapCons;
use ringbuf::HeapProd;

use primer_core::error::{PrimerError, Result};
use primer_core::speech::{Named, StreamingSpeechToText, TranscriptSegment, TranscriptionSession};

use crate::voice_loop::{DrainHook, LoopBackends};
use crate::{MicCapture, Resampler, SpeakerSink};

/// Output resampler chunk size (input samples per `resampler.process`
/// call), shared by every builder. 1024 was empirically picked when the
/// voice loop first shipped: large enough that rubato's per-call overhead
/// is amortised, small enough that mid-phrase latency from buffering one
/// extra chunk is inaudible.
pub(super) const OUTPUT_RESAMPLER_CHUNK_IN: usize = 1024;

/// `push_all_with_bail` retry sleep between drain cycles (production
/// value). Long enough to yield CPU between drain cycles, short enough
/// to keep produce-side latency negligible — see the constant's use site
/// in [`crate::push_all_with_bail`].
pub(super) const PUSH_RETRY_SLEEP: Duration = Duration::from_millis(5);

/// Drain-hook poll cadence (one tick) plus `consecutive_zero_checks`
/// (how many sequential empty observations declare drain complete).
/// `grace` covers cpal's own output-buffer latency past the ringbuf and
/// is included in `max_wait`, so total wallclock spent in the hook is
/// bounded by `max_wait`.
pub(super) const DRAIN_POLL_TICK: Duration = Duration::from_millis(10);
pub(super) const DRAIN_CONSECUTIVE_ZERO_CHECKS: u32 = 3;
pub(super) const DRAIN_GRACE: Duration = Duration::from_millis(80);
pub(super) const DRAIN_MAX_WAIT: Duration = Duration::from_secs(5);

/// Number of trailing silence chunks driven through the output resampler
/// on the end-of-turn flush sentinel (an empty `Vec<f32>` sent through
/// `on_audio`). Drains rubato's internal FFT buffer so the last syllable
/// of a phrase isn't silently discarded. See the long comment at
/// [`crate::cpal_io::Resampler`].
pub(super) const FLUSH_SILENCE_CHUNKS: usize = 4;

/// Shared receiver for the audio-thread → voice-loop transcript channel.
/// `Arc<Mutex<...>>` because the `StreamingSpeechToText` trait hands out
/// sessions through `&self`; there is exactly one consumer.
pub(super) type TranscriptRx = Arc<Mutex<std::sync::mpsc::Receiver<String>>>;

/// `StreamingSpeechToText` adapter: hands out sessions whose `finalize`
/// pulls a transcript from a `std::sync::mpsc` channel populated by the
/// audio capture thread. Decouples the audio thread (which owns the real
/// STT session and pushes mic samples into it) from the voice loop
/// (which only reads `VadEvent`s and calls `finalize` on the session it
/// opened).
///
/// **Ordering contract:** the audio thread MUST send the transcript on
/// `tx` BEFORE emitting `VadEvent::SpeechEnd` on the event channel. That
/// way, by the time the voice loop calls `session.finalize()` (after
/// seeing `SpeechEnd`), the transcript is already buffered in the channel.
pub struct ChannelStt {
    pub(super) rx: TranscriptRx,
}

impl Named for ChannelStt {
    fn name(&self) -> &str {
        "channel-stt"
    }
}

impl StreamingSpeechToText for ChannelStt {
    fn sample_rate(&self) -> u32 {
        16_000
    }
    fn open_session(&self) -> Result<Box<dyn TranscriptionSession>> {
        Ok(Box::new(ChannelSttSession {
            rx: Arc::clone(&self.rx),
        }))
    }
}

struct ChannelSttSession {
    rx: TranscriptRx,
}

impl TranscriptionSession for ChannelSttSession {
    fn push_audio(&mut self, _samples: &[f32]) -> Result<Vec<TranscriptSegment>> {
        // The voice loop never calls push_audio on this session
        // (samples are pushed directly into the STT backend inside the
        // audio thread). If someone wires it up differently this is a
        // silent no-op.
        Ok(vec![])
    }
    fn finalize(self: Box<Self>) -> Result<Vec<TranscriptSegment>> {
        // The audio thread sends BEFORE emitting SpeechEnd, so by the
        // time the voice loop calls us the transcript is normally already
        // buffered. Spin try_recv briefly to tolerate scheduling jitter —
        // but fail loudly rather than blocking, so a contract violation
        // surfaces as a quick "empty utterance" instead of a half-second
        // stall in the async runtime.
        let rx = self.rx.lock().map_err(|_| {
            PrimerError::Speech("ChannelSttSession: transcript receiver mutex poisoned".into())
        })?;
        const TRANSCRIPT_RECV_RETRIES: u32 = 5;
        const TRANSCRIPT_RECV_BACKOFF: Duration = Duration::from_millis(2);
        for _ in 0..TRANSCRIPT_RECV_RETRIES {
            match rx.try_recv() {
                Ok(text) => {
                    return Ok(vec![TranscriptSegment {
                        text,
                        start_ms: 0,
                        end_ms: 0,
                    }]);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    std::thread::sleep(TRANSCRIPT_RECV_BACKOFF);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err(PrimerError::Speech(
                        "audio thread disconnected before producing transcript".into(),
                    ));
                }
            }
        }
        // Contract violation (no transcript queued before SpeechEnd) —
        // empty utterance. The voice loop short-circuits on empty.
        Ok(vec![])
    }
}

/// All the resources `run_loop`/`run_loop_borrowed` needs, plus the
/// owning state for the audio thread + cpal streams.
///
/// Every consumable field is `Option<T>` so the caller can `.take()`
/// each one once when driving the loop:
/// - `backends.take().unwrap()` → first arg to `run_loop[_borrowed]`
/// - `event_rx.take().unwrap()` → second arg
/// - `on_audio.take().unwrap()` → `on_committed_audio`
/// - `Some(drain_hook.take().unwrap())` → `wait_for_speaker_drain`
/// - `Some(Arc::clone(&is_speaking))` → `is_speaking`
///
/// After the loop returns, call [`LocalBackends::shutdown`] to stop the
/// audio thread cleanly; dropping `LocalBackends` also signals + joins
/// the thread defensively.
pub struct LocalBackends {
    /// `Option` so the caller can move it into `run_loop[_borrowed]`
    /// without leaving an unusable placeholder behind in `self`.
    pub backends: Option<LoopBackends>,
    pub event_rx: Option<tokio::sync::mpsc::Receiver<primer_core::speech::VadEvent>>,
    /// Speaker push callback. `take()` once at run-loop-call time.
    pub on_audio: Option<Box<dyn FnMut(Vec<f32>) + Send>>,
    /// Drain hook used by the loop at the end of each SPEAK phase.
    /// `take()` once at run-loop-call time.
    pub drain_hook: Option<DrainHook>,
    /// Anti-feedback gate shared with the audio thread.
    pub is_speaking: Arc<AtomicBool>,
    // ── owned resources kept alive for the duration of the loop ──
    pub(super) audio_thread: Option<std::thread::JoinHandle<Result<()>>>,
    pub(super) stop_flag: Arc<AtomicBool>,
    // Kept alive (cpal streams stop when these drop).
    pub(super) _mic: MicCapture,
    pub(super) _spk: SpeakerSink,
}

impl LocalBackends {
    /// Signal the audio thread to stop and join it. Idempotent; safe to
    /// call after a previous call or after the loop has already exited.
    pub fn shutdown(&mut self) {
        self.stop_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(handle) = self.audio_thread.take() {
            if let Err(panic) = handle.join() {
                tracing::warn!("audio thread panicked: {panic:?}");
            }
        }
    }
}

impl Drop for LocalBackends {
    fn drop(&mut self) {
        // Defensive: if the caller forgot to call shutdown(), still
        // signal the audio thread so it doesn't outlive the streams.
        self.stop_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(handle) = self.audio_thread.take() {
            // Don't block forever in Drop; the thread polls stop_flag
            // every 5 ms so a join here is bounded.
            let _ = handle.join();
        }
    }
}

// ───────────────────────────────────────────────────────────────────────
// Shared builder helpers
// ───────────────────────────────────────────────────────────────────────

/// Bundle returned by [`open_mic_with_resampler`].
///
/// Every voice-loop builder needs the four pieces together (mic stream
/// owner, mic ringbuf consumer for the audio thread, optional input
/// resampler when the device rate differs from the STT target rate, and
/// the chunk size at mic rate that maps to one target-rate chunk).
pub(super) struct MicPipeline {
    pub(super) mic: MicCapture,
    pub(super) mic_cons: HeapCons<f32>,
    pub(super) input_resampler: Option<Resampler>,
    pub(super) in_chunk_samples: usize,
}

/// Open the system mic and (lazily) build the input resampler to
/// `target_rate`. `target_chunk_samples` is the STT/VAD chunk size at
/// `target_rate`; `in_chunk_samples` is the matching window at mic rate.
///
/// `verbose` prints one stderr line on success — the CLI flips this on,
/// the GUI flips it off and logs via tracing in the caller.
pub(super) fn open_mic_with_resampler(
    target_rate: u32,
    target_chunk_samples: usize,
    verbose: bool,
) -> Result<MicPipeline> {
    let (mic, mic_cons) = MicCapture::start()?;
    let mic_rate = mic.sample_rate;
    if verbose {
        eprintln!(
            "[speech] mic opened: {}Hz, {} channels",
            mic_rate, mic.channels
        );
    }

    let in_chunk_samples: usize =
        (target_chunk_samples as u64 * mic_rate as u64 / target_rate as u64) as usize;
    let input_resampler: Option<Resampler> = if mic_rate != target_rate {
        Some(Resampler::new(mic_rate, target_rate, in_chunk_samples)?)
    } else {
        None
    };

    Ok(MicPipeline {
        mic,
        mic_cons,
        input_resampler,
        in_chunk_samples,
    })
}

/// Bundle returned by [`SpeakerPipeline::start`].
///
/// Owns the cpal speaker stream and all the Arc-shared state every
/// builder threads through both the `on_audio` closure and the drain
/// hook. Fields are `pub(super)` so the per-builder modules can
/// destructure freely and pass each Arc clone into the closure builders.
pub(super) struct SpeakerPipeline {
    pub(super) spk: SpeakerSink,
    pub(super) spk_prod: Arc<Mutex<HeapProd<f32>>>,
    pub(super) spk_errored: Arc<AtomicBool>,
    pub(super) output_resampler: Arc<Mutex<Option<Resampler>>>,
    pub(super) need_output_resample: bool,
    pub(super) output_chunk_in: usize,
}

impl SpeakerPipeline {
    /// Open the system speaker and (lazily) build the output resampler
    /// from `tts_sample_rate` to the cpal-picked device rate.
    pub(super) fn start(tts_sample_rate: u32, verbose: bool) -> Result<Self> {
        let (spk, spk_prod) = SpeakerSink::start()?;
        let spk_rate = spk.sample_rate;
        let spk_errored = spk.errored_flag();
        let spk_prod = Arc::new(Mutex::new(spk_prod));
        if verbose {
            eprintln!(
                "[speech] speaker opened: {}Hz, {} channels",
                spk_rate, spk.channels
            );
        }

        let need_output_resample = tts_sample_rate != spk_rate;
        let output_chunk_in = OUTPUT_RESAMPLER_CHUNK_IN;
        let output_resampler = Arc::new(Mutex::new(if need_output_resample {
            Some(Resampler::new(tts_sample_rate, spk_rate, output_chunk_in)?)
        } else {
            None
        }));

        Ok(Self {
            spk,
            spk_prod,
            spk_errored,
            output_resampler,
            need_output_resample,
            output_chunk_in,
        })
    }
}

/// Build the `on_audio` closure that drives synthesised TTS PCM into the
/// speaker ringbuf.
///
/// Behaviour:
/// - An empty `Vec<f32>` is the end-of-turn flush sentinel: pads any
///   resampler tail with zeros and drives [`FLUSH_SILENCE_CHUNKS`]
///   silence frames so rubato's FFT-buffered output reaches the speaker.
///   With no resampling, an empty input is a no-op.
/// - Non-empty input is resampled (if needed) and pushed via
///   [`crate::push_all_with_bail`], which yields the CPU between drain
///   cycles and bails if `spk_errored` flips (cpal stream died).
/// - `output_leftover` is closure-local state that carries the unaligned
///   tail of one call into the head of the next, so phrase boundaries
///   never zero-pad mid-stream.
pub(super) fn make_on_audio(
    spk_prod: Arc<Mutex<HeapProd<f32>>>,
    spk_errored: Arc<AtomicBool>,
    output_resampler: Arc<Mutex<Option<Resampler>>>,
    output_chunk_in: usize,
    need_output_resample: bool,
) -> Box<dyn FnMut(Vec<f32>) + Send> {
    let mut output_leftover: Vec<f32> = Vec::with_capacity(output_chunk_in);
    Box::new(move |samples| {
        let is_flush = samples.is_empty();
        let mut samples = samples;
        if need_output_resample {
            let mut guard = output_resampler.lock().unwrap();
            if let Some(resampler) = guard.as_mut() {
                let mut combined: Vec<f32> =
                    Vec::with_capacity(output_leftover.len() + samples.len());
                combined.append(&mut output_leftover);
                combined.append(&mut samples);
                let usable = (combined.len() / output_chunk_in) * output_chunk_in;
                let mut out_buf: Vec<f32> = Vec::with_capacity(combined.len() * 2);
                let mut i = 0;
                while i + output_chunk_in <= usable {
                    let block = &combined[i..i + output_chunk_in];
                    match resampler.process(block) {
                        Ok(o) => out_buf.extend(o),
                        Err(e) => {
                            tracing::warn!("output resampler error: {e}");
                            return;
                        }
                    }
                    i += output_chunk_in;
                }
                let tail: Vec<f32> = combined[usable..].to_vec();
                if is_flush {
                    if !tail.is_empty() {
                        let mut padded = tail;
                        padded.resize(output_chunk_in, 0.0);
                        if let Ok(o) = resampler.process(&padded) {
                            out_buf.extend(o);
                        }
                    }
                    let silence = vec![0.0_f32; output_chunk_in];
                    for _ in 0..FLUSH_SILENCE_CHUNKS {
                        match resampler.process(&silence) {
                            Ok(o) => out_buf.extend(o),
                            Err(_) => break,
                        }
                    }
                    output_leftover = Vec::new();
                } else {
                    output_leftover = tail;
                }
                samples = out_buf;
            }
        }
        if !samples.is_empty() {
            let mut prod = spk_prod.lock().expect("speaker producer mutex poisoned");
            crate::push_all_with_bail(&mut prod, &samples, &spk_errored, PUSH_RETRY_SLEEP);
        }
    })
}

/// Build the drain hook the voice loop calls at the end of each SPEAK
/// phase. Runs the blocking [`crate::wait_for_drain`] poll inside
/// `tokio::task::spawn_blocking` so the sync wait does not block the
/// tokio worker — required when the voice loop's runtime is the
/// single-threaded current-thread runtime (the macOS-native CLI path).
pub(super) fn make_drain_hook(
    spk_prod: Arc<Mutex<HeapProd<f32>>>,
    spk_errored: Arc<AtomicBool>,
) -> DrainHook {
    Box::new(move || {
        let prod = Arc::clone(&spk_prod);
        let errored = Arc::clone(&spk_errored);
        Box::pin(async move {
            let join = tokio::task::spawn_blocking(move || {
                let prod_guard = prod.lock().expect("speaker producer mutex poisoned");
                let _ = crate::wait_for_drain(
                    &prod_guard,
                    &errored,
                    DRAIN_POLL_TICK,
                    DRAIN_CONSECUTIVE_ZERO_CHECKS,
                    DRAIN_GRACE,
                    DRAIN_MAX_WAIT,
                );
            });
            if let Err(e) = join.await {
                tracing::warn!("speaker drain task did not complete: {e:?}");
            }
        })
    })
}

#[cfg(test)]
mod tests {
    //! Pure-logic tests for [`make_on_audio`] — these don't open a real
    //! cpal device, just a vanilla `ringbuf::HeapRb` so the closure
    //! body's contract (no-op on flush w/o resampling, pushes everything
    //! straight through w/o resampling, bails when `spk_errored` is set)
    //! is pinned without an audio-device dep. The resampling branch is
    //! exercised by the audio-quality smoke at
    //! `examples/tts_hello.rs` since a meaningful unit test would just
    //! re-test rubato.

    use super::*;
    use ringbuf::HeapRb;
    use ringbuf::traits::{Consumer as _, Observer as _, Split as _};
    use std::sync::atomic::Ordering;

    /// Bench-style fixture for the [`make_on_audio`] tests. Owning the
    /// consumer alongside the producer lets each test verify the closure's
    /// push behaviour by inspecting the ringbuf state directly.
    struct Fixture {
        spk_prod: Arc<Mutex<HeapProd<f32>>>,
        spk_cons: ringbuf::HeapCons<f32>,
        spk_errored: Arc<AtomicBool>,
        output_resampler: Arc<Mutex<Option<Resampler>>>,
    }

    fn setup() -> Fixture {
        let rb = HeapRb::<f32>::new(4096);
        let (prod, cons) = rb.split();
        Fixture {
            spk_prod: Arc::new(Mutex::new(prod)),
            spk_cons: cons,
            spk_errored: Arc::new(AtomicBool::new(false)),
            output_resampler: Arc::new(Mutex::new(None)),
        }
    }

    #[test]
    fn make_on_audio_without_resampling_pushes_samples_through() {
        let Fixture {
            spk_prod,
            mut spk_cons,
            spk_errored,
            output_resampler,
        } = setup();
        let mut closure = make_on_audio(
            spk_prod,
            spk_errored,
            output_resampler,
            OUTPUT_RESAMPLER_CHUNK_IN,
            /* need_output_resample */ false,
        );

        let samples: Vec<f32> = (0..16).map(|i| i as f32 * 0.1).collect();
        closure(samples.clone());

        let mut received = vec![0.0_f32; samples.len()];
        let n = spk_cons.pop_slice(&mut received);
        assert_eq!(n, samples.len(), "all input samples reach the speaker");
        assert_eq!(received, samples, "samples flow through verbatim");
    }

    #[test]
    fn make_on_audio_empty_flush_without_resampling_is_a_noop() {
        let Fixture {
            spk_prod,
            spk_cons,
            spk_errored,
            output_resampler,
        } = setup();
        let mut closure = make_on_audio(
            spk_prod,
            spk_errored,
            output_resampler,
            OUTPUT_RESAMPLER_CHUNK_IN,
            /* need_output_resample */ false,
        );

        // End-of-turn flush sentinel: empty Vec, no resampling. With
        // no resampler tail to drain, this must not produce any output.
        closure(Vec::new());

        assert_eq!(
            spk_cons.occupied_len(),
            0,
            "flush w/o resampling must not push samples"
        );
    }

    #[test]
    fn make_on_audio_bails_when_speaker_errored_flag_is_set_before_call() {
        let Fixture {
            spk_prod,
            spk_cons,
            spk_errored,
            output_resampler,
        } = setup();
        spk_errored.store(true, Ordering::SeqCst);

        let mut closure = make_on_audio(
            spk_prod,
            Arc::clone(&spk_errored),
            output_resampler,
            OUTPUT_RESAMPLER_CHUNK_IN,
            /* need_output_resample */ false,
        );

        let samples: Vec<f32> = (0..32).map(|i| i as f32).collect();
        closure(samples);

        // push_all_with_bail bails on the very first iteration when
        // errored is already set — nothing reaches the ringbuf.
        assert_eq!(
            spk_cons.occupied_len(),
            0,
            "bail before pushing when errored is already set"
        );
    }
}
