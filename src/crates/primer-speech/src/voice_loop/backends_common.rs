//! Shared backend types for the voice loop.
//!
//! Holds [`LocalBackends`] (the bundle every builder returns) and
//! [`ChannelStt`] (the streaming-STT adapter that decouples the audio
//! capture thread from the voice loop). All concrete backend builders
//! — whisper+piper in [`super::backends`] and macOS-native in
//! [`super::backends_macos`] — share these types.
//!
//! Gated by the parent `voice_loop` module on `cpal` (the lowest common
//! denominator: every builder needs `MicCapture`/`SpeakerSink`), so the
//! macOS builders don't have to inherit `whisper`/`piper`/`silero`
//! dependencies they never touch.

use std::sync::Arc;

use primer_core::error::{PrimerError, Result};
use primer_core::speech::{Named, StreamingSpeechToText, TranscriptSegment, TranscriptionSession};

use crate::voice_loop::{DrainHook, LoopBackends};
use crate::{MicCapture, SpeakerSink};

/// Shared receiver for the audio-thread → voice-loop transcript channel.
/// `Arc<Mutex<...>>` because the `StreamingSpeechToText` trait hands out
/// sessions through `&self`; there is exactly one consumer.
pub(super) type TranscriptRx = Arc<std::sync::Mutex<std::sync::mpsc::Receiver<String>>>;

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
        const TRANSCRIPT_RECV_BACKOFF: std::time::Duration = std::time::Duration::from_millis(2);
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
    pub is_speaking: Arc<std::sync::atomic::AtomicBool>,
    // ── owned resources kept alive for the duration of the loop ──
    pub(super) audio_thread: Option<std::thread::JoinHandle<Result<()>>>,
    pub(super) stop_flag: Arc<std::sync::atomic::AtomicBool>,
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
