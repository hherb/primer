//! `LoopObserver` trait + supporting state types.

use uuid::Uuid;

/// State of the voice loop's main state machine.
///
/// Wire format (stable): the `name()` method returns snake_case strings
/// that the GUI's `primer://voice/state_change` event payload carries
/// across IPC. Frontend CSS selectors and JS state lookups depend on
/// these exact values — do not rename without bumping the IPC contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VoiceState {
    /// Mic open, waiting for child to start or continue speaking.
    Listen,
    /// Child stopped speaking; LLM is generating. Mic still open so a
    /// resumed utterance can abort the LLM.
    LatentThink,
    /// Primer is speaking aloud; mic closed.
    Speak,
    /// Loop is exiting (final state before observer.on_exit fires).
    Exit,
}

impl VoiceState {
    /// Stable snake_case wire string. Used as the IPC payload value AND
    /// as the `[data-state="..."]` attribute in the frontend.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Listen => "listen",
            Self::LatentThink => "latent_think",
            Self::Speak => "speak",
            Self::Exit => "exit",
        }
    }
}

/// Why the loop exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    /// External `stop_tx` signaled (CLI Ctrl+C, GUI End-voice-mode button).
    UserStop,
    /// Quit keyword matched in a finalized transcript ("goodbye" / "bye primer" / "stop primer").
    Keyword,
    /// Mic capture thread reported an unrecoverable error.
    MicError,
    /// Speaker output stream errored.
    SpeakerError,
}

impl ExitReason {
    /// Stable snake_case wire string. `UserStop` trims to `"user"` (not
    /// `"user_stop"`) because the GUI exit event payload reads more
    /// cleanly as `{reason: "user"}` than `{reason: "user_stop"}` —
    /// "user" reads as the agent, "user_stop" reads as the action.
    pub fn name(&self) -> &'static str {
        match self {
            Self::UserStop => "user",
            Self::Keyword => "keyword",
            Self::MicError => "mic_error",
            Self::SpeakerError => "speaker_error",
        }
    }
}

/// Payload delivered to [`LoopObserver::on_response_complete`] after a
/// full Primer turn finishes synthesising.
#[derive(Debug, Clone)]
pub struct TurnCompletePayload {
    pub session_id: Uuid,
    pub child_turn_index: usize,
    pub primer_turn_index: usize,
}

/// Side-effect surface for the voice loop. CLI provides a stdout-printing
/// impl; GUI provides a Tauri-event-emitting impl. Same state machine,
/// different I/O.
///
/// `Send + 'static` so the loop can move the observer into its
/// state-machine task at spawn time.
pub trait LoopObserver: Send + 'static {
    /// Called on every state transition. `hint` carries optional context:
    /// `Some("user_cancel")` when the loop returns to LISTEN because the
    /// user pressed Stop / Esc; `Some("child_resumed")` when VAD-cancel-
    /// on-resumed-speech fires. `None` for ordinary transitions.
    fn on_state_change(&mut self, state: VoiceState, hint: Option<&str>);

    /// Called when Whisper finalizes a transcript. The corresponding
    /// child turn lands in the DialogueManager session via the loop's
    /// own respond_to_streaming call shortly after.
    fn on_transcript_finalized(&mut self, text: &str);

    /// Called per LLM chunk during LATENT_THINK / SPEAK. Mirrors the
    /// text-mode `primer://chunk` semantics.
    fn on_response_chunk(&mut self, primer_turn_index: usize, chunk: &str);

    /// Called after a turn completes successfully and TTS has finished
    /// synthesising the last phrase.
    fn on_response_complete(&mut self, payload: TurnCompletePayload);

    /// Called when an LLM call fails mid-turn. The loop replays a fallback
    /// "sorry, I had trouble" line through TTS regardless; this hook lets
    /// the GUI surface a banner if it wants to.
    fn on_inference_error(&mut self, err: &primer_core::error::InferenceError);

    /// Called exactly once, just before the loop returns from `run_loop`.
    fn on_exit(&mut self, reason: ExitReason);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_state_name_is_stable_kebab_case() {
        // The frontend matches on these exact strings. A drift here
        // silently breaks the [data-state="..."] CSS selectors and
        // the JS state lookups. Pin every variant.
        assert_eq!(VoiceState::Listen.name(), "listen");
        assert_eq!(VoiceState::LatentThink.name(), "latent_think");
        assert_eq!(VoiceState::Speak.name(), "speak");
        assert_eq!(VoiceState::Exit.name(), "exit");
    }

    #[test]
    fn exit_reason_name_is_stable_snake_case() {
        // Same contract as VoiceState::name — the IPC payload reads
        // these strings.
        assert_eq!(ExitReason::UserStop.name(), "user");
        assert_eq!(ExitReason::Keyword.name(), "keyword");
        assert_eq!(ExitReason::MicError.name(), "mic_error");
        assert_eq!(ExitReason::SpeakerError.name(), "speaker_error");
    }

    #[test]
    fn loop_observer_is_object_safe() {
        // The loop calls the observer through `dyn LoopObserver` for
        // monomorphisation savings; this trick catches accidental
        // generics that would break object safety.
        fn _accepts(_o: Box<dyn LoopObserver>) {}
    }
}
