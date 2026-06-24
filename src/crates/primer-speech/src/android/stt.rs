//! Android recognizer → voice-loop adapter. [`process_event`] is the pure
//! per-event step (host-tested); [`run_recognizer_loop`] is the async
//! driver that polls the bridge and calls it.
//!
//! The STT side of the loop reuses [`crate::voice_loop::channel_stt::ChannelStt`]
//! verbatim — the recognizer consumer feeds it committed transcripts plus a
//! `VadEvent` channel, exactly the macos-native-26 pattern. No distinct
//! `AndroidStt` type is needed; the builder constructs `ChannelStt` directly.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use primer_core::error::Result;
use primer_core::speech::VadEvent;

use crate::android::bridge::AndroidSpeechBridge;
use crate::android::events::SpeechEvent;
use crate::android::vad::AndroidDerivedVad;

/// What the consumer should do this iteration given whether the Primer is
/// speaking and whether the recognizer is currently armed. Pure so the
/// pause-during-SPEAK policy is host-testable.
#[derive(Debug, PartialEq, Eq)]
pub enum ArmAction {
    /// Not speaking and not armed → start listening.
    Arm,
    /// Speaking and armed → stop listening (don't transcribe the Primer's
    /// own TTS; no barge-in).
    Disarm,
    /// Already in the desired state → do nothing.
    Hold,
}

/// Decide the arm transition. The recognizer should listen exactly when the
/// Primer is NOT speaking.
pub fn arm_action(speaking: bool, armed: bool) -> ArmAction {
    match (speaking, armed) {
        (false, false) => ArmAction::Arm,
        (true, true) => ArmAction::Disarm,
        _ => ArmAction::Hold,
    }
}

/// One recognizer event → (VadEvent edges, final transcript). Pure: takes
/// the channels by ref so it is host-testable without tokio. The final
/// transcript is forwarded on `Final` only — partials are volatile and the
/// voice loop only needs the committed utterance (the macos26
/// ChannelStt-bridge policy).
///
/// **Ordering:** the committed transcript is sent BEFORE the `SpeechEnd`
/// edge, so the loop's `ChannelStt` has it buffered by the time it calls
/// `finalize()` after seeing `SpeechEnd` (the [`crate::voice_loop::channel_stt`]
/// ordering contract).
pub fn process_event(
    event: &SpeechEvent,
    vad: &mut AndroidDerivedVad,
    event_tx: &std::sync::mpsc::Sender<VadEvent>,
    transcript_tx: &std::sync::mpsc::Sender<String>,
) {
    if let SpeechEvent::Final { text } = event {
        let _ = transcript_tx.send(text.clone());
    }
    if let Some(edge) = vad.on_event(event) {
        let _ = event_tx.send(edge);
        if let Some(end) = vad.take_pending_end() {
            let _ = event_tx.send(end);
        }
    }
}

/// Whether a silently-dead recognizer should be force-recreated. Even with
/// recreate-per-arm, a fresh on-device recognizer can die with a terminal
/// error (e.g. `ERROR_SERVER_DISCONNECTED`) and then emit NO further events,
/// leaving the loop stuck in `armed=true` with a dead recognizer (device-found
/// 2026-06-24, issue #259). When armed and not speaking, if no recognizer
/// event has arrived within `timeout`, the loop drops the armed state so the
/// top-of-loop [`arm_action`] recreates the recognizer. Returns false while
/// speaking (the recognizer is intentionally disarmed during SPEAK) and when
/// not armed (nothing to watch). Pure so the watchdog decision is host-tested.
pub fn should_force_rearm(
    armed: bool,
    speaking: bool,
    since_last_event: std::time::Duration,
    timeout: std::time::Duration,
) -> bool {
    armed && !speaking && since_last_event >= timeout
}

/// Whether a recognizer event should trigger a re-arm (`start_listening`
/// again). The on-device `SpeechRecognizer` is one-shot per arm, so the
/// loop must re-arm after EVERY terminal outcome to keep listening:
///
/// - `Final` / `EndOfSpeech` — the utterance completed; re-arm for the next.
/// - `SttError` with a RECOVERABLE code (`ERROR_NO_MATCH` /
///   `ERROR_SPEECH_TIMEOUT` / `ERROR_RECOGNIZER_BUSY`) — the recognizer
///   heard nothing this window and stopped; re-arm so the child can still
///   speak. **Without this the loop dies on the first pre-speech timeout**
///   (the recognizer fires `ERROR_SPEECH_TIMEOUT` within seconds of arming
///   if no speech starts), which is exactly the "doesn't activate on my
///   voice" failure mode (device-found 2026-06-19).
/// - Any other `SttError` (permissions, language unavailable, client,
///   server) is terminal — re-arming would spin or never succeed.
/// - `Partial` — mid-utterance; never re-arm.
pub fn should_rearm(event: &SpeechEvent) -> bool {
    use primer_core::consts::speech::android::{
        ERROR_NO_MATCH, ERROR_RECOGNIZER_BUSY, ERROR_SPEECH_TIMEOUT,
    };
    match event {
        SpeechEvent::Final { .. } | SpeechEvent::EndOfSpeech => true,
        SpeechEvent::SttError { code } => {
            matches!(
                *code,
                ERROR_NO_MATCH | ERROR_SPEECH_TIMEOUT | ERROR_RECOGNIZER_BUSY
            )
        }
        _ => false,
    }
}

/// Poll the bridge for recognizer events, driving the derived VAD and
/// forwarding edges + transcripts, until `stop` fires or the bridge errors.
/// Re-arms `start_listening` after each terminal event ([`should_rearm`] —
/// the recognizer is one-shot per `startListening`), including after a
/// recoverable no-match/timeout so the loop keeps listening.
pub async fn run_recognizer_loop(
    bridge: Arc<dyn AndroidSpeechBridge>,
    bcp47: String,
    event_tx: std::sync::mpsc::Sender<VadEvent>,
    transcript_tx: std::sync::mpsc::Sender<String>,
    mut stop: tokio::sync::oneshot::Receiver<()>,
    speaking: Arc<AtomicBool>,
) -> Result<()> {
    use primer_core::consts::speech::android::{POLL_TIMEOUT, RECOGNIZER_WATCHDOG_TIMEOUT};
    use std::time::Instant;
    let timeout_ms = POLL_TIMEOUT.as_millis() as u32;
    let mut vad = AndroidDerivedVad::new();
    let mut armed = false;
    // Last time the recognizer produced any event. Drives the silent-dead
    // recognizer watchdog (issue #259): a fresh recognizer can die with a
    // terminal error and then emit nothing, wedging the armed loop.
    let mut last_event_at = Instant::now();
    loop {
        if stop.try_recv().is_ok() {
            let _ = bridge.stop_listening();
            return Ok(());
        }
        // Pause listening while the Primer speaks (no barge-in / no TTS
        // self-capture); re-arm fresh the moment it stops (less clipping).
        match arm_action(speaking.load(Ordering::SeqCst), armed) {
            ArmAction::Arm => {
                vad.reset();
                bridge.start_listening(&bcp47)?;
                armed = true;
                // Give the freshly armed recognizer a full watchdog window.
                last_event_at = Instant::now();
            }
            ArmAction::Disarm => {
                let _ = bridge.stop_listening();
                vad.reset();
                armed = false;
            }
            ArmAction::Hold => {}
        }
        if !armed {
            // Not listening (Primer speaking) — idle until it stops.
            tokio::time::sleep(POLL_TIMEOUT).await;
            continue;
        }
        // poll_event blocks up to timeout_ms inside Kotlin; wrap in
        // spawn_blocking so the tokio worker is not held for the wait.
        let bridge_poll = Arc::clone(&bridge);
        let polled = tokio::task::spawn_blocking(move || bridge_poll.poll_event(timeout_ms))
            .await
            .map_err(|e| primer_core::error::PrimerError::Speech(format!("poll join: {e}")))??;
        let Some(event) = polled else {
            // No event this poll. If the recognizer has been silent past the
            // watchdog window while armed (a fresh instance that died with a
            // terminal error and emitted nothing — issue #259), drop the armed
            // state so the next `arm_action` recreates it (recreate-per-arm
            // handles cleanup of the dead instance).
            if should_force_rearm(
                armed,
                speaking.load(Ordering::SeqCst),
                last_event_at.elapsed(),
                RECOGNIZER_WATCHDOG_TIMEOUT,
            ) {
                vad.reset();
                armed = false;
                last_event_at = Instant::now();
            }
            continue;
        };
        last_event_at = Instant::now();
        let rearm = should_rearm(&event);
        // Only ERROR_RECOGNIZER_BUSY needs a backoff before re-arming (it can
        // fire immediately and tight-spin). NO_MATCH / SPEECH_TIMEOUT fire only
        // after a multi-second audio window, so they re-arm immediately —
        // minimising the dead window in which the child's first word would be
        // clipped on a follow-up turn (device-found 2026-06-19: leading 1-2
        // words cut after the first prompt; this shrinks the re-arm gap).
        let needs_backoff = matches!(
            event,
            SpeechEvent::SttError {
                code: primer_core::consts::speech::android::ERROR_RECOGNIZER_BUSY
            }
        );
        process_event(&event, &mut vad, &event_tx, &transcript_tx);
        if rearm {
            // Two arm sites coexist by design: the top-of-loop `arm_action`
            // (which re-arms a poll-timeout later) and this inline re-arm. The
            // inline path exists ONLY to shrink the clip window — it re-arms
            // immediately (after the backoff) rather than waiting for the next
            // loop iteration. Setting `armed = true` here makes the next
            // `arm_action` a no-op (`Hold`), so the recognizer is never armed
            // twice. If the Primer is speaking we skip the inline arm and let
            // the top-of-loop `arm_action` re-arm once SPEAK ends.
            vad.reset();
            armed = false;
            if needs_backoff {
                tokio::time::sleep(primer_core::consts::speech::android::REARM_BACKOFF).await;
            }
            if !speaking.load(Ordering::SeqCst) {
                bridge.start_listening(&bcp47)?;
                armed = true;
                // Fresh recognizer — restart the watchdog window.
                last_event_at = Instant::now();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_event_emits_start_then_transcript_then_end() {
        let mut vad = AndroidDerivedVad::new();
        let (event_tx, event_rx) = std::sync::mpsc::channel::<VadEvent>();
        let (txt_tx, txt_rx) = std::sync::mpsc::channel::<String>();

        process_event(
            &SpeechEvent::Partial { text: "how".into() },
            &mut vad,
            &event_tx,
            &txt_tx,
        );
        process_event(
            &SpeechEvent::Final {
                text: "how do birds fly".into(),
            },
            &mut vad,
            &event_tx,
            &txt_tx,
        );
        process_event(&SpeechEvent::EndOfSpeech, &mut vad, &event_tx, &txt_tx);

        // VAD: SpeechStart (from partial), SpeechEnd (from Final).
        assert_eq!(event_rx.recv().unwrap(), VadEvent::SpeechStart);
        assert_eq!(event_rx.recv().unwrap(), VadEvent::SpeechEnd);
        assert!(event_rx.try_recv().is_err(), "no extra edges");
        // Transcript: the Final text, forwarded once.
        assert_eq!(txt_rx.recv().unwrap(), "how do birds fly");
        assert!(txt_rx.try_recv().is_err(), "no extra transcripts");
    }

    #[test]
    fn stt_error_does_not_emit_transcript_or_edge() {
        let mut vad = AndroidDerivedVad::new();
        let (event_tx, event_rx) = std::sync::mpsc::channel::<VadEvent>();
        let (txt_tx, txt_rx) = std::sync::mpsc::channel::<String>();
        process_event(
            &SpeechEvent::SttError { code: 7 },
            &mut vad,
            &event_tx,
            &txt_tx,
        );
        assert!(txt_rx.try_recv().is_err());
        assert!(event_rx.try_recv().is_err());
    }

    #[test]
    fn rearm_on_terminal_events_and_recoverable_errors() {
        use primer_core::consts::speech::android::{
            ERROR_NO_MATCH, ERROR_RECOGNIZER_BUSY, ERROR_SPEECH_TIMEOUT,
        };
        // Terminal utterance events re-arm.
        assert!(should_rearm(&SpeechEvent::Final { text: "hi".into() }));
        assert!(should_rearm(&SpeechEvent::EndOfSpeech));
        // Recoverable errors re-arm so the loop keeps listening (the
        // device-found "doesn't activate" fix — a pre-speech timeout must
        // not kill the loop).
        assert!(should_rearm(&SpeechEvent::SttError {
            code: ERROR_SPEECH_TIMEOUT
        }));
        assert!(should_rearm(&SpeechEvent::SttError {
            code: ERROR_NO_MATCH
        }));
        assert!(should_rearm(&SpeechEvent::SttError {
            code: ERROR_RECOGNIZER_BUSY
        }));
    }

    #[test]
    fn force_rearm_only_when_armed_idle_and_past_timeout() {
        use std::time::Duration;
        let timeout = Duration::from_secs(12);
        // Armed, not speaking, silent past the timeout → recreate (the #259
        // silent-dead-recognizer recovery).
        assert!(should_force_rearm(
            true,
            false,
            Duration::from_secs(13),
            timeout
        ));
        assert!(should_force_rearm(true, false, timeout, timeout)); // boundary: >=
        // Still within the window (healthy idle recognizer fires NO_MATCH
        // every ~5s, resetting the clock) → no recreate.
        assert!(!should_force_rearm(
            true,
            false,
            Duration::from_secs(5),
            timeout
        ));
        // Not armed → nothing to watch.
        assert!(!should_force_rearm(
            false,
            false,
            Duration::from_secs(99),
            timeout
        ));
        // Speaking → recognizer is intentionally disarmed during SPEAK.
        assert!(!should_force_rearm(
            true,
            true,
            Duration::from_secs(99),
            timeout
        ));
    }

    #[test]
    fn arm_action_listens_only_when_not_speaking() {
        // Primer silent + not yet listening → arm.
        assert_eq!(arm_action(false, false), ArmAction::Arm);
        // Primer speaking + still armed → disarm (no barge-in / TTS capture).
        assert_eq!(arm_action(true, true), ArmAction::Disarm);
        // Already in the desired state → hold.
        assert_eq!(arm_action(false, true), ArmAction::Hold);
        assert_eq!(arm_action(true, false), ArmAction::Hold);
    }

    #[test]
    fn no_rearm_on_partial_or_fatal_errors() {
        use primer_core::consts::speech::android::ERROR_INSUFFICIENT_PERMISSIONS;
        // Mid-utterance partials never re-arm.
        assert!(!should_rearm(&SpeechEvent::Partial { text: "ho".into() }));
        // Fatal errors (permissions=9, language unavailable=12, client=5)
        // are terminal — re-arming would spin or never succeed.
        assert!(!should_rearm(&SpeechEvent::SttError {
            code: ERROR_INSUFFICIENT_PERMISSIONS
        }));
        assert!(!should_rearm(&SpeechEvent::SttError { code: 12 }));
        assert!(!should_rearm(&SpeechEvent::SttError { code: 5 }));
    }

    #[test]
    fn final_from_idle_emits_start_end_and_transcript() {
        // A Final with no prior partial: transcript forwarded, then
        // SpeechStart + the pending SpeechEnd both reach the event channel.
        let mut vad = AndroidDerivedVad::new();
        let (event_tx, event_rx) = std::sync::mpsc::channel::<VadEvent>();
        let (txt_tx, txt_rx) = std::sync::mpsc::channel::<String>();
        process_event(
            &SpeechEvent::Final { text: "yes".into() },
            &mut vad,
            &event_tx,
            &txt_tx,
        );
        assert_eq!(txt_rx.recv().unwrap(), "yes");
        assert_eq!(event_rx.recv().unwrap(), VadEvent::SpeechStart);
        assert_eq!(event_rx.recv().unwrap(), VadEvent::SpeechEnd);
        assert!(event_rx.try_recv().is_err());
    }
}
