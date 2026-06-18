//! Derived VAD for the Android on-device `SpeechRecognizer`. The
//! recognizer owns the mic and endpoints itself; we synthesise the
//! `VadEvent::SpeechStart`/`SpeechEnd` pair the voice-loop state machine
//! expects from the recognizer's discrete callbacks.
//!
//! DRY follow-up (noted, not done now): this mirrors `macos26/vad.rs`. The
//! two are kept separate because `SpeechRecognizer` (discrete
//! `onEndOfSpeech`) and `SpeechAnalyzer` (continuous partials + inactivity
//! timer) have different end-of-utterance signals. Once both are
//! device-tuned, hoist a shared pure state machine if the logic converges.
//! YAGNI until then.

use crate::android::events::SpeechEvent;
use primer_core::consts::speech::android::SPEECH_START_MIN_TEXT_CHARS;
use primer_core::speech::VadEvent;

/// Two-state derived VAD. `Idle` until a non-empty transcript arrives
/// (→ `SpeechStart`); `Speaking` until `EndOfSpeech`/`Final` (→
/// `SpeechEnd`). A `Final` arriving from `Idle` both starts and ends: it
/// returns `SpeechStart` and stashes a pending `SpeechEnd` the consumer
/// drains with [`Self::take_pending_end`] (keeps one edge per `on_event`).
#[derive(Debug, Default)]
pub struct AndroidDerivedVad {
    in_speech: bool,
    pending_end: bool,
}

impl AndroidDerivedVad {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset between utterances (the recognizer is one-shot; the consumer
    /// re-arms and resets after each end edge).
    pub fn reset(&mut self) {
        self.in_speech = false;
        self.pending_end = false;
    }

    fn is_onset(text: &str) -> bool {
        text.trim().chars().count() >= SPEECH_START_MIN_TEXT_CHARS
    }

    /// Feed a recognizer event; returns at most one `VadEvent` edge.
    pub fn on_event(&mut self, event: &SpeechEvent) -> Option<VadEvent> {
        match event {
            SpeechEvent::Partial { text } if !self.in_speech && Self::is_onset(text) => {
                self.in_speech = true;
                Some(VadEvent::SpeechStart)
            }
            SpeechEvent::Final { text } if !self.in_speech && Self::is_onset(text) => {
                // A Final from Idle both starts and ends the utterance. Emit
                // the start edge now and stash the end so each call yields a
                // single edge; the consumer drains it via `take_pending_end`.
                self.in_speech = true;
                self.pending_end = true;
                Some(VadEvent::SpeechStart)
            }
            SpeechEvent::EndOfSpeech | SpeechEvent::Final { .. } if self.in_speech => {
                self.in_speech = false;
                Some(VadEvent::SpeechEnd)
            }
            _ => None,
        }
    }

    /// Drain a stashed end edge (set when a `Final` started speech from
    /// `Idle`). Returns `Some(SpeechEnd)` exactly once after such a Final.
    pub fn take_pending_end(&mut self) -> Option<VadEvent> {
        if self.pending_end {
            self.pending_end = false;
            self.in_speech = false;
            Some(VadEvent::SpeechEnd)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_nonempty_partial_starts_speech() {
        let mut vad = AndroidDerivedVad::new();
        assert_eq!(vad.on_event(&SpeechEvent::Partial { text: "".into() }), None);
        assert_eq!(
            vad.on_event(&SpeechEvent::Partial { text: "how".into() }),
            Some(VadEvent::SpeechStart)
        );
        // Subsequent partials do not re-fire SpeechStart.
        assert_eq!(
            vad.on_event(&SpeechEvent::Partial {
                text: "how do".into()
            }),
            None
        );
    }

    #[test]
    fn end_of_speech_ends_when_in_speech() {
        let mut vad = AndroidDerivedVad::new();
        vad.on_event(&SpeechEvent::Partial { text: "hi".into() });
        assert_eq!(
            vad.on_event(&SpeechEvent::EndOfSpeech),
            Some(VadEvent::SpeechEnd)
        );
    }

    #[test]
    fn final_without_prior_end_still_ends() {
        let mut vad = AndroidDerivedVad::new();
        vad.on_event(&SpeechEvent::Partial { text: "hi".into() });
        assert_eq!(
            vad.on_event(&SpeechEvent::Final {
                text: "hi there".into()
            }),
            Some(VadEvent::SpeechEnd)
        );
    }

    #[test]
    fn end_before_start_is_ignored() {
        let mut vad = AndroidDerivedVad::new();
        assert_eq!(vad.on_event(&SpeechEvent::EndOfSpeech), None);
    }

    #[test]
    fn final_from_idle_starts_then_ends_via_pending() {
        // A Final arriving with no prior partial both starts and ends:
        // SpeechStart on the call, SpeechEnd via take_pending_end.
        let mut vad = AndroidDerivedVad::new();
        assert_eq!(
            vad.on_event(&SpeechEvent::Final { text: "yes".into() }),
            Some(VadEvent::SpeechStart)
        );
        assert_eq!(vad.take_pending_end(), Some(VadEvent::SpeechEnd));
        assert_eq!(vad.take_pending_end(), None);
    }

    #[test]
    fn reset_clears_in_speech_and_pending() {
        let mut vad = AndroidDerivedVad::new();
        vad.on_event(&SpeechEvent::Final { text: "yes".into() });
        vad.reset();
        // After reset a fresh partial starts speech again.
        assert_eq!(
            vad.on_event(&SpeechEvent::Partial { text: "next".into() }),
            Some(VadEvent::SpeechStart)
        );
        assert_eq!(vad.take_pending_end(), None);
    }
}
