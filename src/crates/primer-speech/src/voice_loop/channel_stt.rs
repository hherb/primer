//! Channel-backed `StreamingSpeechToText` adapter. Decouples whatever
//! produces final transcripts (a cpal+whisper audio thread, the macOS-26
//! Swift pipeline, or the Android `SpeechRecognizer` consumer) from the
//! voice-loop state machine, which only needs `finalize()` to yield the
//! next utterance. Pure `std::sync::mpsc` — no cpal — so it is reachable
//! from the cpal-free `android-native` build.
//!
//! Moved out of `backends_common` (which is cpal-gated) in Plan 2 Task 4
//! so the Android STT can reuse it verbatim (DRY).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use primer_core::error::{PrimerError, Result};
use primer_core::speech::{Named, StreamingSpeechToText, TranscriptSegment, TranscriptionSession};

/// Shared receiver for the producer → voice-loop transcript channel.
/// `Arc<Mutex<...>>` because the `StreamingSpeechToText` trait hands out
/// sessions through `&self`; there is exactly one consumer.
pub(super) type TranscriptRx = Arc<Mutex<std::sync::mpsc::Receiver<String>>>;

/// `StreamingSpeechToText` adapter: hands out sessions whose `finalize`
/// pulls a transcript from a `std::sync::mpsc` channel populated by the
/// transcript producer (audio capture thread, Swift pipeline, or Android
/// recognizer consumer). Decouples the producer (which owns the real STT
/// session) from the voice loop (which only reads `VadEvent`s and calls
/// `finalize` on the session it opened).
///
/// **Ordering contract:** the producer MUST send the transcript on its
/// `Sender` BEFORE emitting `VadEvent::SpeechEnd` on the event channel. That
/// way, by the time the voice loop calls `session.finalize()` (after seeing
/// `SpeechEnd`), the transcript is already buffered in the channel.
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
        // The producer sends BEFORE emitting SpeechEnd, so by the time the
        // voice loop calls us the transcript is normally already buffered.
        // Spin try_recv briefly to tolerate scheduling jitter — but fail
        // loudly rather than blocking, so a contract violation surfaces as a
        // quick "empty utterance" instead of a half-second stall in the
        // async runtime.
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
