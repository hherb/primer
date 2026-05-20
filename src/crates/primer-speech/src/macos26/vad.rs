//! Pure-logic state machine that translates SpeechTranscriber `Result`
//! events into the `VadEvent`s that `voice_loop::run_loop` consumes.
//! No FFI, no I/O, no async — easy to unit-test.

#![cfg(all(target_vendor = "apple", feature = "macos-native-26"))]

use std::time::Instant;

use primer_core::consts::speech::macos26::{
    EVENT_POLL_INTERVAL, SPEECH_END_TIMEOUT, SPEECH_START_MIN_TEXT_CHARS,
};
use primer_core::speech::VadEvent;

/// Internal state of the derived VAD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Speaking,
}

/// Pure-logic state machine driven by the audio task's `on_result` calls
/// and a periodic `tick`. Returns `Some(VadEvent)` when a transition fires;
/// the caller is responsible for pushing it onto the top-level event mpsc.
pub struct DerivedVadStateMachine {
    state: State,
    last_partial_at: Option<Instant>,
}

impl DerivedVadStateMachine {
    pub fn new() -> Self {
        Self { state: State::Idle, last_partial_at: None }
    }

    /// Reset to `Idle` between utterances.
    pub fn reset(&mut self) {
        self.state = State::Idle;
        self.last_partial_at = None;
    }

    /// Handle a transcriber Result. Returns a VadEvent if the result
    /// triggers a state transition, otherwise None.
    pub fn on_result(
        &mut self,
        text: &str,
        is_final: bool,
        now: Instant,
    ) -> Option<VadEvent> {
        let non_empty = text.trim().chars().count() >= SPEECH_START_MIN_TEXT_CHARS;
        match self.state {
            State::Idle => {
                if non_empty {
                    self.state = State::Speaking;
                    self.last_partial_at = Some(now);
                    Some(VadEvent::SpeechStart)
                } else {
                    None
                }
            }
            State::Speaking => {
                if is_final {
                    self.state = State::Idle;
                    self.last_partial_at = None;
                    Some(VadEvent::SpeechEnd)
                } else {
                    if non_empty {
                        self.last_partial_at = Some(now);
                    }
                    None
                }
            }
        }
    }

    /// Periodic tick from the audio task. Returns a VadEvent if the
    /// inactivity timer fires, otherwise None.
    pub fn tick(&mut self, now: Instant) -> Option<VadEvent> {
        if self.state != State::Speaking {
            return None;
        }
        let Some(last) = self.last_partial_at else {
            return None;
        };
        if now.duration_since(last) > SPEECH_END_TIMEOUT {
            self.state = State::Idle;
            self.last_partial_at = None;
            Some(VadEvent::SpeechEnd)
        } else {
            None
        }
    }
}

impl Default for DerivedVadStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(ms: u64) -> Instant {
        thread_local! {
            static BASE: Instant = Instant::now();
        }
        BASE.with(|b| *b + Duration::from_millis(ms))
    }

    #[test]
    fn first_non_empty_partial_emits_speech_start() {
        let mut sm = DerivedVadStateMachine::new();
        assert_eq!(sm.on_result("hello", false, at(100)), Some(VadEvent::SpeechStart));
    }

    #[test]
    fn empty_partial_does_not_emit_speech_start() {
        let mut sm = DerivedVadStateMachine::new();
        assert_eq!(sm.on_result("", false, at(100)), None);
        assert_eq!(sm.on_result("   ", false, at(150)), None);
    }

    #[test]
    fn is_final_emits_speech_end() {
        let mut sm = DerivedVadStateMachine::new();
        assert_eq!(sm.on_result("hello", false, at(100)), Some(VadEvent::SpeechStart));
        assert_eq!(sm.on_result("hello world.", true, at(2000)), Some(VadEvent::SpeechEnd));
    }

    #[test]
    fn inactivity_timer_emits_speech_end() {
        let mut sm = DerivedVadStateMachine::new();
        assert_eq!(sm.on_result("hello", false, at(100)), Some(VadEvent::SpeechStart));
        // Tick before the timeout — no event.
        assert_eq!(sm.tick(at(100 + 300)), None);
        // Tick after the timeout — SpeechEnd.
        let timeout_ms = SPEECH_END_TIMEOUT.as_millis() as u64;
        assert_eq!(sm.tick(at(100 + timeout_ms + 50)), Some(VadEvent::SpeechEnd));
    }

    #[test]
    fn mid_speaking_partials_extend_timer() {
        let mut sm = DerivedVadStateMachine::new();
        let timeout_ms = SPEECH_END_TIMEOUT.as_millis() as u64;
        assert_eq!(sm.on_result("hello", false, at(0)), Some(VadEvent::SpeechStart));
        // Partial just before the timer would fire — extends last_partial_at.
        assert_eq!(sm.on_result("hello world", false, at(timeout_ms - 100)), None);
        // Original deadline has now passed but the new partial reset the timer.
        assert_eq!(sm.tick(at(timeout_ms + 50)), None);
        // True timeout from the new partial.
        assert_eq!(sm.tick(at(timeout_ms - 100 + timeout_ms + 50)), Some(VadEvent::SpeechEnd));
    }

    #[test]
    fn reset_returns_to_idle() {
        let mut sm = DerivedVadStateMachine::new();
        sm.on_result("hello", false, at(0));
        sm.reset();
        // After reset, next non-empty partial fires SpeechStart cleanly.
        assert_eq!(sm.on_result("hi", false, at(2000)), Some(VadEvent::SpeechStart));
    }

    #[test]
    fn min_text_chars_threshold_respected() {
        // The const lives in primer-core::consts::speech::macos26.
        // Pin its value so a future bump explicitly re-runs these tests.
        assert_eq!(SPEECH_START_MIN_TEXT_CHARS, 1);
        // Sanity: POLL interval is under timeout.
        assert!(EVENT_POLL_INTERVAL < SPEECH_END_TIMEOUT);
    }
}
