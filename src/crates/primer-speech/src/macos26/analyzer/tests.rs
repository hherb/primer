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
}

impl ScriptedPipeline {
    fn new(script: Vec<(ResultEvent, String)>) -> Self {
        Self {
            script: script.into_iter().collect(),
            last_text: String::new(),
        }
    }
}

impl PipelineSource for ScriptedPipeline {
    fn feed_audio(&mut self, _samples: Vec<f32>) {}

    fn next_result(&mut self) -> Pin<Box<dyn Future<Output = ResultEvent> + Send + '_>> {
        Box::pin(async move {
            if let Some((event, text)) = self.script.pop_front() {
                self.last_text = text;
                event
            } else {
                // Hang so the inactivity tick arm wins the select.
                // Explicit type annotation guards against future
                // changes to `ResultEvent`'s shape producing a
                // confusing inference error here.
                std::future::pending::<ResultEvent>().await
            }
        })
    }

    fn last_result_text(&self) -> String {
        self.last_text.clone()
    }

    fn stop(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
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
    // attempts to send the synthetic-final. 150 ms gives ~115 ms of
    // slack on the ~35 ms minimum so a loaded CI runner doesn't
    // race the tick firing.
    tokio::time::sleep(Duration::from_millis(150)).await;

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

/// **Result-branch end-to-end coverage for #140.**
///
/// Locks in the result-branch behavioural envelope across an
/// utterance pair: volatile partial → real-final → new volatile +
/// tick-derived synthetic. Concrete invariants pinned:
///
/// 1. **Volatile partials and real-finals both reach `text_tx`**
///    with the correct `is_final` flag — the result branch's
///    TextMessage construction + send path is exercised.
/// 2. **`buf.record(&msg)` runs on every result.** Removing the
///    line (or moving it after a path that exits the loop early)
///    would mean no synthetic-final ever arrives after the third
///    volatile — the trailing `(true, "next")` row drops out.
/// 3. **The state machine cleanly re-enters `Speaking` after a
///    real-final**, so a subsequent volatile + tick still produces
///    SpeechStart + SpeechEnd via the inactivity path. The event
///    sequence assertion locks in the alternation.
///
/// `PartialBuffer`'s narrow `is_final`-clears-buffer behaviour is
/// pinned at the unit-test layer by
/// `real_final_clears_buffer_to_prevent_duplicate_synthesis` (a
/// few lines above); this orchestration test sits one layer up.
#[tokio::test]
async fn real_final_clears_buffer_so_next_synthetic_uses_fresh_partial() {
    let pipeline = ScriptedPipeline::new(vec![
        (result_event(false, false), "hi".to_string()),
        (result_event(true, false), "hi world".to_string()),
        (result_event(false, false), "next".to_string()),
    ]);
    let (audio_tx, audio_rx) = mpsc::channel::<Vec<f32>>(8);
    let (text_tx, mut text_rx) = mpsc::channel::<TextMessage>(16);
    let (event_tx, mut event_rx) = mpsc::channel::<VadEvent>(16);

    let handle = tokio::spawn(run_consumer_loop_with_config(
        pipeline,
        audio_rx,
        text_tx,
        event_tx,
        fast_test_config(),
    ));

    // Wait for the three scripted results to flow AND the inactivity
    // timer (30 ms past the "next" partial) to fire its synthetic.
    // 150 ms covers ~3 result-branch iterations (<5 ms each) + the
    // 30 ms inactivity window + ~115 ms slack for loaded CI.
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Drain text_rx. Expected (in order):
    //   volatile "hi", real-final "hi world", volatile "next",
    //   synthetic-final "next" (NOT "hi" — the load-bearing check).
    let mut texts = Vec::new();
    while let Ok(msg) = text_rx.try_recv() {
        texts.push((msg.is_final, msg.segment.text));
    }
    assert_eq!(
        texts,
        vec![
            (false, "hi".to_string()),
            (true, "hi world".to_string()),
            (false, "next".to_string()),
            (true, "next".to_string()),
        ],
        "#140: buf.record must clear on is_final so the next tick-derived synthetic uses the fresh partial, NOT the stale one"
    );

    // Drain event_rx. Expected: SpeechStart (from "hi"),
    // SpeechEnd (from real "hi world"), SpeechStart (from "next"),
    // SpeechEnd (from inactivity tick after "next").
    let mut events = Vec::new();
    while let Ok(ev) = event_rx.try_recv() {
        events.push(ev);
    }
    assert_eq!(
        events,
        vec![
            VadEvent::SpeechStart,
            VadEvent::SpeechEnd,
            VadEvent::SpeechStart,
            VadEvent::SpeechEnd,
        ],
        "VadEvent sequence must alternate cleanly across the real-final + restart"
    );

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
        run_consumer_loop_with_config(pipeline, audio_rx, text_tx, event_tx, fast_test_config()),
    )
    .await
    .expect("consumer loop did not exit within 200 ms of stream_done");
    assert!(
        result.is_ok(),
        "stream_done should yield Ok(()), got {result:?}"
    );
}

/// Sanity test: closing the audio mpsc terminates the consumer
/// loop cleanly via the `audio_rx.recv() = None` branch. Pins the
/// second documented exit path.
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
        run_consumer_loop_with_config(pipeline, audio_rx, text_tx, event_tx, fast_test_config()),
    )
    .await
    .expect("consumer loop did not exit within 200 ms of audio close");
    assert!(
        result.is_ok(),
        "audio close should yield Ok(()), got {result:?}"
    );
}
