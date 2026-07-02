//! Public types and traits consumed by the voice-loop state machine:
//! configuration, injected backends, the responder seam, and the
//! external-control handle. No control flow lives here.

use std::path::PathBuf;
use std::sync::Arc;

use primer_core::error::Result;
use primer_core::speech::{StreamingSpeechToText, StreamingTextToSpeech};

/// Configuration passed into [`super::run_loop`] / the higher-level `run`
/// entry point in `primer-cli`.
///
/// Owns its paths and the voice id so the entire config is `'static` and
/// can be moved into a spawned task. Previously borrowed (`&'a Path` /
/// `&'a str`) — the spawn-based [`super::run_loop`] requires `'static`.
pub struct LoopConfig {
    pub whisper_model: PathBuf,
    pub voice_onnx: PathBuf,
    pub voice_config: PathBuf,
    pub voice_id: String,
    pub mic_silence_ms: u32,
    pub verbose: bool,
    /// Active locale for TTS dispatch. Today's CLI binds this to the
    /// resolved `--language` once and uses the same locale for the
    /// whole session.
    pub locale: primer_core::i18n::Locale,
}

/// Bound on the VAD event channel. At ~32 events/s (silero on 512-sample
/// chunks at 16 kHz), 256 holds ~8 seconds of accumulated events. The
/// audio thread sends via `blocking_send`, so saturation back-pressures
/// the audio thread (it stops draining the mic ringbuf) rather than
/// dropping events — drops would break SpeechStart/SpeechEnd pairing.
/// The cap is sized large enough that this never triggers in steady
/// state; if `run_loop` falls 8 s behind, the audio thread will block
/// briefly until the consumer catches up.
pub const VAD_EVENT_CHANNEL_CAPACITY: usize = 256;

/// Trait-injected backends consumed by `run_loop`. Production wires real
/// whisper / piper instances; tests wire mocks. The VAD lives on the
/// audio capture thread (production) or is stubbed out via direct VAD
/// events on the channel (tests), so it's not part of this struct.
///
/// ## Per-locale TTS / voice
///
/// `tts_by_locale` and `voice_by_locale` are keyed maps: each entry is
/// the TTS engine + voice profile to use when synthesising for that
/// locale. `active_locale` is the dispatch key the SPEAK phase reads
/// at each turn — bound for the lifetime of the loop in v1, but the
/// shape leaves room for future code-switching scenarios (a locale
/// switch mid-session for language-teaching) without further
/// restructuring.
///
/// Today's CLI populates exactly one entry — the active locale —
/// constructed via `LoopBackends::single_locale`. The state machine
/// and dispatch logic are untouched by this refactor.
pub struct LoopBackends {
    pub stt: Arc<dyn StreamingSpeechToText>,
    pub tts_by_locale:
        std::collections::HashMap<primer_core::i18n::Locale, Arc<dyn StreamingTextToSpeech>>,
    /// Voice profile keyed by locale. Production wires the `model_id`
    /// from `--voice` (e.g. `en_GB-alba-medium`); tests use
    /// `VoiceProfile::default()`. Piper rejects model-id mismatches,
    /// so each entry must align with the loaded voice ONNX file stem
    /// for that locale.
    pub voice_by_locale:
        std::collections::HashMap<primer_core::i18n::Locale, primer_core::speech::VoiceProfile>,
    /// Locale the SPEAK phase looks up in the maps above. v1 binds it
    /// for the lifetime of the loop.
    pub active_locale: primer_core::i18n::Locale,
}

impl LoopBackends {
    /// Convenience constructor for the single-locale case (production
    /// today, every existing test). Takes ownership of one TTS + voice
    /// pair, wraps them in single-entry maps keyed by `locale`, and
    /// sets `active_locale = locale`.
    pub fn single_locale(
        stt: Arc<dyn StreamingSpeechToText>,
        tts: Arc<dyn StreamingTextToSpeech>,
        voice: primer_core::speech::VoiceProfile,
        locale: primer_core::i18n::Locale,
    ) -> Self {
        let mut tts_by_locale = std::collections::HashMap::new();
        tts_by_locale.insert(locale, tts);
        let mut voice_by_locale = std::collections::HashMap::new();
        voice_by_locale.insert(locale, voice);
        Self {
            stt,
            tts_by_locale,
            voice_by_locale,
            active_locale: locale,
        }
    }

    /// Pre-flight: verify the dispatch maps cover `active_locale` BEFORE
    /// the SPEAK phase ever fires. v1's `single_locale` constructor
    /// satisfies this trivially; this guard exists so a future caller
    /// that builds the maps directly (e.g. from a voice-pack directory
    /// scan) cannot silently leave a hole that would surface only on
    /// the child's first sentence as a `PrimerError::Speech`.
    ///
    /// Pure (no I/O), so the CLI can call it at startup as a
    /// fail-fast check.
    pub fn ensure_active_locale_coverage(
        &self,
    ) -> std::result::Result<(), primer_core::error::PrimerError> {
        if !self.tts_by_locale.contains_key(&self.active_locale) {
            return Err(primer_core::error::PrimerError::Speech(format!(
                "no TTS configured for active locale '{locale}'. \
                 Pass --voice-onnx, --voice-config, and --voice for this \
                 locale (the model_id should match the .onnx file stem). \
                 Suggested Piper voices: 'en' \u{2192} en_US-amy-medium, \
                 'de' \u{2192} de_DE-thorsten-medium \
                 (https://huggingface.co/rhasspy/piper-voices).",
                locale = self.active_locale.pack_id(),
            )));
        }
        if !self.voice_by_locale.contains_key(&self.active_locale) {
            return Err(primer_core::error::PrimerError::Speech(format!(
                "no voice profile configured for active locale '{}'.",
                self.active_locale.pack_id(),
            )));
        }
        Ok(())
    }
}

/// Awaitable hook that blocks until the speaker has finished playing
/// every queued sample. Production wires this to a `spawn_blocking`
/// around [`crate::wait_for_drain`]; tests pass `None`.
///
/// `FnMut` (not `FnOnce`) so it can be reused across SPEAK phases. The
/// returned future is a `'static` boxed future so the hook does not
/// borrow from `run_loop`'s call frame — captures live in the closure
/// itself (typically `Arc`s to the speaker producer + errored flag).
///
/// Why a separate hook instead of doing the wait inside `on_audio`:
/// `on_audio` is sync, called from `run_loop`'s async context. A
/// `std::thread::sleep` inside it would block the tokio worker for the
/// duration of the drain (up to 5 s in production), starving any other
/// task scheduled on the same worker — and panicking on a single-threaded
/// runtime. Going through `spawn_blocking` lets the runtime schedule
/// other work onto a free worker while the drain spins on the blocking
/// pool. See PR #12 review for the full discussion.
pub type DrainHook =
    Box<dyn FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send>;

/// One commit cycle: receives transcripts on `transcript_rx`, runs the
/// LLM, returns the full Primer reply (for the caller to print and feed
/// into TTS). Production wires this through `DialogueManager`; tests
/// wire a closure that returns canned output.
///
/// **Lifetime:** the trait is NOT `'static` — `DialogueResponder` (Task 21)
/// borrows the `&mut DialogueManager`, which has its own borrowed
/// `&dyn InferenceBackend`. `run_loop` does not `tokio::spawn` the
/// responder, only `select!`s on it, so a `'static` bound would be
/// over-restrictive.
pub trait Responder: Send {
    /// Generate a response to `transcript`, calling `on_chunk` per chunk.
    /// Awaiting this future = "LLM is thinking". Cancellable via
    /// dropping the future (no `JoinHandle` involved — `run_loop` keeps
    /// the future on the stack via `tokio::pin!`).
    fn respond<'a>(
        &'a mut self,
        transcript: &'a str,
        on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>>;
}

/// Handle returned by [`super::run_loop`] for external control.
///
/// `stop_tx` ends the loop entirely (CLI Ctrl+C / GUI End-voice-mode).
/// `cancel_response_tx` aborts the in-flight LLM call + TTS synthesis
/// and returns the loop to LISTEN (GUI Stop button, Esc keypress).
pub struct LoopHandle {
    pub stop_tx: tokio::sync::oneshot::Sender<()>,
    pub cancel_response_tx: tokio::sync::mpsc::Sender<()>,
}

/// Voice loop error type. Today carries a single string variant; new
/// variants land here when the state machine grows recoverable error
/// paths.
#[derive(Debug, thiserror::Error)]
pub enum VoiceLoopError {
    #[error("voice loop error: {0}")]
    Other(String),
}
