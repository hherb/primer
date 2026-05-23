//! Shared drain-loop helper for the macOS-native streaming TTS paths.
//!
//! Both `synthesize_streaming_main_thread` and
//! `synthesize_streaming_background` in [`super::tts`] drive an
//! `mpsc::channel` of [`SynthesisEvent`]s until `PhraseEnd` arrives or
//! the overall drain deadline expires. The two paths differ only in how
//! they wait between channel polls:
//!
//! - **Main-thread**: `try_recv` + `NSRunLoop.runUntilDate(slice)` — the
//!   runloop slice drains the GCD main queue, which is where
//!   AVSpeechSynthesizer's PCM callbacks are dispatched.
//! - **Background**: `recv_timeout(STREAM_DRAIN_POLL_MS)` — a blocking
//!   receive that the producer (on the GCD main queue) wakes by
//!   `Sender::send`.
//!
//! [`drive_streaming_drain`] absorbs that threading-model difference
//! into a single closure parameter: callers pass a `next_event` closure
//! that returns `Ok(Some(event))` when one is available, `Ok(None)`
//! when the wait step elapsed without yielding an event, or `Err(...)`
//! on a structural failure (e.g. channel disconnected). The helper
//! owns the deadline check, the `on_event` dispatch, and the
//! `PhraseEnd`-terminates-the-loop invariant.
//!
//! See [issue #124](https://github.com/hherb/primer/issues/124) for the
//! design rationale.

use std::time::Instant;

use primer_core::consts::speech::STREAM_DRAIN_TIMEOUT_SECS;
use primer_core::error::{PrimerError, Result};
use primer_core::speech::SynthesisEvent;

/// Drive the streaming drain loop using `next_event` as the
/// platform-specific wait step.
///
/// The loop:
/// 1. checks the deadline (returns `Speech` error if expired),
/// 2. calls `next_event()` to obtain the next event (or `None` if the
///    wait step elapsed without one),
/// 3. fires `on_event(event)` for each event,
/// 4. returns `Ok(())` when `SynthesisEvent::PhraseEnd` is observed.
///
/// `path_label` is interpolated into the timeout error message so the
/// two call sites can be distinguished in logs (`"main-thread streaming"`
/// vs `"background streaming"`).
///
/// # Errors
///
/// Returns `PrimerError::Speech` on deadline expiry, or whatever error
/// the `next_event` closure produces (e.g. channel disconnected on the
/// background path).
pub(super) fn drive_streaming_drain<F>(
    on_event: &mut dyn FnMut(SynthesisEvent),
    deadline: Instant,
    path_label: &str,
    mut next_event: F,
) -> Result<()>
where
    F: FnMut() -> Result<Option<SynthesisEvent>>,
{
    loop {
        if Instant::now() >= deadline {
            return Err(PrimerError::Speech(format!(
                "AVSpeechSynthesizer {STREAM_DRAIN_TIMEOUT_SECS}s drain timeout ({path_label})"
            )));
        }
        match next_event()? {
            Some(event) => {
                let is_phrase_end = matches!(event, SynthesisEvent::PhraseEnd);
                on_event(event);
                if is_phrase_end {
                    return Ok(());
                }
            }
            None => {
                // Wait step elapsed without an event — re-check deadline
                // and try again. This is the no-op branch that lets a
                // hung synth trip the sanity cap.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primer_core::speech::AudioChunk;
    use std::sync::mpsc::{RecvTimeoutError, channel};
    use std::thread;
    use std::time::Duration;

    /// Number of `Audio` events the structural tests send before
    /// `PhraseEnd`. Mirrors the
    /// `streaming_emits_multiple_audio_events_before_phrase_end`
    /// invariant from `tests/macos_tts.rs` — at least 2 audio chunks
    /// must reach the consumer before the phrase terminator.
    const AUDIO_EVENTS_PER_PHRASE: usize = 3;

    /// Marker sample-rate stamped onto each synthetic `AudioChunk` so
    /// tests can confirm the chunks arrived through the helper rather
    /// than coming from somewhere else. Arbitrary value; chosen to be
    /// distinct from real Apple voice sample rates (22 050 / 24 000).
    const TEST_MARKER_SAMPLE_RATE: u32 = 7_777;

    /// Conservative ceiling for the structural tests' real-time
    /// allowance. Each test sends a few hundred milliseconds of work;
    /// 5 s is comfortably larger than that and still small enough that
    /// a hang fails the test promptly.
    const STRUCTURAL_TEST_TIMEOUT_SECS: u64 = 5;

    fn make_audio_chunk(value: f32) -> SynthesisEvent {
        SynthesisEvent::Audio(AudioChunk {
            samples: vec![value],
            sample_rate: TEST_MARKER_SAMPLE_RATE,
        })
    }

    /// Background-path shape: a `recv_timeout`-style closure that
    /// returns `Ok(Some(_))` on data, `Ok(None)` on timeout, and
    /// `Err(...)` on disconnect. Verifies the helper emits every Audio
    /// event in order, terminates on PhraseEnd, and ignores anything
    /// the producer sends afterward.
    #[test]
    fn drives_recv_timeout_shape_until_phrase_end() {
        let (tx, rx) = channel::<SynthesisEvent>();

        let producer = thread::spawn(move || {
            for i in 0..AUDIO_EVENTS_PER_PHRASE {
                tx.send(make_audio_chunk(i as f32)).expect("send audio");
            }
            tx.send(SynthesisEvent::PhraseEnd).expect("send phrase end");
            // A late event the helper must NOT see (PhraseEnd already
            // terminated the loop). Sending after PhraseEnd models the
            // real-world case where AVSpeechSynthesizer's Block_retain
            // keeps the closure alive and may fire one more time.
            let _ = tx.send(make_audio_chunk(99.0));
        });

        let mut collected: Vec<SynthesisEvent> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(STRUCTURAL_TEST_TIMEOUT_SECS);
        let result = drive_streaming_drain(
            &mut |e| collected.push(e),
            deadline,
            "test-recv-timeout",
            || match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(event) => Ok(Some(event)),
                Err(RecvTimeoutError::Timeout) => Ok(None),
                Err(RecvTimeoutError::Disconnected) => Err(PrimerError::Speech(
                    "channel disconnected before PhraseEnd".into(),
                )),
            },
        );

        producer.join().expect("producer joined");
        assert!(result.is_ok(), "drain should terminate cleanly: {result:?}");

        let audio_count = collected
            .iter()
            .filter(|e| matches!(e, SynthesisEvent::Audio(_)))
            .count();
        let phrase_end_count = collected
            .iter()
            .filter(|e| matches!(e, SynthesisEvent::PhraseEnd))
            .count();
        assert_eq!(
            audio_count, AUDIO_EVENTS_PER_PHRASE,
            "every Audio event before PhraseEnd must reach the consumer"
        );
        assert_eq!(
            phrase_end_count, 1,
            "PhraseEnd must terminate the loop exactly once"
        );
    }

    /// Main-thread-path shape: a `try_recv` + no-op-wait-step closure.
    /// Verifies the helper produces the same observable behaviour
    /// regardless of whether the wait step blocks or polls.
    #[test]
    fn drives_try_recv_shape_until_phrase_end() {
        let (tx, rx) = channel::<SynthesisEvent>();

        // Pre-load the channel before drain starts; the no-op wait step
        // mirrors the main-thread path's "drain everything queued, then
        // pump the runloop for one slice" pattern (here the slice is a
        // 1 ms sleep to keep the test fast).
        for i in 0..AUDIO_EVENTS_PER_PHRASE {
            tx.send(make_audio_chunk(i as f32)).expect("send audio");
        }
        tx.send(SynthesisEvent::PhraseEnd).expect("send phrase end");

        let mut collected: Vec<SynthesisEvent> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(STRUCTURAL_TEST_TIMEOUT_SECS);
        let result = drive_streaming_drain(
            &mut |e| collected.push(e),
            deadline,
            "test-try-recv",
            || {
                // Main-thread shape: poll the channel; if empty, the
                // production code drives `NSRunLoop.runUntilDate`. Here
                // we substitute a 1 ms sleep so the test deadline
                // doesn't fire on a busy-loop.
                if let Ok(event) = rx.try_recv() {
                    return Ok(Some(event));
                }
                thread::sleep(Duration::from_millis(1));
                Ok(None)
            },
        );

        assert!(result.is_ok(), "drain should terminate cleanly: {result:?}");
        let audio_count = collected
            .iter()
            .filter(|e| matches!(e, SynthesisEvent::Audio(_)))
            .count();
        assert_eq!(audio_count, AUDIO_EVENTS_PER_PHRASE);
        assert!(matches!(collected.last(), Some(SynthesisEvent::PhraseEnd)));
    }

    /// Deadline already expired before the first poll → immediate
    /// `Speech` error. The wait-step closure must NOT be called.
    #[test]
    fn returns_timeout_error_when_deadline_has_already_passed() {
        let already_expired = Instant::now() - Duration::from_millis(1);
        let mut wait_calls = 0u32;
        let result = drive_streaming_drain(
            &mut |_| panic!("on_event must not be called"),
            already_expired,
            "test-expired",
            || {
                wait_calls += 1;
                Ok(None)
            },
        );

        assert!(result.is_err(), "expected timeout error");
        let err = result.unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("drain timeout"),
            "error must mention drain timeout: {msg}"
        );
        assert!(
            msg.contains("test-expired"),
            "error must include path_label: {msg}"
        );
        assert_eq!(
            wait_calls, 0,
            "wait step must not run when deadline is already in the past"
        );
    }

    /// A `next_event` closure that returns `Err(...)` must propagate
    /// the error through the helper without invoking `on_event`.
    #[test]
    fn propagates_inner_error_from_next_event_closure() {
        let deadline = Instant::now() + Duration::from_secs(STRUCTURAL_TEST_TIMEOUT_SECS);
        let result = drive_streaming_drain(
            &mut |_| panic!("on_event must not be called when next_event errors"),
            deadline,
            "test-disconnect",
            || {
                Err(PrimerError::Speech(
                    "channel disconnected before PhraseEnd".into(),
                ))
            },
        );

        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("disconnected"),
            "inner error must surface verbatim: {msg}"
        );
    }
}
