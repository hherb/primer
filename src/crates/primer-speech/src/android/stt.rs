//! Android recognizer → voice-loop adapter. [`process_event`] is the pure
//! per-event step (host-tested); [`run_recognizer_loop`] is the async
//! driver that polls the bridge and calls it.
//!
//! The STT side of the loop reuses [`crate::voice_loop::channel_stt::ChannelStt`]
//! verbatim — the recognizer consumer feeds it committed transcripts plus a
//! `VadEvent` channel, exactly the macos-native-26 pattern. No distinct
//! `AndroidStt` type is needed; the builder constructs `ChannelStt` directly.

use std::sync::Arc;

use primer_core::error::Result;
use primer_core::speech::VadEvent;

use crate::android::bridge::AndroidSpeechBridge;
use crate::android::events::SpeechEvent;
use crate::android::vad::AndroidDerivedVad;

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
) -> Result<()> {
    use primer_core::consts::speech::android::POLL_TIMEOUT;
    let timeout_ms = POLL_TIMEOUT.as_millis() as u32;
    let mut vad = AndroidDerivedVad::new();
    bridge.start_listening(&bcp47)?;
    loop {
        if stop.try_recv().is_ok() {
            let _ = bridge.stop_listening();
            return Ok(());
        }
        // poll_event blocks up to timeout_ms inside Kotlin; wrap in
        // spawn_blocking so the tokio worker is not held for the wait.
        let bridge_poll = Arc::clone(&bridge);
        let polled = tokio::task::spawn_blocking(move || bridge_poll.poll_event(timeout_ms))
            .await
            .map_err(|e| primer_core::error::PrimerError::Speech(format!("poll join: {e}")))??;
        let Some(event) = polled else { continue };
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
            vad.reset();
            if needs_backoff {
                tokio::time::sleep(primer_core::consts::speech::android::REARM_BACKOFF).await;
            }
            bridge.start_listening(&bcp47)?;
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
    fn no_rearm_on_partial_or_fatal_errors() {
        // Mid-utterance partials never re-arm.
        assert!(!should_rearm(&SpeechEvent::Partial { text: "ho".into() }));
        // Fatal errors (permissions=9, language unavailable=12, client=5)
        // are terminal — re-arming would spin or never succeed.
        assert!(!should_rearm(&SpeechEvent::SttError { code: 9 }));
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
