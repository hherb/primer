//! Rust-side driver for the Swift sidecar. Owns one `Macos26Pipeline`
//! and `select!`s over three sources:
//!   - incoming audio chunks from the audio thread (mpsc)
//!   - the next transcriber result from Swift (`pipeline.next_result()`)
//!   - a periodic tick that drives the inactivity timer in
//!     `DerivedVadStateMachine`
//!
//! Emits text segments on one mpsc and VadEvents on another. Stops when
//! the audio mpsc closes OR the Swift stream signals end-of-stream via
//! `ResultEvent.stream_done`.

#![cfg(all(target_vendor = "apple", feature = "macos-native-26"))]

use std::time::Instant;

use primer_core::consts::speech::macos26::EVENT_POLL_INTERVAL;
use primer_core::error::Result;
use primer_core::speech::{TranscriptSegment, VadEvent};
use tokio::sync::mpsc;

use crate::macos26::bridge::Macos26Pipeline;
use crate::macos26::vad::DerivedVadStateMachine;

/// Message pushed onto the text channel for each transcriber Result.
/// `ChannelStt` (in voice_loop) consumes these to feed the streaming
/// STT trait surface.
#[derive(Debug, Clone)]
pub struct TextMessage {
    pub segment: TranscriptSegment,
    pub is_final: bool,
}

/// Buffers the most recent non-empty volatile partial so it can be
/// promoted to a synthetic final `TextMessage` when `SpeechEnd` is
/// derived from transcript inactivity (rather than from Swift emitting
/// `isFinal=true`).
///
/// Why this exists: `SpeechTranscriber` with `.progressiveTranscription`
/// preset typically only emits `isFinal=true` when the analyzer's input
/// stream is explicitly finalized (i.e. on full pipeline teardown).
/// Free-running audio with natural pauses produces only volatile
/// partials, which the bridge at `voice_loop::backends` drops on the
/// floor. Without this buffer the downstream `ChannelStt` consumer
/// would never see a transcript and `[child] ...` would never print.
#[derive(Debug, Default)]
pub struct PartialBuffer {
    latest: Option<TextMessage>,
}

impl PartialBuffer {
    pub fn new() -> Self {
        Self { latest: None }
    }

    /// Record an incoming `TextMessage`. Non-empty volatile partials
    /// replace the buffer; a real `is_final=true` message clears the
    /// buffer (the genuine final flows through the normal path, so we
    /// must not synthesize a duplicate at the next `SpeechEnd`).
    pub fn record(&mut self, msg: &TextMessage) {
        if msg.is_final {
            self.latest = None;
            return;
        }
        if !msg.segment.text.trim().is_empty() {
            self.latest = Some(msg.clone());
        }
    }

    /// Take the buffered partial, marked as final. Returns `None` when
    /// the buffer is empty (e.g. SpeechEnd derived without any prior
    /// non-empty partial — caller skips the synthetic emit). Clears
    /// the buffer.
    pub fn take_synthetic_final(&mut self) -> Option<TextMessage> {
        let mut msg = self.latest.take()?;
        msg.is_final = true;
        Some(msg)
    }
}

/// Drive the Macos26Pipeline pull-loop. Returns when either the audio
/// channel closes (mic shut down) or the Swift stream signals
/// `stream_done == true`.
///
/// The `pipeline` is moved in; the caller does not retain access.
/// `audio_rx` is the inbound 16 kHz mono Float32 audio (Vec<f32> per
/// chunk; chunk size is up to the producer). `text_tx` and `event_tx`
/// are the outbound channels consumed by voice_loop.
pub async fn run_consumer_loop(
    mut pipeline: Macos26Pipeline,
    mut audio_rx: mpsc::Receiver<Vec<f32>>,
    text_tx: mpsc::Sender<TextMessage>,
    event_tx: mpsc::Sender<VadEvent>,
) -> Result<()> {
    tracing::info!(target: "primer::speech::macos26", "consumer loop starting");
    let mut sm = DerivedVadStateMachine::new();
    let mut buf = PartialBuffer::new();
    let mut tick = tokio::time::interval(EVENT_POLL_INTERVAL);
    // Skip the immediate first tick that tokio fires at t=0.
    tick.tick().await;
    let mut audio_chunks_received: u64 = 0;
    let mut results_received: u64 = 0;

    loop {
        tokio::select! {
            // Audio in from the mic thread.
            samples = audio_rx.recv() => {
                match samples {
                    Some(s) => {
                        audio_chunks_received += 1;
                        if audio_chunks_received == 1 {
                            tracing::info!(
                                target: "primer::speech::macos26",
                                "first audio chunk reached consumer (len={})", s.len()
                            );
                        }
                        pipeline.feed_audio(s);
                    }
                    None => {
                        // Mic gone — stop the pipeline gracefully.
                        tracing::info!(
                            target: "primer::speech::macos26",
                            "audio_rx closed; stopping pipeline (chunks={} results={})",
                            audio_chunks_received, results_received
                        );
                        pipeline.stop().await;
                        return Ok(());
                    }
                }
            }
            // Results out from Swift. The Swift wrapper returns a sentinel
            // ResultEvent with stream_done=true to signal end-of-stream.
            event = pipeline.next_result() => {
                if event.stream_done {
                    tracing::info!(
                        target: "primer::speech::macos26",
                        "Swift stream ended (stream_done=true), chunks={} results={}",
                        audio_chunks_received, results_received
                    );
                    return Ok(());
                }
                // Text comes via a separate sync accessor — see bridge.rs
                // for the swift-bridge 0.1.x String-in-async-struct bug.
                // Must call BEFORE next pipeline.next_result() since the
                // Swift side overwrites lastText on each iteration.
                let text = pipeline.last_result_text();
                results_received += 1;
                if results_received <= 3 || results_received % 50 == 0 {
                    tracing::info!(
                        target: "primer::speech::macos26",
                        "result #{} is_final={} text={:?}",
                        results_received, event.is_final, text
                    );
                }
                let now = Instant::now();
                if let Some(ev) = sm.on_result(&text, event.is_final, now) {
                    if event_tx.try_send(ev).is_err() {
                        tracing::warn!(
                            target: "primer::speech::macos26",
                            "vad event channel full; dropped {:?}", ev
                        );
                    }
                }
                let msg = TextMessage {
                    segment: TranscriptSegment {
                        text,
                        start_ms: event.range_start_ms,
                        end_ms: event.range_end_ms,
                    },
                    is_final: event.is_final,
                };
                buf.record(&msg);
                if text_tx.send(msg).await.is_err() {
                    // Downstream gone — exit cleanly.
                    return Ok(());
                }
            }
            // Inactivity-timer tick.
            _ = tick.tick() => {
                if let Some(ev) = sm.tick(Instant::now()) {
                    // SpeechEnd derived from inactivity: SpeechTranscriber
                    // hasn't emitted isFinal=true (and likely won't until
                    // pipeline teardown), so promote the latest buffered
                    // volatile partial to a synthetic final TextMessage
                    // BEFORE forwarding the VAD event. Order matters:
                    // ChannelStt must see the transcript first so the
                    // voice loop can pair it with the SpeechEnd that
                    // arrives moments later.
                    if matches!(ev, VadEvent::SpeechEnd) {
                        if let Some(final_msg) = buf.take_synthetic_final() {
                            if text_tx.send(final_msg).await.is_err() {
                                return Ok(());
                            }
                        }
                    }
                    if event_tx.try_send(ev).is_err() {
                        tracing::warn!(
                            target: "primer::speech::macos26",
                            "vad event channel full; dropped {:?}", ev
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(text: &str, is_final: bool) -> TextMessage {
        TextMessage {
            segment: TranscriptSegment {
                text: text.to_string(),
                start_ms: 0,
                end_ms: 0,
            },
            is_final,
        }
    }

    #[test]
    fn buffer_starts_empty() {
        let mut b = PartialBuffer::new();
        assert!(b.take_synthetic_final().is_none());
    }

    #[test]
    fn buffer_records_non_empty_volatile_partial() {
        let mut b = PartialBuffer::new();
        b.record(&msg("hello", false));
        let out = b
            .take_synthetic_final()
            .expect("buffer should have a partial");
        assert_eq!(out.segment.text, "hello");
        assert!(out.is_final, "synthetic emit must be marked final");
    }

    #[test]
    fn buffer_skips_empty_and_whitespace_partials() {
        let mut b = PartialBuffer::new();
        b.record(&msg("", false));
        b.record(&msg("   ", false));
        assert!(b.take_synthetic_final().is_none());
    }

    #[test]
    fn later_partial_replaces_earlier() {
        let mut b = PartialBuffer::new();
        b.record(&msg("hello", false));
        b.record(&msg("hello world", false));
        let out = b.take_synthetic_final().unwrap();
        assert_eq!(out.segment.text, "hello world");
    }

    #[test]
    fn empty_partial_does_not_clear_existing_buffer() {
        // A whitespace-only volatile result mid-utterance shouldn't
        // erase the previously buffered text — only a real isFinal
        // clears it.
        let mut b = PartialBuffer::new();
        b.record(&msg("hello", false));
        b.record(&msg("", false));
        let out = b.take_synthetic_final().unwrap();
        assert_eq!(out.segment.text, "hello");
    }

    #[test]
    fn real_final_clears_buffer_to_prevent_duplicate_synthesis() {
        // When Swift does emit a genuine isFinal=true, that message
        // flows through the normal path. The buffer must be cleared
        // so the next tick-derived SpeechEnd doesn't synthesize a
        // stale duplicate.
        let mut b = PartialBuffer::new();
        b.record(&msg("hello", false));
        b.record(&msg("hello world", true));
        assert!(b.take_synthetic_final().is_none());
    }

    #[test]
    fn take_is_idempotent() {
        let mut b = PartialBuffer::new();
        b.record(&msg("hi", false));
        assert!(b.take_synthetic_final().is_some());
        // Second take after the buffer was drained returns None.
        assert!(b.take_synthetic_final().is_none());
    }
}
