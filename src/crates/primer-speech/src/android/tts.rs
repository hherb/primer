//! `StreamingTextToSpeech` over Android `TextToSpeech`. The OS plays the
//! audio itself, so `push_text` calls `bridge.speak` (which blocks until
//! the engine's `onDone`) and emits NO `SynthesisEvent::Audio` — the
//! voice loop's `on_committed_audio` stays a no-op on android (D3).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use primer_core::error::Result;
use primer_core::speech::{
    Named, StreamingTextToSpeech, SynthesisEvent, SynthesisSession, VoiceProfile,
};

use crate::android::bridge::AndroidSpeechBridge;

/// Nominal sample rate reported to the loop. Android plays audio itself,
/// so this never drives a resampler on android; it exists only to satisfy
/// `StreamingTextToSpeech::sample_rate`. 22_050 mirrors the Piper-class
/// default used elsewhere.
const ANDROID_TTS_NOMINAL_RATE: u32 = 22_050;

/// Android `TextToSpeech` synthesiser. Holds the bridge; each session is a
/// thin handle that routes `push_text` to `bridge.speak`.
///
/// `speaking` is a shared flag raised from a session's first `push_text`
/// until the session ends (= while the OS is playing the reply, including
/// the gaps between its phrases). The recognizer consumer
/// ([`crate::android::stt::run_recognizer_loop`]) watches it and stops
/// listening while it is set, so the recognizer never transcribes the
/// Primer's own TTS (no barge-in) and re-arms fresh the instant the Primer
/// stops speaking (less leading-word clipping; device-found 2026-06-19).
pub struct AndroidTts {
    bridge: Arc<dyn AndroidSpeechBridge>,
    speaking: Arc<AtomicBool>,
}

impl AndroidTts {
    pub fn new(bridge: Arc<dyn AndroidSpeechBridge>) -> Self {
        Self {
            bridge,
            speaking: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The shared "Primer is speaking" flag, for the recognizer consumer to
    /// pause listening during SPEAK. Both hold the same `Arc`.
    pub fn speaking_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.speaking)
    }
}

impl Named for AndroidTts {
    fn name(&self) -> &str {
        "android-tts"
    }
}

impl StreamingTextToSpeech for AndroidTts {
    fn sample_rate(&self) -> u32 {
        ANDROID_TTS_NOMINAL_RATE
    }
    fn open_session(&self, _voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>> {
        Ok(Box::new(AndroidTtsSession {
            bridge: Arc::clone(&self.bridge),
            speaking: Arc::clone(&self.speaking),
        }))
    }
}

struct AndroidTtsSession {
    bridge: Arc<dyn AndroidSpeechBridge>,
    speaking: Arc<AtomicBool>,
}

impl SynthesisSession for AndroidTtsSession {
    fn push_text(&mut self, text: &str, _on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()> {
        if text.trim().is_empty() {
            return Ok(());
        }
        // Raise the shared flag so the recognizer consumer stops listening.
        // It stays up between phrases and is cleared when the session drops
        // (#325: one push per phrase).
        self.speaking.store(true, Ordering::SeqCst);
        // Blocks until the Android engine reports onDone (D3). No PCM is
        // surfaced — the OS already played it.
        self.bridge.speak(text.trim())
    }

    fn finalize(self: Box<Self>, _on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()> {
        Ok(())
    }
}

/// Clears the `speaking` flag when the session ends: after `finalize`, or
/// when an error (or early return) drops it, so the recognizer can never
/// stay paused forever.
impl Drop for AndroidTtsSession {
    fn drop(&mut self) {
        self.speaking.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::android::bridge::tests::MockBridge;

    #[test]
    fn push_text_speaks_via_bridge_and_emits_no_audio() {
        let bridge = Arc::new(MockBridge::with_events(vec![]));
        let tts = AndroidTts::new(Arc::clone(&bridge) as Arc<dyn AndroidSpeechBridge>);
        let mut session = tts.open_session(&VoiceProfile::default()).unwrap();
        let mut audio_events = 0u32;
        session
            .push_text("Why do you think birds have feathers?", &mut |e| {
                if let SynthesisEvent::Audio(_) = e {
                    audio_events += 1;
                }
            })
            .unwrap();
        assert_eq!(audio_events, 0, "android TTS plays itself; no PCM events");
        assert_eq!(
            bridge.spoken.lock().unwrap().as_slice(),
            ["Why do you think birds have feathers?"]
        );
    }

    #[test]
    fn empty_text_is_a_noop() {
        let bridge = Arc::new(MockBridge::with_events(vec![]));
        let tts = AndroidTts::new(Arc::clone(&bridge) as Arc<dyn AndroidSpeechBridge>);
        let mut session = tts.open_session(&VoiceProfile::default()).unwrap();
        session.push_text("   ", &mut |_| {}).unwrap();
        assert!(bridge.spoken.lock().unwrap().is_empty());
    }

    /// #325 stage 2: SPEAK pushes one phrase at a time, so the flag must
    /// stay up between pushes; a per-push clear would let the recognizer
    /// re-arm (and hear the TTS) in the gap.
    #[test]
    fn speaking_stays_raised_between_phrases_until_finalize() {
        let bridge = Arc::new(MockBridge::with_events(vec![]));
        let tts = AndroidTts::new(Arc::clone(&bridge) as Arc<dyn AndroidSpeechBridge>);
        let flag = tts.speaking_flag();
        let mut session = tts.open_session(&VoiceProfile::default()).unwrap();
        session.push_text("One. ", &mut |_| {}).unwrap();
        assert!(flag.load(Ordering::SeqCst), "raised after the first phrase");
        session.push_text("Two. ", &mut |_| {}).unwrap();
        assert!(flag.load(Ordering::SeqCst));
        session.finalize(&mut |_| {}).unwrap();
        assert!(
            !flag.load(Ordering::SeqCst),
            "cleared when the session ends"
        );
    }

    /// A session dropped without `finalize` (an error in SPEAK) must not
    /// leave the recognizer paused forever.
    #[test]
    fn dropping_a_session_clears_speaking() {
        let bridge = Arc::new(MockBridge::with_events(vec![]));
        let tts = AndroidTts::new(Arc::clone(&bridge) as Arc<dyn AndroidSpeechBridge>);
        let flag = tts.speaking_flag();
        let mut session = tts.open_session(&VoiceProfile::default()).unwrap();
        session.push_text("One. ", &mut |_| {}).unwrap();
        drop(session);
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn finalize_emits_no_audio() {
        let bridge = Arc::new(MockBridge::with_events(vec![]));
        let tts = AndroidTts::new(Arc::clone(&bridge) as Arc<dyn AndroidSpeechBridge>);
        let session = tts.open_session(&VoiceProfile::default()).unwrap();
        let mut audio_events = 0u32;
        session
            .finalize(&mut |e| {
                if let SynthesisEvent::Audio(_) = e {
                    audio_events += 1;
                }
            })
            .unwrap();
        assert_eq!(audio_events, 0);
    }
}
