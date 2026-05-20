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
    let mut sm = DerivedVadStateMachine::new();
    let mut tick = tokio::time::interval(EVENT_POLL_INTERVAL);
    // Skip the immediate first tick that tokio fires at t=0.
    tick.tick().await;

    loop {
        tokio::select! {
            // Audio in from the mic thread.
            samples = audio_rx.recv() => {
                match samples {
                    Some(s) => pipeline.feed_audio(s),
                    None => {
                        // Mic gone — stop the pipeline gracefully.
                        pipeline.stop().await;
                        return Ok(());
                    }
                }
            }
            // Results out from Swift. The Swift wrapper returns a sentinel
            // ResultEvent with stream_done=true to signal end-of-stream.
            event = pipeline.next_result() => {
                if event.stream_done {
                    return Ok(());
                }
                let now = Instant::now();
                if let Some(ev) = sm.on_result(&event.text, event.is_final, now) {
                    if event_tx.try_send(ev).is_err() {
                        tracing::warn!(
                            target: "primer::speech::macos26",
                            "vad event channel full; dropped {:?}", ev
                        );
                    }
                }
                let msg = TextMessage {
                    segment: TranscriptSegment {
                        text: event.text,
                        start_ms: event.range_start_ms,
                        end_ms: event.range_end_ms,
                    },
                    is_final: event.is_final,
                };
                if text_tx.send(msg).await.is_err() {
                    // Downstream gone — exit cleanly.
                    return Ok(());
                }
            }
            // Inactivity-timer tick.
            _ = tick.tick() => {
                if let Some(ev) = sm.tick(Instant::now()) {
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
