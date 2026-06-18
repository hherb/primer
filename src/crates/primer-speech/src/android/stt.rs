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

/// Poll the bridge for recognizer events, driving the derived VAD and
/// forwarding edges + transcripts, until `stop` fires or the bridge errors.
/// Re-arms `start_listening` after each utterance (the recognizer is
/// one-shot per `startListening`).
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
        let was_end = matches!(event, SpeechEvent::EndOfSpeech | SpeechEvent::Final { .. });
        process_event(&event, &mut vad, &event_tx, &transcript_tx);
        if was_end {
            // Re-arm for the next utterance (one-shot recognizer).
            vad.reset();
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
