//! Audio capture thread and channel-based STT adapter.
//!
//! `ChannelStt` / `ChannelSttSession` decouple the audio capture thread
//! (which owns the real `WhisperStream` and pushes mic samples into it)
//! from the voice-loop state machine (which only reads `VadEvent`s and
//! calls `finalize` on the session it opened).
//!
//! `run_audio_thread` is the body of the `std::thread` that drives the
//! LISTEN-side pipeline: raw mic → resample → Silero VAD → Whisper push.
//! It stays in `primer-cli` because it owns `cpal`/`ringbuf` I/O, which
//! is a CLI-specific concern.

use std::sync::Arc;

use primer_core::error::Result;
use primer_core::speech::StreamingSpeechToText;

/// Shared receiver for the audio thread → voice-loop transcript channel.
/// `Arc<Mutex<...>>` only because the `StreamingSpeechToText` trait hands
/// out sessions through `&self`; there is exactly one consumer.
#[cfg(feature = "speech")]
pub(super) type TranscriptRx = Arc<std::sync::Mutex<std::sync::mpsc::Receiver<String>>>;

/// `StreamingSpeechToText` adapter: hands out sessions whose `finalize`
/// pulls a transcript from a `std::sync::mpsc` channel populated by the
/// audio capture thread. This decouples the audio thread (which owns
/// the real `WhisperStream` and pushes mic samples into it) from
/// `run_loop_borrowed` (which only reads `VadEvent`s and calls `finalize`
/// on the session it opened).
///
/// **Ordering contract:** the audio thread MUST send the transcript on
/// `tx` BEFORE emitting `VadEvent::SpeechEnd` on the event channel. That
/// way, by the time the voice loop calls `session.finalize()` (after
/// seeing `SpeechEnd`), the transcript is already buffered in the channel.
#[cfg(feature = "speech")]
pub(super) struct ChannelStt {
    pub(super) rx: TranscriptRx,
}

#[cfg(feature = "speech")]
impl primer_core::speech::Named for ChannelStt {
    fn name(&self) -> &str {
        "channel-stt"
    }
}

#[cfg(feature = "speech")]
impl StreamingSpeechToText for ChannelStt {
    fn sample_rate(&self) -> u32 {
        16_000
    }
    fn open_session(&self) -> Result<Box<dyn primer_core::speech::TranscriptionSession>> {
        Ok(Box::new(ChannelSttSession {
            rx: Arc::clone(&self.rx),
        }))
    }
}

#[cfg(feature = "speech")]
struct ChannelSttSession {
    rx: TranscriptRx,
}

#[cfg(feature = "speech")]
impl primer_core::speech::TranscriptionSession for ChannelSttSession {
    fn push_audio(
        &mut self,
        _samples: &[f32],
    ) -> Result<Vec<primer_core::speech::TranscriptSegment>> {
        // run_loop_borrowed never calls push_audio on this session
        // (samples are pushed directly into whisper inside the audio
        // thread). If someone wires it up differently this is a silent
        // no-op.
        Ok(vec![])
    }
    fn finalize(self: Box<Self>) -> Result<Vec<primer_core::speech::TranscriptSegment>> {
        // The audio thread sends BEFORE emitting SpeechEnd, so by the
        // time the voice loop calls us the transcript is normally already
        // buffered. Spin try_recv briefly to tolerate scheduling
        // jitter — but fail loudly rather than blocking, so a contract
        // violation surfaces as a quick "empty utterance" instead of a
        // half-second stall in the async runtime.
        let rx = self.rx.lock().map_err(|_| {
            primer_core::error::PrimerError::Speech(
                "ChannelSttSession: transcript receiver mutex poisoned".into(),
            )
        })?;
        const TRANSCRIPT_RECV_RETRIES: u32 = 5;
        const TRANSCRIPT_RECV_BACKOFF: std::time::Duration = std::time::Duration::from_millis(2);
        for _ in 0..TRANSCRIPT_RECV_RETRIES {
            match rx.try_recv() {
                Ok(text) => {
                    return Ok(vec![primer_core::speech::TranscriptSegment {
                        text,
                        start_ms: 0,
                        end_ms: 0,
                    }]);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    std::thread::sleep(TRANSCRIPT_RECV_BACKOFF);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err(primer_core::error::PrimerError::Speech(
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

/// Body of the audio capture thread.
///
/// Pulls mic samples from `mic_cons`, resamples to the VAD rate when
/// needed, runs the VAD on fixed-size chunks, and drives the per-
/// utterance whisper session: open on `SpeechStart`, push every chunk
/// while in speech, finalize on `SpeechEnd` and SEND the transcript on
/// `transcript_tx` BEFORE forwarding the `SpeechEnd` event on
/// `event_tx`. That ordering guarantees `ChannelSttSession::finalize`
/// (called by the voice loop after seeing the event) finds a transcript.
///
/// Polling cadence: a 5 ms sleep between empty mic-buffer reads. cpal's
/// callback fires every few ms, so this stays well within real-time
/// tolerances.
#[cfg(feature = "speech")]
#[allow(clippy::too_many_arguments)]
pub(super) fn run_audio_thread(
    mut mic_cons: ringbuf::HeapCons<f32>,
    mut input_resampler: Option<primer_speech::Resampler>,
    in_chunk_samples: usize,
    vad_chunk_samples: usize,
    vad: &mut primer_speech::SileroVad,
    whisper: std::sync::Arc<primer_speech::WhisperStt>,
    event_tx: tokio::sync::mpsc::Sender<primer_core::speech::VadEvent>,
    transcript_tx: std::sync::mpsc::Sender<String>,
    stop_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    is_speaking: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    use primer_core::speech::{StreamingSpeechToText, TranscriptionSession, VadEvent};
    use primer_core::speech::VoiceActivityDetector;
    use ringbuf::traits::Consumer as _;

    let mut raw_buf: Vec<f32> = Vec::with_capacity(in_chunk_samples * 2);
    let mut vad_in_buf: Vec<f32> = Vec::with_capacity(vad_chunk_samples * 2);
    // Reusable scratch buffer for the per-iteration VAD chunk; avoids
    // allocating ~32 Vecs/sec for the hot path.
    let mut vad_chunk: Vec<f32> = Vec::with_capacity(vad_chunk_samples);
    // The active whisper session, if we're in speech.
    let mut active_session: Option<Box<dyn TranscriptionSession>> = None;

    loop {
        if stop_flag.load(std::sync::atomic::Ordering::SeqCst) {
            // Drain whisper if mid-utterance.
            if let Some(s) = active_session.take() {
                let _ = s.finalize();
            }
            return Ok(());
        }

        // Anti-feedback gate: while the voice loop is in SPEAK, drop
        // everything the mic captures (the Primer's own voice would
        // otherwise be transcribed). Also discard any partial whisper
        // session; we'll open a fresh one when the gate reopens.
        if is_speaking.load(std::sync::atomic::Ordering::SeqCst) {
            while mic_cons.try_pop().is_some() {}
            if let Some(s) = active_session.take() {
                let _ = s.finalize();
            }
            raw_buf.clear();
            vad_in_buf.clear();
            // Reset VAD's internal speech-state debouncer so we don't
            // emit a stale SpeechEnd as soon as the gate reopens.
            vad.reset();
            std::thread::sleep(std::time::Duration::from_millis(20));
            continue;
        }

        // Drain available samples from the mic ringbuf.
        let mut produced_any = false;
        while let Some(s) = mic_cons.try_pop() {
            raw_buf.push(s);
            produced_any = true;
            if raw_buf.len() >= 8192 {
                // Hard cap: don't let the buffer balloon if we're
                // resampling unevenly.
                break;
            }
        }
        if !produced_any {
            std::thread::sleep(std::time::Duration::from_millis(5));
            continue;
        }

        // Convert raw_buf into VAD-rate chunks.
        if let Some(resampler) = input_resampler.as_mut() {
            // Process exactly `in_chunk_samples`-sized blocks.
            let usable = (raw_buf.len() / in_chunk_samples) * in_chunk_samples;
            let mut consumed = 0;
            while consumed + in_chunk_samples <= usable {
                let block = &raw_buf[consumed..consumed + in_chunk_samples];
                match resampler.process(block) {
                    Ok(out) => vad_in_buf.extend(out),
                    Err(e) => {
                        tracing::warn!("input resampler error: {e}");
                        // Skip the block.
                    }
                }
                consumed += in_chunk_samples;
            }
            // Drop consumed samples from raw_buf.
            raw_buf.drain(..consumed);
        } else {
            // Native rate already matches VAD rate.
            vad_in_buf.append(&mut raw_buf);
        }

        // Process VAD chunks while we have enough samples.
        while vad_in_buf.len() >= vad_chunk_samples {
            vad_chunk.clear();
            vad_chunk.extend(vad_in_buf.drain(..vad_chunk_samples));
            // Push into whisper if we have an active session.
            if let Some(session) = active_session.as_mut() {
                if let Err(e) = session.push_audio(&vad_chunk) {
                    tracing::warn!("whisper push_audio: {e}");
                }
            }
            let frame = match vad.process_chunk(&vad_chunk) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("vad process_chunk: {e}");
                    continue;
                }
            };
            match frame.event {
                VadEvent::SpeechStart => {
                    // Open a fresh whisper session if one isn't already open.
                    if active_session.is_none() {
                        match whisper.open_session() {
                            Ok(s) => active_session = Some(s),
                            Err(e) => {
                                tracing::warn!("whisper open_session: {e}");
                            }
                        }
                    }
                    if event_tx.blocking_send(VadEvent::SpeechStart).is_err() {
                        // Voice loop has dropped; exit thread.
                        return Ok(());
                    }
                }
                VadEvent::SpeechEnd => {
                    // Finalize whisper, send transcript BEFORE the event.
                    let segments = match active_session.take() {
                        Some(s) => match s.finalize() {
                            Ok(segs) => segs,
                            Err(e) => {
                                tracing::warn!("whisper finalize: {e}");
                                Vec::new()
                            }
                        },
                        None => Vec::new(),
                    };
                    let text: String = segments
                        .iter()
                        .map(|s| s.text.as_str())
                        .collect::<Vec<_>>()
                        .join("")
                        .trim()
                        .to_string();
                    if transcript_tx.send(text).is_err() {
                        return Ok(());
                    }
                    if event_tx.blocking_send(VadEvent::SpeechEnd).is_err() {
                        return Ok(());
                    }
                }
                VadEvent::None => {}
            }
        }
    }
}
