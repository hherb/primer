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

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use primer_core::consts::speech::macos26::{EVENT_POLL_INTERVAL, SPEECH_END_TIMEOUT};
use primer_core::error::Result;
use primer_core::speech::{TranscriptSegment, VadEvent};
use tokio::sync::mpsc;

use crate::macos26::bridge::{Macos26Pipeline, ResultEvent};
use crate::macos26::vad::DerivedVadStateMachine;

/// Abstraction over the Swift `Macos26Pipeline` so tests can drive
/// [`run_consumer_loop`] with a scripted mock instead of the live
/// `SpeechAnalyzer` sidecar (which would otherwise require macOS 26
/// hardware + a mic).
///
/// The shape mirrors the four methods [`run_consumer_loop`] actually
/// calls on the real pipeline. Returning `Pin<Box<dyn Future + Send>>`
/// rather than `impl Future + Send` (RPITIT) is deliberate: it keeps the
/// trait stable across rustc nightly's evolving `async fn in trait`
/// auto-trait inference and makes the `Send` bound explicit so a
/// regression in swift-bridge's future-send-ness shows up at the impl
/// site rather than at `tokio::spawn`.
///
/// Production impl: `Macos26Pipeline` (immediately below).
/// Test impls: see [`tests::ScriptedPipeline`].
pub(crate) trait PipelineSource: Send {
    /// Push captured PCM samples into the analyzer.
    fn feed_audio(&mut self, samples: Vec<f32>);

    /// Await the next analyzer result. The returned future must be
    /// cancel-safe under `tokio::select!` — a partially-awaited future
    /// dropped because another arm was ready must not lose the result.
    /// The Swift side achieves this via a cached single-flighted
    /// `Task<ResultEvent>` (see `Macos26PipelineImpl.swift`).
    fn next_result(&mut self) -> Pin<Box<dyn Future<Output = ResultEvent> + Send + '_>>;

    /// Fetch the transcript text associated with the most-recently
    /// awaited `next_result`. Sync to avoid the swift-bridge 0.1.x
    /// async-shared-struct-String bug; see `bridge.rs` for context.
    fn last_result_text(&self) -> String;

    /// Tear down the analyzer pipeline gracefully.
    fn stop(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

impl PipelineSource for Macos26Pipeline {
    fn feed_audio(&mut self, samples: Vec<f32>) {
        Macos26Pipeline::feed_audio(self, samples);
    }

    fn next_result(&mut self) -> Pin<Box<dyn Future<Output = ResultEvent> + Send + '_>> {
        Box::pin(Macos26Pipeline::next_result(self))
    }

    fn last_result_text(&self) -> String {
        Macos26Pipeline::last_result_text(self)
    }

    fn stop(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(Macos26Pipeline::stop(self))
    }
}

/// Tunable timings used by [`run_consumer_loop_with_config`].
///
/// The default is `(EVENT_POLL_INTERVAL, SPEECH_END_TIMEOUT)` from
/// `primer_core::consts::speech::macos26`. Tests shrink both so the
/// inactivity-derived `SpeechEnd` path fires in tens of milliseconds
/// instead of the production ~1 s window.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ConsumerConfig {
    pub tick_interval: Duration,
    pub inactivity_timeout: Duration,
}

impl Default for ConsumerConfig {
    fn default() -> Self {
        Self {
            tick_interval: EVENT_POLL_INTERVAL,
            inactivity_timeout: SPEECH_END_TIMEOUT,
        }
    }
}

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
    pipeline: Macos26Pipeline,
    audio_rx: mpsc::Receiver<Vec<f32>>,
    text_tx: mpsc::Sender<TextMessage>,
    event_tx: mpsc::Sender<VadEvent>,
) -> Result<()> {
    run_consumer_loop_with_config(
        pipeline,
        audio_rx,
        text_tx,
        event_tx,
        ConsumerConfig::default(),
    )
    .await
}

/// Generic, parameterised inner loop. Identical to [`run_consumer_loop`]
/// except the pipeline can be any [`PipelineSource`] and the tick /
/// inactivity timings can be shrunk for fast unit tests.
///
/// Production paths call [`run_consumer_loop`] (the thin wrapper above);
/// tests call this directly with a [`tests::ScriptedPipeline`] and a
/// short-timeout [`ConsumerConfig`].
pub(crate) async fn run_consumer_loop_with_config<P: PipelineSource>(
    mut pipeline: P,
    mut audio_rx: mpsc::Receiver<Vec<f32>>,
    text_tx: mpsc::Sender<TextMessage>,
    event_tx: mpsc::Sender<VadEvent>,
    cfg: ConsumerConfig,
) -> Result<()> {
    tracing::info!(target: "primer::speech::macos26", "consumer loop starting");
    let mut sm = DerivedVadStateMachine::with_inactivity_timeout(cfg.inactivity_timeout);
    let mut buf = PartialBuffer::new();
    let mut tick = tokio::time::interval(cfg.tick_interval);
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
mod tests;
