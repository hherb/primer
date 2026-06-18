//! Android-native voice backend builder. Unlike the cpal builders, the OS
//! owns the mic (SpeechRecognizer) and the speaker (TextToSpeech), so
//! there is no audio thread, no mic/speaker ringbuf, and no `on_audio` /
//! drain machinery — the GUI passes a no-op `on_committed_audio`,
//! `wait_for_speaker_drain = None`, and `is_speaking = None` to `run_loop`
//! (D1).

#![cfg(feature = "android-native")]

use std::sync::Arc;

use primer_core::error::Result;
use primer_core::i18n::Locale;
use primer_core::speech::{VadEvent, VoiceProfile};

use crate::android::bridge::AndroidSpeechBridge;
use crate::android::stt::run_recognizer_loop;
use crate::android::tts::AndroidTts;
use crate::voice_loop::channel_stt::ChannelStt;
use crate::voice_loop::{LoopBackends, VAD_EVENT_CHANNEL_CAPACITY};

/// Cpal-free Android backend bundle. The GUI extracts `backends` +
/// `event_rx` for `run_loop`; `stop` ends the recognizer consumer task
/// when voice mode is turned off.
pub struct AndroidVoiceBackends {
    pub backends: LoopBackends,
    pub event_rx: tokio::sync::mpsc::Receiver<VadEvent>,
    pub stop: tokio::sync::oneshot::Sender<()>,
}

/// Map the active locale to the recognizer + TTS BCP-47 tag. The android
/// POC is en-focused (spec scope); `de` maps to `de-DE` for when on-device
/// German STT exists (else the recognizer errors and the GUI falls back —
/// deferred). Uses `pack_id` → BCP-47.
fn bcp47_for(locale: Locale) -> String {
    match locale.pack_id() {
        "de" => "de-DE".to_string(),
        _ => "en-US".to_string(),
    }
}

/// Build the Android voice backends: a `ChannelStt` fed by the recognizer
/// consumer task, an `AndroidTts`, and the `VadEvent` channel.
///
/// Must run inside a tokio runtime (the GUI command is async) — it spawns
/// the recognizer consumer task and the std→tokio event forwarder.
pub fn build_android_voice_backends(
    bridge: Arc<dyn AndroidSpeechBridge>,
    locale: Locale,
    voice: VoiceProfile,
) -> Result<AndroidVoiceBackends> {
    let bcp47 = bcp47_for(locale);

    // tokio mpsc: consumer → voice loop (VadEvent). `run_loop` awaits this.
    let (event_tx_tok, event_rx) =
        tokio::sync::mpsc::channel::<VadEvent>(VAD_EVENT_CHANNEL_CAPACITY);
    // std mpsc: consumer → ChannelStt (final transcripts as String).
    let (transcript_tx, transcript_rx) = std::sync::mpsc::channel::<String>();
    // std mpsc: the recognizer consumer emits VadEvent on a std Sender (so
    // its pure `process_event` core stays tokio-free for host tests); a
    // forwarder task converts them to the tokio channel the loop reads.
    let (event_tx_std, event_rx_std) = std::sync::mpsc::channel::<VadEvent>();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();

    // Forward std VadEvents → tokio channel. `recv()` blocks, so run it on a
    // blocking thread rather than holding a tokio worker.
    tokio::task::spawn_blocking(move || {
        while let Ok(evt) = event_rx_std.recv() {
            if event_tx_tok.blocking_send(evt).is_err() {
                break;
            }
        }
    });

    // Recognizer consumer task.
    let bridge_consumer = Arc::clone(&bridge);
    tokio::spawn(async move {
        if let Err(e) =
            run_recognizer_loop(bridge_consumer, bcp47, event_tx_std, transcript_tx, stop_rx).await
        {
            tracing::warn!(target: "primer::speech::android", "recognizer loop ended: {e}");
        }
    });

    let stt = Arc::new(ChannelStt::from_receiver(transcript_rx));
    let tts = Arc::new(AndroidTts::new(bridge));
    let backends = LoopBackends::single_locale(stt, tts, voice, locale);
    backends.ensure_active_locale_coverage()?;

    Ok(AndroidVoiceBackends {
        backends,
        event_rx,
        stop: stop_tx,
    })
}
