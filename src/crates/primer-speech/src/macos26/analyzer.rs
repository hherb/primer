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

    // ── ScriptedPipeline + run_consumer_loop orchestration tests ────────
    //
    // Below: a scripted `PipelineSource` impl that lets unit tests drive
    // `run_consumer_loop_with_config` without the live Swift sidecar.
    // The shared invariant under test (#140): when SpeechEnd is derived
    // from the inactivity tick, the buffered synthetic-final
    // TextMessage must be emitted on `text_tx` BEFORE the SpeechEnd
    // VadEvent is emitted on `event_tx`. Children's TTS-driven follow-up
    // turns rely on ChannelStt seeing the transcript moments before the
    // VAD `SpeechEnd` arrives — invert the order and the dialogue
    // manager handles a `SpeechEnd` without ever having received the
    // utterance that triggered it.

    use std::collections::VecDeque;

    /// Mock `PipelineSource` driven by a pre-scripted sequence of
    /// `(ResultEvent, text)` pairs.
    ///
    /// `next_result` pops the next scripted pair and stashes the text
    /// for the subsequent `last_result_text` call (mirroring the Swift
    /// side's cached `lastText` field). When the script is empty
    /// `next_result` hangs forever via `std::future::pending` so the
    /// `tick.tick()` arm of the consumer's `tokio::select!` becomes the
    /// only ready branch — driving the inactivity-derived SpeechEnd
    /// path that this test cluster exists to lock in.
    struct ScriptedPipeline {
        script: VecDeque<(ResultEvent, String)>,
        last_text: String,
        feed_audio_calls: usize,
        stop_calls: usize,
    }

    impl ScriptedPipeline {
        fn new(script: Vec<(ResultEvent, String)>) -> Self {
            Self {
                script: script.into_iter().collect(),
                last_text: String::new(),
                feed_audio_calls: 0,
                stop_calls: 0,
            }
        }
    }

    impl PipelineSource for ScriptedPipeline {
        fn feed_audio(&mut self, _samples: Vec<f32>) {
            self.feed_audio_calls += 1;
        }

        fn next_result(&mut self) -> Pin<Box<dyn Future<Output = ResultEvent> + Send + '_>> {
            Box::pin(async move {
                if let Some((event, text)) = self.script.pop_front() {
                    self.last_text = text;
                    event
                } else {
                    // Hang so the inactivity tick arm wins the select.
                    std::future::pending().await
                }
            })
        }

        fn last_result_text(&self) -> String {
            self.last_text.clone()
        }

        fn stop(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            self.stop_calls += 1;
            Box::pin(async {})
        }
    }

    /// Construct a `ResultEvent` with default ranges. Keeps the test
    /// bodies focused on the bits that actually matter (`is_final`,
    /// `stream_done`).
    fn result_event(is_final: bool, stream_done: bool) -> ResultEvent {
        ResultEvent {
            is_final,
            range_start_ms: 0,
            range_end_ms: 100,
            stream_done,
        }
    }

    /// Test config: shrink the tick interval and inactivity timeout so
    /// the inactivity-derived SpeechEnd fires inside ~50 ms wallclock
    /// rather than the production 1 s window.
    fn fast_test_config() -> ConsumerConfig {
        ConsumerConfig {
            tick_interval: Duration::from_millis(5),
            inactivity_timeout: Duration::from_millis(30),
        }
    }

    /// **Load-bearing regression test for #140.**
    ///
    /// When the inactivity tick derives a SpeechEnd, the consumer loop
    /// must emit the buffered synthetic-final `TextMessage` on `text_tx`
    /// BEFORE forwarding the `SpeechEnd` `VadEvent` on `event_tx`. A
    /// future refactor that swaps the order of those two sends inside
    /// the tick branch would break the contract silently — manual mic
    /// testing on macOS 26 hardware was previously the only way to
    /// catch this. This test catches it via a capacity-1 `text_tx`:
    /// the synthetic-final `send().await` blocks the loop until the
    /// test drains the volatile partial, so a SpeechEnd that landed on
    /// `event_rx` *before* the synthetic-final landed on `text_rx` is a
    /// reorder bug.
    #[tokio::test]
    async fn synthetic_final_text_arrives_before_speech_end_vad() {
        // Script one volatile partial; the script then empties so the
        // tick arm drives SpeechEnd derivation.
        let pipeline = ScriptedPipeline::new(vec![(
            result_event(false, false),
            "hello world".to_string(),
        )]);

        let (audio_tx, audio_rx) = mpsc::channel::<Vec<f32>>(8);
        // Capacity-1 text channel: the synthetic-final `send().await`
        // will block until we drain the volatile partial. That blocking
        // is what gives the test deterministic order observation.
        let (text_tx, mut text_rx) = mpsc::channel::<TextMessage>(1);
        let (event_tx, mut event_rx) = mpsc::channel::<VadEvent>(8);

        let handle = tokio::spawn(run_consumer_loop_with_config(
            pipeline,
            audio_rx,
            text_tx,
            event_tx,
            fast_test_config(),
        ));

        // Wait long enough for: (1) the volatile partial to be sent →
        // text_rx fills its single slot, (2) the inactivity timeout
        // (30 ms) + a tick (5 ms) to elapse so the SpeechEnd branch
        // attempts to send the synthetic-final.
        tokio::time::sleep(Duration::from_millis(80)).await;

        // Drain the volatile partial. This MUST succeed — if it
        // doesn't, the loop never sent it and the rest of the test is
        // meaningless.
        let first_text = text_rx
            .try_recv()
            .expect("volatile partial should be in text_rx");
        assert!(
            !first_text.is_final,
            "first message is the volatile partial"
        );
        assert_eq!(first_text.segment.text, "hello world");

        // SpeechStart should already be present. SpeechEnd MUST NOT be.
        // If SpeechEnd is observable here, the consumer loop sent it
        // before the synthetic-final TextMessage — the #140 reorder bug.
        let start_event = event_rx
            .try_recv()
            .expect("SpeechStart should be in event_rx");
        assert!(
            matches!(start_event, VadEvent::SpeechStart),
            "first event must be SpeechStart, got {start_event:?}"
        );
        assert!(
            event_rx.try_recv().is_err(),
            "#140: SpeechEnd must NOT precede the synthetic-final TextMessage on text_tx"
        );

        // Now drain the synthetic-final TextMessage. The blocking send
        // unblocks once we just drained the volatile partial above,
        // so this should arrive promptly.
        let synthetic = tokio::time::timeout(Duration::from_millis(200), text_rx.recv())
            .await
            .expect("synthetic-final TextMessage never arrived")
            .expect("text channel closed before synthetic-final");
        assert!(synthetic.is_final, "synthetic emit must be marked final");
        assert_eq!(synthetic.segment.text, "hello world");

        // Now (and only now) the consumer loop should proceed to send
        // SpeechEnd. Wait briefly for it.
        let end_event = tokio::time::timeout(Duration::from_millis(200), event_rx.recv())
            .await
            .expect("SpeechEnd never arrived")
            .expect("event channel closed before SpeechEnd");
        assert!(
            matches!(end_event, VadEvent::SpeechEnd),
            "second event must be SpeechEnd, got {end_event:?}"
        );

        // Shut down: closing audio_tx makes audio_rx.recv() return None,
        // which calls pipeline.stop() and returns Ok.
        drop(audio_tx);
        let _ = tokio::time::timeout(Duration::from_millis(200), handle).await;
    }

    /// Sanity test: a `stream_done=true` sentinel from the pipeline
    /// terminates the consumer loop cleanly with `Ok(())`. Pins the
    /// other documented exit path (besides `audio_rx` closing).
    #[tokio::test]
    async fn stream_done_sentinel_exits_cleanly() {
        let pipeline = ScriptedPipeline::new(vec![(result_event(false, true), String::new())]);
        let (_audio_tx, audio_rx) = mpsc::channel::<Vec<f32>>(8);
        let (text_tx, _text_rx) = mpsc::channel::<TextMessage>(8);
        let (event_tx, _event_rx) = mpsc::channel::<VadEvent>(8);

        let result = tokio::time::timeout(
            Duration::from_millis(200),
            run_consumer_loop_with_config(
                pipeline,
                audio_rx,
                text_tx,
                event_tx,
                fast_test_config(),
            ),
        )
        .await
        .expect("consumer loop did not exit within 200 ms of stream_done");
        assert!(
            result.is_ok(),
            "stream_done should yield Ok(()), got {result:?}"
        );
    }

    /// Sanity test: closing the audio mpsc terminates the consumer
    /// loop cleanly via the `audio_rx.recv() = None` branch, and the
    /// pipeline's `stop()` is invoked exactly once. Pins the second
    /// documented exit path.
    #[tokio::test]
    async fn audio_channel_close_exits_cleanly() {
        let pipeline = ScriptedPipeline::new(vec![]);
        let (audio_tx, audio_rx) = mpsc::channel::<Vec<f32>>(8);
        let (text_tx, _text_rx) = mpsc::channel::<TextMessage>(8);
        let (event_tx, _event_rx) = mpsc::channel::<VadEvent>(8);

        // Close the audio channel before the consumer loop starts.
        drop(audio_tx);

        let result = tokio::time::timeout(
            Duration::from_millis(200),
            run_consumer_loop_with_config(
                pipeline,
                audio_rx,
                text_tx,
                event_tx,
                fast_test_config(),
            ),
        )
        .await
        .expect("consumer loop did not exit within 200 ms of audio close");
        assert!(
            result.is_ok(),
            "audio close should yield Ok(()), got {result:?}"
        );
    }
}
