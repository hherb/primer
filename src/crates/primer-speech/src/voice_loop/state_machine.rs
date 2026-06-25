//! State machine implementation (LISTEN → LATENT_THINK → SPEAK → LISTEN).
//!
//! Lifted from `primer-cli/src/speech_loop.rs` in PR 1 of the GUI
//! voice-mode work. Side-effects now route through [`super::LoopObserver`]
//! instead of inline `println!`s; the CLI's stdout output is preserved
//! by the `StdoutObserver` adapter in `primer-cli`.

use std::path::PathBuf;

use primer_core::error::Result;

use super::observer::{ExitReason, LoopObserver, TurnCompletePayload, VoiceState};
use super::quit_detect::is_quit_phrase;
use super::tts_markdown::strip_markdown_for_tts;

/// Spoken when the LLM call fails (rate limit, network, etc.). Goes
/// through Piper just like any normal Primer turn — the child hears
/// the apology, then we loop back to LISTEN.
const FALLBACK_LINE: &str = "Sorry, I had trouble with that. Could you ask again?";

/// Handle an LLM error inside the LATENT_THINK select arms.
///
/// Surfaces the typed error to the observer, then **drops** any chunks
/// the partial attempt managed to push into `chunk_buffer` and replaces
/// them with a single synthetic FALLBACK_LINE chunk. The replay loop
/// downstream will deliver that one chunk to the observer, so the GUI
/// chat bubble shows exactly the text TTS will speak — no truncated
/// pre-error stream stuck on screen.
///
/// Preserves the typed `InferenceError` variant when the responder
/// returned a `PrimerError::Inference(_)` so the i18n layer can render
/// the variant-specific user-facing copy (`Auth` / `RateLimited` /
/// `ServiceUnavailable` / `NetworkUnavailable` / `ModelNotFound`). Only
/// non-Inference `PrimerError` variants fall back to `Other` (carrying
/// the dev-facing display string, which the i18n layer redacts before
/// it ever reaches the user). CLAUDE.md's i18n contract forbids
/// reintroducing a `to_string().into()` wrap on the inference path —
/// it would flatten every typed variant to `Other`.
///
/// Returns the text the caller should set `accumulated` to (which then
/// flows into TTS synthesis).
fn handle_llm_err<O: LoopObserver>(
    err: primer_core::error::PrimerError,
    chunk_buffer: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    observer: &mut O,
) -> String {
    let inference_err = match err {
        primer_core::error::PrimerError::Inference(inf) => inf,
        other => other.to_string().into(),
    };
    observer.on_inference_error(&inference_err);
    let mut chunks = chunk_buffer.lock().unwrap();
    chunks.clear();
    chunks.push(FALLBACK_LINE.to_string());
    FALLBACK_LINE.to_string()
}

/// Configuration passed into [`run_loop`] / the higher-level `run` entry
/// point in `primer-cli`.
///
/// Owns its paths and the voice id so the entire config is `'static` and
/// can be moved into a spawned task. Previously borrowed (`&'a Path` /
/// `&'a str`) — the spawn-based [`run_loop`] requires `'static`.
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

use std::sync::Arc;

use primer_core::consts::speech::DEFAULT_INTER_PHRASE_SILENCE_MS;
use primer_core::speech::{StreamingSpeechToText, StreamingTextToSpeech, SynthesisEvent};

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
/// around [`primer_speech::wait_for_drain`]; tests pass `None`.
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

/// Handle returned by [`run_loop`] for external control.
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

/// Spawn-based entry point. Returns a [`LoopHandle`] for external control
/// and a `JoinHandle` so consumers (CLI, GUI) can wait for completion.
///
/// Caller must wrap any `&mut DialogueManager` in an
/// `Arc<Mutex<DialogueManager>>` (or analogue) so the boxed responder can
/// satisfy `'static`. For tests that need to share borrowed state with
/// the loop, use [`run_loop_borrowed`] instead.
#[allow(clippy::too_many_arguments)]
pub fn run_loop<O: LoopObserver>(
    backends: LoopBackends,
    events: tokio::sync::mpsc::Receiver<primer_core::speech::VadEvent>,
    responder: Box<dyn Responder + 'static>,
    on_committed_audio: Box<dyn FnMut(Vec<f32>) + Send>,
    wait_for_speaker_drain: Option<DrainHook>,
    verbose: bool,
    is_speaking: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    observer: O,
) -> (
    LoopHandle,
    tokio::task::JoinHandle<std::result::Result<Vec<String>, VoiceLoopError>>,
) {
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let (cancel_tx, cancel_rx) = tokio::sync::mpsc::channel::<()>(8);
    let handle = LoopHandle {
        stop_tx,
        cancel_response_tx: cancel_tx,
    };
    let join = tokio::spawn(async move {
        run_loop_inner(
            backends,
            events,
            responder,
            on_committed_audio,
            wait_for_speaker_drain,
            verbose,
            is_speaking,
            observer,
            stop_rx,
            cancel_rx,
        )
        .await
        .map_err(|e| VoiceLoopError::Other(e.to_string()))
    });
    (handle, join)
}

/// Same state machine as [`run_loop`] but with no spawn and a borrowed
/// responder (`'r` lifetime). Used by tests that share state with the
/// loop on the call stack.
#[allow(clippy::too_many_arguments)]
pub async fn run_loop_borrowed<'r, O: LoopObserver>(
    backends: LoopBackends,
    events: tokio::sync::mpsc::Receiver<primer_core::speech::VadEvent>,
    responder: Box<dyn Responder + 'r>,
    on_committed_audio: Box<dyn FnMut(Vec<f32>) + Send>,
    wait_for_speaker_drain: Option<DrainHook>,
    verbose: bool,
    is_speaking: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    observer: O,
) -> Result<Vec<String>> {
    // No external stop / cancel channels — tests don't need them.
    // Construct never-firing receivers so the inner function's
    // `tokio::select!` arm on them is harmless.
    let (_stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let (_cancel_tx, cancel_rx) = tokio::sync::mpsc::channel::<()>(8);
    run_loop_inner(
        backends,
        events,
        responder,
        on_committed_audio,
        wait_for_speaker_drain,
        verbose,
        is_speaking,
        observer,
        stop_rx,
        cancel_rx,
    )
    .await
}

/// Internal state-machine body shared by [`run_loop`] (spawn) and
/// [`run_loop_borrowed`] (in-place).
///
/// `verbose` gates `[stt]` debug lines on stderr (the only
/// stderr-printing site left after observer integration). Per-state
/// transitions, transcripts, chunks, completion, errors, and exits are
/// all delivered through `observer`.
///
/// `is_speaking` is the gate the audio thread checks to decide whether to
/// process or discard mic samples. The state machine flips it true at the
/// start of SPEAK and back to false after the synthesised audio has had
/// time to drain to the speaker. Tests pass `None` (mocks have no audio
/// thread).
///
/// `wait_for_speaker_drain` is awaited (in production) after the flush
/// sentinel returns and before `is_speaking` is cleared. Production wires
/// it to a `spawn_blocking` around [`primer_speech::wait_for_drain`];
/// tests pass `None` (mock speakers have no real ringbuf to drain).
///
/// `external_stop` ends the loop entirely (LISTEN, LATENT_THINK, SPEAK
/// — all states observe it as a cancel signal). `cancel_response` aborts
/// the in-flight LLM call + TTS synthesis and returns to LISTEN.
#[allow(clippy::too_many_arguments)]
async fn run_loop_inner<'r, O: LoopObserver>(
    backends: LoopBackends,
    mut events: tokio::sync::mpsc::Receiver<primer_core::speech::VadEvent>,
    mut responder: Box<dyn Responder + 'r>,
    mut on_committed_audio: Box<dyn FnMut(Vec<f32>) + Send>,
    mut wait_for_speaker_drain: Option<DrainHook>,
    verbose: bool,
    is_speaking: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    mut observer: O,
    external_stop: tokio::sync::oneshot::Receiver<()>,
    mut cancel_response: tokio::sync::mpsc::Receiver<()>,
) -> Result<Vec<String>> {
    use primer_core::speech::VadEvent;

    let mut transcripts: Vec<String> = Vec::new();
    // Counter for `TurnCompletePayload.primer_turn_index` and
    // `LoopObserver::on_response_chunk(primer_turn_index, ...)`. The
    // state machine doesn't own a session id yet (the GUI plumbing in
    // PR 3 will introduce one); `Uuid::nil()` is the placeholder.
    let mut primer_turn_index: usize = 0;
    // Pin the external_stop receiver so it can be polled inside the
    // tokio::select! arms below.
    tokio::pin!(external_stop);

    'outer: loop {
        // ── LISTEN ────────────────────────────────────────────────────
        observer.on_state_change(VoiceState::Listen, None);
        let mut stt_session = backends.stt.open_session()?;
        let mut in_speech = false;
        loop {
            // Poll events alongside the external stop signal so the loop
            // can be terminated from the LISTEN state.
            tokio::select! {
                biased;
                _ = &mut external_stop => {
                    observer.on_state_change(VoiceState::Exit, None);
                    observer.on_exit(ExitReason::UserStop);
                    return Ok(transcripts);
                }
                evt = events.recv() => {
                    let Some(event) = evt else {
                        break 'outer;
                    };
                    match event {
                        VadEvent::SpeechStart => in_speech = true,
                        VadEvent::SpeechEnd if in_speech => break,
                        _ => {}
                    }
                }
            }
        }

        // ── LATENT_THINK ──────────────────────────────────────────────
        // Loop here so a SpeechStart-cancel can resume listening with
        // the same whisper session and re-attempt the LLM call once the
        // child finishes their continuation.
        observer.on_state_change(VoiceState::LatentThink, None);
        // Accumulates STT output across the iterations of this inner
        // loop. On a SpeechStart-cancel + retry, each iteration's
        // `finalize()` only returns audio captured since the previous
        // `open_session()` — so reassignment (the pre-#103 behaviour)
        // dropped the first half of the child's utterance whenever the
        // VAD tripped a mid-sentence cancel. The fix is to append each
        // iteration's new tokens with a single separating space, after
        // both halves have already been `.trim()`'d.
        let mut transcript_so_far = String::new();
        let mut accumulated = String::new();
        // Per-turn chunk accumulator. Captured by the on_chunk closure
        // passed to the responder; replayed onto the observer after the
        // future resolves so we keep the streaming semantics (one
        // observer.on_response_chunk per actual chunk) without trying to
        // re-borrow `observer` mutably inside a closure that already
        // borrows `'r`.
        let chunk_buffer: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        loop {
            // Peek (finalize-and-reopen since whisper-cpp-plus has no
            // partial-extract API exposed here): we accept the slight
            // mock-friendliness — production whisper supports peeking via
            // process_step but the trait surface is finalize-only today.
            let segments = stt_session.finalize()?;
            let new_text = segments
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join("")
                .trim()
                .to_string();
            stt_session = backends.stt.open_session()?;
            if !new_text.is_empty() {
                if !transcript_so_far.is_empty() {
                    transcript_so_far.push(' ');
                }
                transcript_so_far.push_str(&new_text);
            }

            if transcript_so_far.is_empty() {
                if verbose {
                    eprintln!("[stt] empty transcript, looping");
                }
                break;
            }

            // Reset the chunk buffer at the top of each LLM attempt so
            // a cancelled-then-retried turn delivers only the second
            // attempt's chunks to the observer.
            chunk_buffer.lock().unwrap().clear();

            // Drive the LLM. `respond` returns the full accumulated text
            // as Ok(String); the on_chunk callback captures into
            // `chunk_buffer` for later observer replay.
            let chunk_buffer_for_cb = std::sync::Arc::clone(&chunk_buffer);
            let on_chunk: Box<dyn FnMut(&str) + Send + 'r> = Box::new(move |c: &str| {
                chunk_buffer_for_cb.lock().unwrap().push(c.to_string());
            });
            let llm_fut = responder.respond(&transcript_so_far, on_chunk);
            tokio::pin!(llm_fut);

            // Wait for either: (a) llm done, (b) VAD SpeechStart (cancel),
            // (c) external stop, (d) external cancel-response.
            // If the VAD event channel is both closed AND drained, we can
            // complete the LLM unconditionally — no more events will arrive.
            // Note: is_closed() alone returns true even with buffered messages
            // (when all senders are dropped), so we must also check is_empty().
            #[derive(PartialEq, Eq)]
            enum LatentResult {
                Completed,
                CancelledByVad,
                CancelledByUser,
                Stopped,
            }
            let outcome: LatentResult = if events.is_closed() && events.is_empty() {
                // No more VAD events possible: complete the LLM unconditionally.
                // Still poll external_stop / cancel_response so the loop
                // remains responsive to those.
                tokio::select! {
                    biased;
                    _ = &mut external_stop => LatentResult::Stopped,
                    _ = cancel_response.recv() => LatentResult::CancelledByUser,
                    res = &mut llm_fut => {
                        accumulated = match res {
                            Ok(text) => text,
                            Err(e) => handle_llm_err(e, &chunk_buffer, &mut observer),
                        };
                        LatentResult::Completed
                    }
                }
            } else {
                // `biased` ensures the LLM future gets polled before
                // events.recv() — without it, select!'s random order can
                // resolve a queued SpeechStart-cancel before the LLM
                // future is ever polled, leaving the LLM call's destructor
                // un-run because it never started. Bias makes
                // cancellation observable and testable: if cancel fires,
                // the parking future has been polled at least once.
                tokio::select! {
                    biased;
                    _ = &mut external_stop => LatentResult::Stopped,
                    _ = cancel_response.recv() => LatentResult::CancelledByUser,
                    res = &mut llm_fut => {
                        accumulated = match res {
                            Ok(text) => text,
                            Err(e) => handle_llm_err(e, &chunk_buffer, &mut observer),
                        };
                        LatentResult::Completed
                    }
                    event = events.recv() => {
                        match event {
                            Some(VadEvent::SpeechStart) => {
                                // Cancel: drop the future, loop back, keep listening.
                                LatentResult::CancelledByVad
                            }
                            Some(VadEvent::SpeechEnd) | Some(VadEvent::None) => {
                                // Spurious — shouldn't happen during LATENT_THINK
                                // since we entered on SpeechEnd. Treat as
                                // continue-waiting by completing the LLM.
                                tokio::select! {
                                    biased;
                                    _ = &mut external_stop => LatentResult::Stopped,
                                    _ = cancel_response.recv() => LatentResult::CancelledByUser,
                                    res = &mut llm_fut => {
                                        accumulated = match res {
                                            Ok(text) => text,
                                            Err(e) => handle_llm_err(e, &chunk_buffer, &mut observer),
                                        };
                                        LatentResult::Completed
                                    }
                                }
                            }
                            None => {
                                // Channel just closed mid-select: complete the LLM.
                                tokio::select! {
                                    biased;
                                    _ = &mut external_stop => LatentResult::Stopped,
                                    _ = cancel_response.recv() => LatentResult::CancelledByUser,
                                    res = &mut llm_fut => {
                                        accumulated = match res {
                                            Ok(text) => text,
                                            Err(e) => handle_llm_err(e, &chunk_buffer, &mut observer),
                                        };
                                        LatentResult::Completed
                                    }
                                }
                            }
                        }
                    }
                }
            };

            match outcome {
                LatentResult::Stopped => {
                    observer.on_state_change(VoiceState::Exit, None);
                    observer.on_exit(ExitReason::UserStop);
                    return Ok(transcripts);
                }
                LatentResult::CancelledByUser => {
                    // Back to LISTEN with a `user_cancel` hint. Drop the
                    // chunks accumulated for this aborted attempt.
                    chunk_buffer.lock().unwrap().clear();
                    observer.on_state_change(VoiceState::Listen, Some("user_cancel"));
                    continue 'outer;
                }
                LatentResult::CancelledByVad => {
                    // VAD-cancel-on-resumed-speech: drop chunks from the
                    // aborted attempt, signal the LISTEN transition, then
                    // wait for the next SpeechEnd to retry.
                    chunk_buffer.lock().unwrap().clear();
                    observer.on_state_change(VoiceState::Listen, Some("child_resumed"));
                    loop {
                        tokio::select! {
                            biased;
                            _ = &mut external_stop => {
                                observer.on_state_change(VoiceState::Exit, None);
                                observer.on_exit(ExitReason::UserStop);
                                return Ok(transcripts);
                            }
                            evt = events.recv() => {
                                let Some(event) = evt else {
                                    observer.on_state_change(VoiceState::Exit, None);
                                    observer.on_exit(ExitReason::UserStop);
                                    return Ok(transcripts);
                                };
                                if event == VadEvent::SpeechEnd {
                                    break;
                                }
                            }
                        }
                    }
                    // Re-enter LATENT_THINK for the retry.
                    observer.on_state_change(VoiceState::LatentThink, None);
                    continue;
                }
                LatentResult::Completed => break,
            }
        }

        // ── Quit check + commit transcript ────────────────────────────
        if transcript_so_far.is_empty() {
            continue;
        }
        observer.on_transcript_finalized(&transcript_so_far);
        // Replay accumulated chunks onto the observer so streaming
        // semantics (one observer.on_response_chunk per actual chunk)
        // are preserved while we still get to take `observer` by `&mut`.
        {
            let mut chunks = chunk_buffer.lock().unwrap();
            for c in chunks.drain(..) {
                observer.on_response_chunk(primer_turn_index, &c);
            }
        }
        if is_quit_phrase(&transcript_so_far, &backends.active_locale) {
            transcripts.push(transcript_so_far);
            observer.on_state_change(VoiceState::Exit, None);
            observer.on_exit(ExitReason::Keyword);
            break 'outer;
        }
        let child_turn_index = transcripts.len();
        transcripts.push(transcript_so_far);

        // ── SPEAK ─────────────────────────────────────────────────────
        if !accumulated.is_empty() {
            observer.on_state_change(VoiceState::Speak, None);
            // Strip markdown so Piper doesn't pronounce '*' / '`'. The
            // text shown to the user keeps the markdown; only the audio
            // input to the synthesiser is stripped.
            let tts_text = strip_markdown_for_tts(&accumulated);
            // Gate the mic: from here until the speaker has drained, the
            // audio thread should discard incoming samples (the Primer's
            // own voice would otherwise be transcribed as the next child
            // utterance via mic→speaker acoustic feedback).
            if let Some(flag) = is_speaking.as_ref() {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            // Dispatch on `active_locale`. v1 has a single entry so
            // these lookups can't miss in practice; we treat a miss as
            // a Speech error (rather than a panic) because LoopBackends
            // is a public-ish struct that future code might construct
            // without the convenience helper.
            let active_tts = backends
                .tts_by_locale
                .get(&backends.active_locale)
                .ok_or_else(|| {
                    primer_core::error::PrimerError::Speech(format!(
                        "no TTS configured for active locale {}",
                        backends.active_locale.pack_id()
                    ))
                })?;
            let active_voice = backends
                .voice_by_locale
                .get(&backends.active_locale)
                .ok_or_else(|| {
                    primer_core::error::PrimerError::Speech(format!(
                        "no voice profile configured for active locale {}",
                        backends.active_locale.pack_id()
                    ))
                })?;
            let mut session = active_tts.open_session(active_voice)?;
            let tts_rate = active_tts.sample_rate();
            // Inter-phrase silence inserted on each `PhraseEnd` event. Value
            // in `primer_core::consts::speech::DEFAULT_INTER_PHRASE_SILENCE_MS`.
            let inter_phrase_silence_samples =
                (tts_rate * DEFAULT_INTER_PHRASE_SILENCE_MS / 1000) as usize;
            let mut on_event = |event: SynthesisEvent| match event {
                SynthesisEvent::Audio(chunk) => on_committed_audio(chunk.samples),
                SynthesisEvent::PhraseEnd => {
                    on_committed_audio(vec![0.0_f32; inter_phrase_silence_samples])
                }
            };
            // Sync calls deliberately not wrapped in `tokio::task::spawn_blocking`,
            // even though the trait's `# Blocking` doc-section asks async callers
            // to do so. Rationale per production backend:
            //
            // * **macOS-native CLI** (`Builder::new_current_thread()` on the OS
            //   main thread): `spawn_blocking` would hop work to a worker
            //   thread; `synthesize_streaming` then sees `is_main_thread() ==
            //   false` and takes the GCD-bounce path; that path drains via
            //   `dispatch2::DispatchQueue::main().exec_async(...)` which only
            //   runs when main is owned by `NSApplicationMain` or
            //   `dispatch_main()`. The CLI's main thread is running tokio's
            //   current-thread runtime instead — the bounced work is queued
            //   onto GCD main but nothing drains it, so synthesis stalls. The
            //   direct sync call keeps `push_text` on main and lets
            //   `synthesize_streaming` take the main-thread `runUntilDate`
            //   path — the architecture the CLI binary is built around (see
            //   the `run_tokio_on_main` doc-comment in
            //   `primer-cli/src/main.rs` for the full deadlock argument).
            //
            // * **macOS-native GUI** (multi-thread tokio + Tauri's
            //   `NSApplicationMain` on main): the direct call blocks one
            //   tokio worker for the synth duration. Other workers keep
            //   running, and the GCD bounce drains because Tauri owns main.
            //   A `spawn_blocking` wrap would free the one worker but
            //   `push_text` still has to bounce to main eventually, so the
            //   net wallclock cost on synthesis is unchanged. Worker-block
            //   accepted as a known cost; background tokio tasks queued
            //   during SPEAK catch up at the next turn via
            //   `DialogueManager::await_pending_post_response`.
            //
            // * **Piper backend (default Linux speech build)**:
            //   `PiperSession::synth_phrase` runs ONNX inference synchronously
            //   on the calling worker. No main-thread requirement; the wrap
            //   would free one worker but the multi-thread runtime has
            //   several so this isn't a regression to leave it.
            //
            // * **Android-native (`android-native`, Tauri GUI)**:
            //   `AndroidTtsSession::push_text` blocks in JNI until the OS
            //   `TextToSpeech` engine reports `onDone` (bounded by the Kotlin
            //   `TTS_SPEAK_TIMEOUT`). No main-thread requirement — only the
            //   recognizer methods post to the main Looper; `tts.speak` runs
            //   on the calling thread. So this is the Piper case: one
            //   multi-thread-tokio worker is blocked for the synth, the
            //   others keep running. Worker-block accepted as a known cost.
            //
            // What would have to change for the wrap to matter:
            // - A non-Apple multi-tenant runtime where the synth worker-block
            //   starves dense background tasks during SPEAK.
            // - A new backend that can synthesize off-main without bouncing
            //   to a specific thread (so `spawn_blocking` would have no
            //   side-effect on which thread the synth runs on).
            // - The macOS-native CLI moving to multi-thread tokio (which
            //   itself requires either calling `NSApplicationMain` or a
            //   GCD-main pump — see the `run_tokio_on_main` doc-comment
            //   in `primer-cli/src/main.rs`).
            session.push_text(&tts_text, &mut on_event)?;
            session.finalize(&mut on_event)?;
            // Flush sentinel: empty Vec signals on_audio to drain any
            // resampler-leftover tail. Mock callbacks no-op on empty input.
            on_committed_audio(Vec::new());
            // Wait for cpal to actually empty the speaker ringbuf
            // before clearing the mic gate. Going through `spawn_blocking`
            // (in the hook the caller wired) keeps this off the tokio
            // worker so other async work isn't starved during the
            // drain wait. Replaces the old `samples / tts_rate + 0.4s`
            // heuristic sleep — exact instead of "fixed margin that's
            // too short on slow hardware and too long on fast hardware".
            if let Some(hook) = wait_for_speaker_drain.as_mut() {
                hook().await;
            }
            if let Some(flag) = is_speaking.as_ref() {
                flag.store(false, std::sync::atomic::Ordering::SeqCst);
            }
            // Drain any events the audio thread may have queued in the
            // narrow window between un-gating and the next LISTEN read:
            // the speaker→mic acoustic tail of the Primer's own voice
            // can otherwise emit a stale SpeechStart/SpeechEnd that would
            // be processed as a child utterance. Tradeoff: a child whose
            // SpeechStart lands inside this same window (a few ms) loses
            // that event — but their continuing speech immediately fires
            // a fresh SpeechStart on the next VAD chunk, so the start is
            // delayed by ~32 ms at worst. Acceptable for the no-barge-in
            // model.
            while events.try_recv().is_ok() {}
        }

        // Always fire on_response_complete at the end of a turn, even
        // when `accumulated` was whitespace-only and we skipped SPEAK.
        // Consumers (GUI session journal) need to know a turn finished.
        observer.on_response_complete(TurnCompletePayload {
            session_id: uuid::Uuid::nil(),
            child_turn_index,
            primer_turn_index,
        });
        primer_turn_index += 1;
    }

    // Outer loop exited via `events.recv()` returning None (channel
    // drained and closed). No quit phrase, no user stop — the audio
    // thread is gone, treat as a user-initiated end.
    observer.on_state_change(VoiceState::Exit, None);
    observer.on_exit(ExitReason::UserStop);
    Ok(transcripts)
}

// NOTE: The DialogueResponder adapter, ChannelStt adapter,
// `pub async fn run` entry point, and `run_audio_thread` body all stay
// in `primer-cli/src/speech_loop.rs` — they are CLI-specific glue around
// the state machine. This file owns only the state machine itself and
// the public types/traits the CLI and GUI both consume.

#[cfg(test)]
mod mocks;
