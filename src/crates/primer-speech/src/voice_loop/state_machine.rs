//! State machine implementation (LISTEN → LATENT_THINK → SPEAK → LISTEN).
//!
//! Lifted from `primer-cli/src/speech_loop.rs` in PR 1 of the GUI
//! voice-mode work. Side-effects now route through [`super::LoopObserver`]
//! instead of inline `println!`s; the CLI's stdout output is preserved
//! by the `StdoutObserver` adapter in `primer-cli`.

use std::path::PathBuf;

use primer_core::error::Result;

use super::observer::{ExitReason, LoopObserver, TurnCompletePayload, VoiceState};

/// Per-locale quit phrases. If heard in the child's transcript, the
/// session ends. Case-insensitive, word-boundary match (see
/// [`is_quit_phrase`]). Each locale ships its own set so a child can
/// quit in the language they speak — without a locale-aware list, the
/// German voice mode silently lacks any voice-keyword end affordance.
///
/// Adding a new locale: append a `(pack_id, &[phrase, ...])` row. The
/// pack_id must match `Locale::pack_id()` for the corresponding locale.
fn quit_phrases_for(locale: &primer_core::i18n::Locale) -> &'static [&'static str] {
    match locale.pack_id() {
        "de" => &[
            // "Tschüss" (informal goodbye) — the most natural for a child.
            "tschüss",
            // Formal goodbye.
            "auf wiedersehen",
            // Primer-direct variants, mirroring the EN set.
            "bye primer",
            "stop primer",
        ],
        // English is the default for any unrecognised locale.
        _ => &["goodbye", "bye primer", "stop primer"],
    }
}

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

/// Returns true if `transcript`, after trimming surrounding whitespace
/// and punctuation, **equals** one of `locale`'s quit phrases
/// (case-insensitive).
///
/// Why exact-equality rather than `contains` or word-boundary: the
/// pre-fix `contains` would end the session on *"I don't want to stop
/// primer"* because the substring `"stop primer"` matched. Word-boundary
/// matching alone doesn't help — end-of-string is itself a word boundary.
/// The only safe contract for an auto-quit voice keyword is "the child
/// said exactly the keyword, nothing else." Children can always say
/// "goodbye" by itself; this also matches the way real children end
/// conversations.
///
/// Trimming punctuation handles Whisper's habit of producing trailing
/// `.` / `!` / `?` on a finalized utterance: `"Goodbye!"` still ends the
/// session. Internal whitespace is normalised to single spaces so the
/// transcript `"bye   primer"` matches `"bye primer"`.
fn is_quit_phrase(transcript: &str, locale: &primer_core::i18n::Locale) -> bool {
    let normalised = normalise_for_match(transcript);
    quit_phrases_for(locale)
        .iter()
        .any(|p| normalised == normalise_for_match(p))
}

/// Lowercase, strip leading/trailing whitespace + punctuation, collapse
/// internal whitespace to single spaces. The result is the canonical
/// form used for quit-phrase equality.
///
/// `char::is_alphanumeric` is Unicode-aware so German `ü`/`ö`/`ä` are
/// preserved; only punctuation and non-letter symbols get trimmed.
fn normalise_for_match(s: &str) -> String {
    let lower = s.to_lowercase();
    let trimmed = lower.trim_matches(|c: char| !c.is_alphanumeric());
    let mut out = String::with_capacity(trimmed.len());
    let mut prev_was_space = false;
    for c in trimmed.chars() {
        if c.is_whitespace() {
            if !prev_was_space {
                out.push(' ');
                prev_was_space = true;
            }
        } else {
            out.push(c);
            prev_was_space = false;
        }
    }
    out
}

/// Strip markdown emphasis markers so Piper's espeak phonemizer doesn't
/// pronounce them ("*why*" → "asterisks why asterisks"). Paired
/// `*emphasis*` and `**strong**` markers are removed; paired
/// `` `code` `` markers are removed. Bare unmatched `*` or `` ` `` are
/// left in place. A `*` (or run of `*`) sandwiched between digits is
/// treated as multiplication and replaced with " times " so `5*3=15`
/// reads as "5 times 3=15" instead of "53=15". Underscore-emphasis is
/// rare and ambiguous (shows up in identifiers too) — left alone.
///
/// Recursion: the function recurses into the inner content of paired
/// markers (e.g. the `5*3=15` inside `**5*3=15**`). Each recursive call
/// receives a strict substring, so depth is bounded by `input.len()/2`
/// and stack overflow is impossible for any realistic Primer turn.
fn strip_markdown_for_tts(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '*' {
            if let Some(end) = consume_digit_times(&chars, i) {
                out.push_str(" times ");
                i = end;
                continue;
            }
            let marker = if i + 1 < chars.len() && chars[i + 1] == '*' {
                2
            } else {
                1
            };
            if let Some(close) = find_paired_marker(&chars, i + marker, marker, '*') {
                let inner: String = chars[i + marker..close].iter().collect();
                out.push_str(&strip_markdown_for_tts(&inner));
                i = close + marker;
                continue;
            }
            out.push('*');
            i += 1;
        } else if c == '`' {
            if let Some(close) = find_paired_marker(&chars, i + 1, 1, '`') {
                let inner: String = chars[i + 1..close].iter().collect();
                out.push_str(&strip_markdown_for_tts(&inner));
                i = close + 1;
                continue;
            }
            out.push('`');
            i += 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// If `chars[i]` is the start of a digit-bounded run of `*` (e.g. `*`
/// or `**` flanked by digits), return the index just past the run.
/// Otherwise return `None`. The caller emits " times " in that case.
///
/// Only ASCII integer boundaries match: `1.5*2`, `1,000*5`, and any
/// non-ASCII numeral won't trigger the rewrite. This is the right
/// trade-off for a children's tutor (integer multiplication dominates),
/// and keeps the heuristic narrow enough that it never fires on prose.
fn consume_digit_times(chars: &[char], i: usize) -> Option<usize> {
    if i == 0 || !chars[i - 1].is_ascii_digit() {
        return None;
    }
    let mut j = i;
    while j < chars.len() && chars[j] == '*' {
        j += 1;
    }
    if j < chars.len() && chars[j].is_ascii_digit() {
        Some(j)
    } else {
        None
    }
}

/// Find the next run of exactly `marker_len` consecutive `marker`
/// characters starting at or after `start`, not adjacent to another
/// `marker` (so a `*` inside a `**` run never matches a single-`*`
/// search and vice versa). Returns the start index of that run.
fn find_paired_marker(
    chars: &[char],
    start: usize,
    marker_len: usize,
    marker: char,
) -> Option<usize> {
    let n = chars.len();
    let mut i = start;
    while i + marker_len <= n {
        let matches = (0..marker_len).all(|k| chars[i + k] == marker);
        let prev_ok = i == 0 || chars[i - 1] != marker;
        let next_ok = i + marker_len >= n || chars[i + marker_len] != marker;
        if matches && prev_ok && next_ok {
            return Some(i);
        }
        i += 1;
    }
    None
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
mod mocks {
    use std::sync::{Arc, Mutex};

    use primer_core::error::Result;
    use primer_core::speech::{
        AudioChunk, Named, StreamingSpeechToText, StreamingTextToSpeech, SynthesisEvent,
        SynthesisSession, TranscriptSegment, TranscriptionSession, VoiceProfile,
    };

    /// Mock streaming STT: emits a fixed transcript on `finalize`.
    pub struct MockStreamingStt {
        finalize_text: String,
    }

    impl MockStreamingStt {
        pub fn new(finalize_text: impl Into<String>) -> Self {
            Self {
                finalize_text: finalize_text.into(),
            }
        }
    }

    impl Named for MockStreamingStt {
        fn name(&self) -> &str {
            "mock-stt"
        }
    }

    impl StreamingSpeechToText for MockStreamingStt {
        fn sample_rate(&self) -> u32 {
            16_000
        }
        fn open_session(&self) -> Result<Box<dyn TranscriptionSession>> {
            Ok(Box::new(MockSttSession {
                final_text: self.finalize_text.clone(),
            }))
        }
    }

    struct MockSttSession {
        final_text: String,
    }

    impl TranscriptionSession for MockSttSession {
        fn push_audio(&mut self, _samples: &[f32]) -> Result<Vec<TranscriptSegment>> {
            Ok(vec![])
        }
        fn finalize(self: Box<Self>) -> Result<Vec<TranscriptSegment>> {
            Ok(vec![TranscriptSegment {
                text: self.final_text,
                start_ms: 0,
                end_ms: 1_000,
            }])
        }
    }

    /// Scriptable streaming STT: yields a different `finalize` text on
    /// each successive `open_session()` call, draining a FIFO queue. Once
    /// the queue is exhausted any further session finalizes to the empty
    /// string. Required to exercise the cancel-and-retry path where the
    /// first and second STT sessions return different partial transcripts
    /// — see issue #103 / `cancel_and_retry_stitches_full_transcript`.
    pub struct ScriptedStreamingStt {
        pending: Arc<Mutex<std::collections::VecDeque<String>>>,
    }

    impl ScriptedStreamingStt {
        pub fn new<I, S>(texts: I) -> Self
        where
            I: IntoIterator<Item = S>,
            S: Into<String>,
        {
            Self {
                pending: Arc::new(Mutex::new(texts.into_iter().map(Into::into).collect())),
            }
        }
    }

    impl Named for ScriptedStreamingStt {
        fn name(&self) -> &str {
            "scripted-stt"
        }
    }

    impl StreamingSpeechToText for ScriptedStreamingStt {
        fn sample_rate(&self) -> u32 {
            16_000
        }
        fn open_session(&self) -> Result<Box<dyn TranscriptionSession>> {
            let next = self.pending.lock().unwrap().pop_front().unwrap_or_default();
            Ok(Box::new(MockSttSession { final_text: next }))
        }
    }

    /// Mock streaming TTS: emits one fixed AudioChunk per `push_text` call.
    pub struct MockStreamingTts {
        chunk_samples: usize,
    }

    impl MockStreamingTts {
        pub fn new(chunk_samples: usize) -> Self {
            Self { chunk_samples }
        }
    }

    impl Named for MockStreamingTts {
        fn name(&self) -> &str {
            "mock-tts"
        }
    }

    impl StreamingTextToSpeech for MockStreamingTts {
        fn sample_rate(&self) -> u32 {
            22_050
        }
        fn open_session(&self, _voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>> {
            Ok(Box::new(MockTtsSession {
                chunk_samples: self.chunk_samples,
            }))
        }
    }

    struct MockTtsSession {
        chunk_samples: usize,
    }

    impl SynthesisSession for MockTtsSession {
        fn push_text(
            &mut self,
            text: &str,
            on_event: &mut dyn FnMut(SynthesisEvent),
        ) -> Result<()> {
            if text.is_empty() {
                return Ok(());
            }
            on_event(SynthesisEvent::Audio(AudioChunk {
                samples: vec![0.5; self.chunk_samples],
                sample_rate: 22_050,
            }));
            on_event(SynthesisEvent::PhraseEnd);
            Ok(())
        }

        fn finalize(self: Box<Self>, _on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()> {
            Ok(())
        }
    }

    /// Sample count emitted per Audio event by [`TimedMockTts`]. Small
    /// enough that on_committed_audio doesn't do meaningful work per
    /// call; large enough that the consumer's audio plumbing accepts
    /// each push without backpressure.
    const TIMED_MOCK_SAMPLES_PER_CHUNK: usize = 64;
    /// Sample rate of the timed-mock chunks. Matches [`MockStreamingTts`]
    /// (Piper-class voice rate).
    const TIMED_MOCK_SAMPLE_RATE: u32 = 22_050;
    /// Wallclock delay between successive Audio events emitted by
    /// [`TimedMockTts`]. The TTFA test relies on a real wallclock gap so
    /// the consumer's per-event `Instant::now()` records show separation;
    /// `std::thread::sleep` is correct here because `push_text` is
    /// synchronous.
    const TIMED_MOCK_INTER_CHUNK_MS: u64 = 50;

    /// Streaming TTS mock that injects real wallclock delays between
    /// Audio events, used to verify the consumer doesn't buffer chunks
    /// before forwarding them to `on_committed_audio`.
    ///
    /// Each non-empty `push_text` emits three Audio events at
    /// [`TIMED_MOCK_INTER_CHUNK_MS`]-millisecond intervals (with sample
    /// values 0.1, 0.2, 0.3 as identity markers), then `PhraseEnd`.
    /// Total per-push wallclock ≈ 2 × interval = 100 ms. Empty input
    /// emits no events (same shape as the production sessions).
    ///
    /// **Why `std::thread::sleep` inside an `async`-ish call path is OK
    /// here:** `SynthesisSession::push_text` is a synchronous trait
    /// method by design (production backends do CPU-heavy ONNX
    /// inference inline; see the trait's `# Blocking` note). The TTFA
    /// test needs a *real* wallclock gap between Audio events so the
    /// consumer's per-event `Instant::now()` records can show
    /// separation — `tokio::time::sleep().await` would be inappropriate
    /// because the trait isn't async, and `std::thread::yield_now()`
    /// gives no measurable gap. Total wallclock cost per push is
    /// ~100 ms, briefly blocking one tokio worker; tolerable in a unit
    /// test mock.
    pub struct TimedMockTts;

    impl Named for TimedMockTts {
        fn name(&self) -> &str {
            "timed-mock-tts"
        }
    }

    impl StreamingTextToSpeech for TimedMockTts {
        fn sample_rate(&self) -> u32 {
            TIMED_MOCK_SAMPLE_RATE
        }
        fn open_session(&self, _voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>> {
            Ok(Box::new(TimedMockTtsSession))
        }
    }

    struct TimedMockTtsSession;

    impl SynthesisSession for TimedMockTtsSession {
        fn push_text(
            &mut self,
            text: &str,
            on_event: &mut dyn FnMut(SynthesisEvent),
        ) -> Result<()> {
            if text.is_empty() {
                return Ok(());
            }
            for (i, marker) in [0.1_f32, 0.2, 0.3].iter().enumerate() {
                on_event(SynthesisEvent::Audio(AudioChunk {
                    samples: vec![*marker; TIMED_MOCK_SAMPLES_PER_CHUNK],
                    sample_rate: TIMED_MOCK_SAMPLE_RATE,
                }));
                if i < 2 {
                    std::thread::sleep(std::time::Duration::from_millis(TIMED_MOCK_INTER_CHUNK_MS));
                }
            }
            on_event(SynthesisEvent::PhraseEnd);
            Ok(())
        }

        fn finalize(self: Box<Self>, _on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()> {
            Ok(())
        }
    }

    /// Observer-event record used by the unit tests. The original
    /// `speech_loop.rs` asserted side-effects via captured channels;
    /// after the observer refactor each test inspects the recorded
    /// event stream against the expected sequence. `TurnCompletePayload`
    /// has no `PartialEq` so the enum uses pattern-matching assertions
    /// instead of `==`.
    ///
    /// `#[allow(dead_code)]` is applied because individual tests only
    /// pattern-match a subset of fields; the unused-field analysis
    /// fires across the whole enum.
    #[allow(dead_code)]
    #[derive(Debug, Clone)]
    pub enum MockEvent {
        StateChange {
            state: super::VoiceState,
            hint: Option<String>,
        },
        Transcript(String),
        Chunk {
            primer_turn_index: usize,
            text: String,
        },
        Complete(super::TurnCompletePayload),
        InferenceError(String),
        Exit(super::ExitReason),
    }

    /// Test observer that records every callback into a shared `Vec`.
    #[derive(Clone, Default)]
    pub struct MockObserver(pub Arc<Mutex<Vec<MockEvent>>>);

    impl MockObserver {
        pub fn new() -> Self {
            Self(Arc::new(Mutex::new(Vec::new())))
        }
        pub fn events(&self) -> Vec<MockEvent> {
            self.0.lock().unwrap().clone()
        }
    }

    impl super::LoopObserver for MockObserver {
        fn on_state_change(&mut self, state: super::VoiceState, hint: Option<&str>) {
            self.0.lock().unwrap().push(MockEvent::StateChange {
                state,
                hint: hint.map(String::from),
            });
        }
        fn on_transcript_finalized(&mut self, text: &str) {
            self.0
                .lock()
                .unwrap()
                .push(MockEvent::Transcript(text.to_string()));
        }
        fn on_response_chunk(&mut self, primer_turn_index: usize, chunk: &str) {
            self.0.lock().unwrap().push(MockEvent::Chunk {
                primer_turn_index,
                text: chunk.to_string(),
            });
        }
        fn on_response_complete(&mut self, payload: super::TurnCompletePayload) {
            self.0.lock().unwrap().push(MockEvent::Complete(payload));
        }
        fn on_inference_error(&mut self, err: &primer_core::error::InferenceError) {
            self.0
                .lock()
                .unwrap()
                .push(MockEvent::InferenceError(format!("{err:?}")));
        }
        fn on_exit(&mut self, reason: super::ExitReason) {
            self.0.lock().unwrap().push(MockEvent::Exit(reason));
        }
    }

    #[test]
    fn mock_streaming_stt_finalizes_canned_text() {
        let stt = MockStreamingStt::new("hello world");
        let session = stt.open_session().unwrap();
        let segs = session.finalize().unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "hello world");
    }

    #[test]
    fn mock_streaming_tts_emits_one_chunk_per_text() {
        let tts = MockStreamingTts::new(100);
        let voice = VoiceProfile::default();
        let mut session = tts.open_session(&voice).unwrap();

        let mut count_non_empty: u32 = 0;
        session
            .push_text("hi.", &mut |e| {
                if let SynthesisEvent::Audio(_) = e {
                    count_non_empty += 1;
                }
            })
            .unwrap();
        assert_eq!(count_non_empty, 1);

        let mut count_empty: u32 = 0;
        session.push_text("", &mut |_| count_empty += 1).unwrap();
        assert_eq!(count_empty, 0);
    }

    /// Pin the state machine's consumer-side guarantee that PCM events
    /// reach `on_committed_audio` AS THEY ARRIVE — not buffered until
    /// after `push_text` returns.
    ///
    /// [`TimedMockTts`] emits three Audio events at 50 ms intervals
    /// ([`TIMED_MOCK_INTER_CHUNK_MS`]). We record `Instant::now()` at
    /// each `on_committed_audio` call and assert the FIRST 0.1-marker's
    /// timestamp precedes the LAST 0.3-marker's timestamp by ≥80 ms (vs.
    /// the 100 ms total inter-event budget — 20 ms slack absorbs CI
    /// noise). A consumer that buffered all chunks before forwarding
    /// would see all three timestamps clustered within microseconds and
    /// fail this assertion.
    #[tokio::test]
    async fn streaming_chunks_reach_speaker_before_phrase_completes() {
        use std::sync::Mutex;
        use std::time::Instant;

        use primer_core::speech::VadEvent;

        /// Lower bound on the wallclock gap between the first and last
        /// committed Audio sample. Allows ~20 ms of slack on top of the
        /// nominal `2 × TIMED_MOCK_INTER_CHUNK_MS = 100 ms` budget.
        const STREAMING_GAP_FLOOR_MS: u64 = 80;
        /// Float-equality tolerance for matching the 0.1 / 0.3 identity
        /// markers in the committed sample stream. The markers are
        /// exact `f32` literals; the tolerance only guards against any
        /// future resampler that might be inserted between mock and
        /// consumer.
        const MARKER_EPS: f32 = 0.01;

        let backends = super::LoopBackends::single_locale(
            Arc::new(MockStreamingStt::new("hello primer")),
            Arc::new(TimedMockTts),
            primer_core::speech::VoiceProfile::default(),
            primer_core::i18n::Locale::English,
        );

        let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
        event_tx.try_send(VadEvent::SpeechStart).unwrap();
        event_tx.try_send(VadEvent::SpeechEnd).unwrap();
        drop(event_tx);

        /// `(wallclock, first-sample-value)` for each non-empty
        /// on_audio call. The first sample uniquely identifies which
        /// TimedMockTts marker (0.1 / 0.2 / 0.3) drove the commit.
        type TimelineEntry = (Instant, Option<f32>);

        // Record per non-empty on_audio call. Empty pushes (inter-phrase
        // silence frames) are filtered out so the markers' positions are
        // unambiguous.
        let timeline: Arc<Mutex<Vec<TimelineEntry>>> = Arc::new(Mutex::new(Vec::new()));
        let timeline_cb = Arc::clone(&timeline);
        let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(move |samples| {
            if !samples.is_empty() {
                timeline_cb
                    .lock()
                    .unwrap()
                    .push((Instant::now(), samples.first().copied()));
            }
        });

        struct EchoResponder;
        impl super::Responder for EchoResponder {
            fn respond<'a>(
                &'a mut self,
                transcript: &'a str,
                mut on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = primer_core::error::Result<String>>
                        + Send
                        + 'a,
                >,
            > {
                let owned = transcript.to_string();
                Box::pin(async move {
                    on_chunk(&owned);
                    Ok(owned)
                })
            }
        }

        let observer = MockObserver::new();
        super::run_loop_borrowed(
            backends,
            event_rx,
            Box::new(EchoResponder),
            on_audio,
            None,
            false,
            None,
            observer.clone(),
        )
        .await
        .expect("loop runs to completion");

        let recorded = timeline.lock().unwrap();
        let first = recorded
            .iter()
            .find(|(_, v)| v.map(|x| (x - 0.1).abs() < MARKER_EPS).unwrap_or(false))
            .expect("0.1 marker was committed");
        let last = recorded
            .iter()
            .rfind(|(_, v)| v.map(|x| (x - 0.3).abs() < MARKER_EPS).unwrap_or(false))
            .expect("0.3 marker was committed");
        let gap = last.0.duration_since(first.0);
        assert!(
            gap >= std::time::Duration::from_millis(STREAMING_GAP_FLOOR_MS),
            "expected ≥{STREAMING_GAP_FLOOR_MS}ms between first and last Audio commit \
             (true streaming); got {gap:?}"
        );
    }

    #[test]
    fn scripted_stt_drains_queue_then_returns_empty() {
        let stt = ScriptedStreamingStt::new(vec!["first", "second"]);
        let s1 = stt.open_session().unwrap().finalize().unwrap();
        assert_eq!(s1[0].text, "first");
        let s2 = stt.open_session().unwrap().finalize().unwrap();
        assert_eq!(s2[0].text, "second");
        // Queue exhausted: subsequent sessions finalize to the empty
        // string (the run-loop's empty-check handles this gracefully).
        let s3 = stt.open_session().unwrap().finalize().unwrap();
        assert_eq!(s3[0].text, "");
    }

    /// Test 1 — happy path: scripted SpeechEnd → LLM called with expected
    /// transcript → audio chunks committed → run_loop returns transcripts.
    #[tokio::test]
    async fn happy_path_records_one_round_trip() {
        use std::sync::Mutex;

        use primer_core::speech::VadEvent;

        let backends = super::LoopBackends::single_locale(
            Arc::new(MockStreamingStt::new("hello primer")),
            Arc::new(MockStreamingTts::new(64)),
            primer_core::speech::VoiceProfile::default(),
            primer_core::i18n::Locale::English,
        );

        let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
        event_tx.try_send(VadEvent::SpeechStart).unwrap();
        event_tx.try_send(VadEvent::SpeechEnd).unwrap();
        drop(event_tx);

        let captured_transcript = Arc::new(Mutex::new(String::new()));
        let captured_clone = Arc::clone(&captured_transcript);
        struct ScriptedResponder {
            captured_transcript: Arc<Mutex<String>>,
        }
        impl super::Responder for ScriptedResponder {
            fn respond<'a>(
                &'a mut self,
                transcript: &'a str,
                mut on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = primer_core::error::Result<String>>
                        + Send
                        + 'a,
                >,
            > {
                *self.captured_transcript.lock().unwrap() = transcript.to_string();
                Box::pin(async move {
                    on_chunk("Hello, child.");
                    Ok("Hello, child.".to_string())
                })
            }
        }
        let responder = Box::new(ScriptedResponder {
            captured_transcript: captured_clone,
        });

        let committed = Arc::new(Mutex::new(Vec::<f32>::new()));
        let committed_clone = Arc::clone(&committed);
        let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(move |samples| {
            committed_clone.lock().unwrap().extend(samples);
        });

        let observer = MockObserver::new();
        let result = super::run_loop_borrowed(
            backends,
            event_rx,
            responder,
            on_audio,
            None,
            false,
            None,
            observer.clone(),
        )
        .await;
        let transcripts = result.expect("loop ok");
        assert_eq!(transcripts, vec!["hello primer".to_string()]);
        assert_eq!(*captured_transcript.lock().unwrap(), "hello primer");
        assert!(!committed.lock().unwrap().is_empty(), "audio was committed");

        // Verify the observer saw a complete state journey: enter LISTEN,
        // finalize the transcript, transition through LATENT_THINK and
        // SPEAK, then back to LISTEN for the next utterance.
        let events = observer.events();
        assert!(
            events.iter().any(|e| matches!(
                e,
                MockEvent::StateChange {
                    state: super::VoiceState::Listen,
                    ..
                }
            )),
            "saw at least one Listen state: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, MockEvent::Transcript(t) if t == "hello primer")),
            "transcript finalized event: {events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(
                e,
                MockEvent::StateChange {
                    state: super::VoiceState::Speak,
                    ..
                }
            )),
            "entered SPEAK: {events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(e, MockEvent::Complete(_))),
            "fired Complete: {events:?}"
        );
    }

    /// Test 4 — quit phrase short-circuits SPEAK: child says "goodbye",
    /// the responder returns an empty string. The loop pushes the
    /// transcript, hits the quit-phrase branch, and exits before
    /// reaching SPEAK — so no audio is committed regardless of the
    /// (empty) responder output.
    #[tokio::test]
    async fn quit_phrase_short_circuits_speak() {
        use primer_core::speech::VadEvent;

        let backends = super::LoopBackends::single_locale(
            Arc::new(MockStreamingStt::new("goodbye")),
            Arc::new(MockStreamingTts::new(64)),
            primer_core::speech::VoiceProfile::default(),
            primer_core::i18n::Locale::English,
        );

        let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
        event_tx.try_send(VadEvent::SpeechStart).unwrap();
        event_tx.try_send(VadEvent::SpeechEnd).unwrap();
        drop(event_tx);

        struct EmptyResponder;
        impl super::Responder for EmptyResponder {
            fn respond<'a>(
                &'a mut self,
                _transcript: &'a str,
                mut on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = primer_core::error::Result<String>>
                        + Send
                        + 'a,
                >,
            > {
                Box::pin(async move {
                    on_chunk("");
                    Ok(String::new())
                })
            }
        }

        let committed: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let committed_clone = Arc::clone(&committed);
        let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(move |samples| {
            committed_clone.lock().unwrap().extend(samples);
        });

        let observer = MockObserver::new();
        let result = super::run_loop_borrowed(
            backends,
            event_rx,
            Box::new(EmptyResponder),
            on_audio,
            None,
            false,
            None,
            observer.clone(),
        )
        .await;
        let transcripts = result.expect("loop ok");
        assert_eq!(transcripts, vec!["goodbye".to_string()]);
        assert!(
            committed.lock().unwrap().is_empty(),
            "quit phrase exits before SPEAK"
        );
        // Quit phrase fires Exit(Keyword) and never enters SPEAK.
        let events = observer.events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, MockEvent::Exit(super::ExitReason::Keyword))),
            "Exit(Keyword) fired: {events:?}"
        );
        assert!(
            !events.iter().any(|e| matches!(
                e,
                MockEvent::StateChange {
                    state: super::VoiceState::Speak,
                    ..
                }
            )),
            "did NOT enter SPEAK: {events:?}"
        );
    }

    /// Test 2 — cancel on resumed speech: SpeechEnd, then SpeechStart
    /// before LLM completes. The LLM is cancelled. When the next
    /// SpeechEnd arrives, the responder is called again with the
    /// concatenated transcript. Audio commits on the second attempt.
    #[tokio::test]
    async fn cancel_on_resumed_speech_retries_after_continuation() {
        use primer_core::speech::VadEvent;

        // The MockStreamingStt always finalizes the SAME canned text. To
        // simulate "first attempt: 'why does'; second: 'why does the sky
        // look blue'", we need a smarter mock — but for the unit test
        // we accept that both attempts return the same canned text. The
        // assertion is about cancellation, not transcript stitching.
        let backends = super::LoopBackends::single_locale(
            Arc::new(MockStreamingStt::new("why does the sky look blue")),
            Arc::new(MockStreamingTts::new(64)),
            primer_core::speech::VoiceProfile::default(),
            primer_core::i18n::Locale::English,
        );

        let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
        // First SpeechStart → SpeechEnd: triggers LATENT_THINK.
        event_tx.try_send(VadEvent::SpeechStart).unwrap();
        event_tx.try_send(VadEvent::SpeechEnd).unwrap();
        // Then SpeechStart mid-LATENT_THINK: triggers cancel.
        event_tx.try_send(VadEvent::SpeechStart).unwrap();
        // Then SpeechEnd: retry LATENT_THINK.
        event_tx.try_send(VadEvent::SpeechEnd).unwrap();
        drop(event_tx);

        let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let cc_clone = Arc::clone(&call_count);
        // Cancel-drop counter — bumped only by the guard inside the
        // PARKING branch. The succeeding future never enters that
        // branch, so this counter discriminates "cancelled future was
        // dropped" from "any future was dropped." `== 1` is the exact
        // cancellation contract: the in-flight LLM call's destructor
        // runs, releasing resources.
        let cancel_drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let cd_clone = Arc::clone(&cancel_drops);
        struct CountingResponder {
            count: Arc<std::sync::atomic::AtomicUsize>,
            cancel_drops: Arc<std::sync::atomic::AtomicUsize>,
        }
        struct CancelGuard {
            drops: Arc<std::sync::atomic::AtomicUsize>,
        }
        impl Drop for CancelGuard {
            fn drop(&mut self) {
                self.drops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
        impl super::Responder for CountingResponder {
            fn respond<'a>(
                &'a mut self,
                _transcript: &'a str,
                mut on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = primer_core::error::Result<String>>
                        + Send
                        + 'a,
                >,
            > {
                let n = self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let cancel_drops = Arc::clone(&self.cancel_drops);
                Box::pin(async move {
                    if n == 0 {
                        // First call: park forever so the cancel arm wins.
                        // CancelGuard scoped to this branch only — its
                        // Drop only runs when the parking future itself
                        // is dropped (i.e. cancelled).
                        let _guard = CancelGuard {
                            drops: cancel_drops,
                        };
                        std::future::pending::<()>().await;
                        unreachable!()
                    }
                    // Second call: respond promptly. No CancelGuard
                    // here — only the cancellation path counts.
                    on_chunk("Because of Rayleigh scattering.");
                    Ok("Because of Rayleigh scattering.".to_string())
                })
            }
        }

        let committed: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let committed_clone = Arc::clone(&committed);
        let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(move |samples| {
            committed_clone.lock().unwrap().extend(samples);
        });

        let observer = MockObserver::new();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            super::run_loop_borrowed(
                backends,
                event_rx,
                Box::new(CountingResponder {
                    count: cc_clone,
                    cancel_drops: cd_clone,
                }),
                on_audio,
                None,
                false,
                None,
                observer.clone(),
            ),
        )
        .await
        .expect("did not deadlock")
        .expect("loop ok");

        // run_loop pushes one transcript per outer-loop iteration. Cancel-and-retry
        // is internal to one iteration. So we expect exactly one transcript.
        assert_eq!(result.len(), 1, "one commit cycle, one transcript");
        // Responder was called twice (first cancelled, second succeeded).
        assert_eq!(
            call_count.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "responder called twice"
        );
        // The cancelled (parking) future MUST have its destructor run —
        // exactly once — that's the cancellation guarantee. CancelGuard
        // is scoped to the parking branch only; the succeeding future
        // doesn't construct one, so this asserts the leak-vs-drop
        // contract precisely.
        assert_eq!(
            cancel_drops.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "cancelled LLM future was dropped exactly once (not leaked)"
        );
        // Audio committed (from second responder call).
        assert!(
            !committed.lock().unwrap().is_empty(),
            "audio committed on retry"
        );
        // The cancel-by-VAD path fires a Listen state_change with the
        // "child_resumed" hint. That's the observable contract for
        // GUIs/CLIs that want to surface the cancellation.
        let events = observer.events();
        assert!(
            events.iter().any(|e| matches!(
                e,
                MockEvent::StateChange {
                    state: super::VoiceState::Listen,
                    hint: Some(h),
                } if h == "child_resumed"
            )),
            "observer saw Listen state with child_resumed hint: {events:?}"
        );
    }

    /// Regression test for issue #103 — cancel-and-retry must stitch the
    /// transcript across the two STT sessions rather than discarding the
    /// pre-cancel half. Drives the same SpeechEnd → SpeechStart-cancel →
    /// SpeechEnd flow as the test above but with a scripted STT whose
    /// successive sessions return *different* partial transcripts. The
    /// final transcript surfaced to the observer (and to the LLM on
    /// retry) must be the concatenation of both halves.
    #[tokio::test]
    async fn cancel_and_retry_stitches_full_transcript() {
        use primer_core::speech::VadEvent;

        // Session #0 (opened during LISTEN) finalizes to "why does".
        // Session #1 (opened at the end of the latent-think iter 0)
        // finalizes to "the sky look blue". Session #2 (opened at the
        // end of iter 1) is unused; the queue is sized exactly so an
        // accidental third finalize would surface as an empty string.
        let backends = super::LoopBackends::single_locale(
            Arc::new(ScriptedStreamingStt::new(vec![
                "why does",
                "the sky look blue",
            ])),
            Arc::new(MockStreamingTts::new(64)),
            primer_core::speech::VoiceProfile::default(),
            primer_core::i18n::Locale::English,
        );

        let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
        // First SpeechStart → SpeechEnd: triggers LATENT_THINK (finalize
        // session #0 → "why does").
        event_tx.try_send(VadEvent::SpeechStart).unwrap();
        event_tx.try_send(VadEvent::SpeechEnd).unwrap();
        // SpeechStart mid-LATENT_THINK: cancels the LLM.
        event_tx.try_send(VadEvent::SpeechStart).unwrap();
        // Then SpeechEnd: retries LATENT_THINK (finalize session #1 →
        // "the sky look blue"). The accumulated transcript at this point
        // must be "why does the sky look blue".
        event_tx.try_send(VadEvent::SpeechEnd).unwrap();
        drop(event_tx);

        // Capture the transcript the responder sees on each call so the
        // assertions can pin "second call saw the full stitched text"
        // rather than guessing from the final result alone.
        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = Arc::clone(&captured);
        struct CapturingResponder {
            captured: Arc<Mutex<Vec<String>>>,
            count: Arc<std::sync::atomic::AtomicUsize>,
        }
        impl super::Responder for CapturingResponder {
            fn respond<'a>(
                &'a mut self,
                transcript: &'a str,
                mut on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = primer_core::error::Result<String>>
                        + Send
                        + 'a,
                >,
            > {
                self.captured.lock().unwrap().push(transcript.to_string());
                let n = self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async move {
                    if n == 0 {
                        // First call: park forever so the cancel arm wins.
                        std::future::pending::<()>().await;
                        unreachable!()
                    }
                    on_chunk("Because of Rayleigh scattering.");
                    Ok("Because of Rayleigh scattering.".to_string())
                })
            }
        }
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let responder = Box::new(CapturingResponder {
            captured: captured_clone,
            count: Arc::clone(&count),
        });

        let committed: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let committed_clone = Arc::clone(&committed);
        let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(move |samples| {
            committed_clone.lock().unwrap().extend(samples);
        });

        let observer = MockObserver::new();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            super::run_loop_borrowed(
                backends,
                event_rx,
                responder,
                on_audio,
                None,
                false,
                None,
                observer.clone(),
            ),
        )
        .await
        .expect("did not deadlock")
        .expect("loop ok");

        // Exactly one turn (cancel-and-retry stays inside a single outer
        // iteration) and the surfaced transcript is the full stitched
        // utterance — the headline assertion of issue #103.
        assert_eq!(
            result,
            vec!["why does the sky look blue".to_string()],
            "transcript stitched across both STT sessions"
        );

        // Responder was called twice; the second call saw the full
        // stitched text. Before the fix, the second call received only
        // "the sky look blue" and the test would fail here.
        let calls = captured.lock().unwrap().clone();
        assert_eq!(
            calls.len(),
            2,
            "responder called once before cancel, once on retry"
        );
        assert_eq!(
            calls[1], "why does the sky look blue",
            "retry LLM call received the full stitched transcript, not the tail"
        );

        // Observer saw the full stitched transcript via
        // `on_transcript_finalized` — the bubble-emission path that the
        // issue called out as user-visibly broken before the fix.
        let events = observer.events();
        assert!(
            events.iter().any(|e| matches!(
                e,
                MockEvent::Transcript(t) if t == "why does the sky look blue"
            )),
            "observer received the full stitched transcript: {events:?}"
        );

        // Sanity: audio reaches the speaker on the second attempt.
        assert!(
            !committed.lock().unwrap().is_empty(),
            "audio committed on retry"
        );
    }

    /// Defends the `!new_text.is_empty()` gate inside the LATENT_THINK
    /// accumulator. If a future refactor removed the gate, an iteration
    /// that finalizes to empty would append a phantom trailing space to
    /// the running transcript, and a subsequent non-empty finalize would
    /// surface as `"first  second"` (double space) rather than the
    /// correct `"first second"`. Drives three STT sessions —
    /// `["first", "", "second"]` — across two cancel-and-retry cycles
    /// and asserts the final stitched transcript has the right shape.
    #[tokio::test]
    async fn cancel_and_retry_skips_empty_finalize_no_phantom_space() {
        use primer_core::speech::VadEvent;

        let backends = super::LoopBackends::single_locale(
            Arc::new(ScriptedStreamingStt::new(vec!["first", "", "second"])),
            Arc::new(MockStreamingTts::new(64)),
            primer_core::speech::VoiceProfile::default(),
            primer_core::i18n::Locale::English,
        );

        let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
        // Iter 0: SpeechStart → SpeechEnd → finalize "first" → LLM call 0.
        event_tx.try_send(VadEvent::SpeechStart).unwrap();
        event_tx.try_send(VadEvent::SpeechEnd).unwrap();
        // Cancel call 0; iter 1: SpeechEnd → finalize "" → LLM call 1
        // (with the carried-over "first").
        event_tx.try_send(VadEvent::SpeechStart).unwrap();
        event_tx.try_send(VadEvent::SpeechEnd).unwrap();
        // Cancel call 1; iter 2: SpeechEnd → finalize "second" → LLM
        // call 2 (with the stitched "first second", a single space).
        event_tx.try_send(VadEvent::SpeechStart).unwrap();
        event_tx.try_send(VadEvent::SpeechEnd).unwrap();
        drop(event_tx);

        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = Arc::clone(&captured);
        struct CapturingResponder {
            captured: Arc<Mutex<Vec<String>>>,
            count: Arc<std::sync::atomic::AtomicUsize>,
        }
        impl super::Responder for CapturingResponder {
            fn respond<'a>(
                &'a mut self,
                transcript: &'a str,
                mut on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = primer_core::error::Result<String>>
                        + Send
                        + 'a,
                >,
            > {
                self.captured.lock().unwrap().push(transcript.to_string());
                let n = self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async move {
                    if n < 2 {
                        // First two calls park so the cancel arms win.
                        std::future::pending::<()>().await;
                        unreachable!()
                    }
                    on_chunk("ok.");
                    Ok("ok.".to_string())
                })
            }
        }
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let responder = Box::new(CapturingResponder {
            captured: captured_clone,
            count: Arc::clone(&count),
        });

        let committed: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let committed_clone = Arc::clone(&committed);
        let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(move |samples| {
            committed_clone.lock().unwrap().extend(samples);
        });

        let observer = MockObserver::new();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            super::run_loop_borrowed(
                backends,
                event_rx,
                responder,
                on_audio,
                None,
                false,
                None,
                observer.clone(),
            ),
        )
        .await
        .expect("did not deadlock")
        .expect("loop ok");

        // The final transcript is "first second" with a single separating
        // space. With the empty-gate removed, this would be "first  second"
        // (double space) because the empty middle finalize would append a
        // phantom trailing space onto "first" before "second" is appended.
        assert_eq!(
            result,
            vec!["first second".to_string()],
            "empty middle finalize must not introduce a phantom space"
        );

        // The responder saw three calls; the third saw the stitched text
        // with a single space. The second call saw the carried-over
        // "first" alone (no trailing space) — the strongest direct check
        // on the empty-text gate.
        let calls = captured.lock().unwrap().clone();
        assert_eq!(calls.len(), 3, "three LLM attempts (two cancelled, one ok)");
        assert_eq!(
            calls[1], "first",
            "empty iteration must not add a trailing space to the carried transcript"
        );
        assert_eq!(
            calls[2], "first second",
            "third call sees the stitched transcript with a single space"
        );
    }

    /// Test 3 — commit on first audio: synthesis fires before any
    /// resumed speech. Audio reaches the speaker callback; subsequent
    /// VAD events arriving after commit do not affect the in-flight
    /// SPEAK phase.
    #[tokio::test]
    async fn commit_on_first_chunk_proceeds_to_speak() {
        use primer_core::speech::VadEvent;

        let backends = super::LoopBackends::single_locale(
            Arc::new(MockStreamingStt::new("hi primer")),
            Arc::new(MockStreamingTts::new(64)),
            primer_core::speech::VoiceProfile::default(),
            primer_core::i18n::Locale::English,
        );

        let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
        event_tx.try_send(VadEvent::SpeechStart).unwrap();
        event_tx.try_send(VadEvent::SpeechEnd).unwrap();
        // Crucially: NO SpeechStart between SpeechEnd and the LLM future
        // resolving. Commit should proceed.
        drop(event_tx);

        struct PromptResponder;
        impl super::Responder for PromptResponder {
            fn respond<'a>(
                &'a mut self,
                _transcript: &'a str,
                mut on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = primer_core::error::Result<String>>
                        + Send
                        + 'a,
                >,
            > {
                Box::pin(async move {
                    on_chunk("Hello!");
                    Ok("Hello!".to_string())
                })
            }
        }

        let committed: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let committed_clone = Arc::clone(&committed);
        let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(move |samples| {
            committed_clone.lock().unwrap().extend(samples);
        });

        let observer = MockObserver::new();
        let result = super::run_loop_borrowed(
            backends,
            event_rx,
            Box::new(PromptResponder),
            on_audio,
            None,
            false,
            None,
            observer.clone(),
        )
        .await
        .expect("loop ok");

        assert_eq!(result, vec!["hi primer".to_string()]);
        assert!(!committed.lock().unwrap().is_empty(), "audio committed");
    }

    /// Test 5 — is_speaking gate observed during SPEAK: when the loop
    /// is given an `Arc<AtomicBool>` gate, the on_audio callback fires
    /// while the gate is set true (proving the audio thread would discard
    /// mic samples), and the gate clears to false before run_loop returns.
    #[tokio::test]
    async fn is_speaking_gate_flips_around_speak() {
        use std::sync::atomic::{AtomicBool, Ordering};

        use primer_core::speech::VadEvent;

        let backends = super::LoopBackends::single_locale(
            Arc::new(MockStreamingStt::new("hi")),
            Arc::new(MockStreamingTts::new(64)),
            primer_core::speech::VoiceProfile::default(),
            primer_core::i18n::Locale::English,
        );

        let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
        event_tx.try_send(VadEvent::SpeechStart).unwrap();
        event_tx.try_send(VadEvent::SpeechEnd).unwrap();
        drop(event_tx);

        struct PromptResponder;
        impl super::Responder for PromptResponder {
            fn respond<'a>(
                &'a mut self,
                _transcript: &'a str,
                _on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = primer_core::error::Result<String>>
                        + Send
                        + 'a,
                >,
            > {
                Box::pin(async move { Ok("Hi back.".to_string()) })
            }
        }

        let is_speaking = Arc::new(AtomicBool::new(false));
        let observed_true = Arc::new(AtomicBool::new(false));
        let observed_clone = Arc::clone(&observed_true);
        let speaking_for_cb = Arc::clone(&is_speaking);
        let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(move |_samples| {
            if speaking_for_cb.load(Ordering::SeqCst) {
                observed_clone.store(true, Ordering::SeqCst);
            }
        });

        // 2 s timeout: the mock on_audio doesn't poll for drain so the
        // gate clears as soon as the synth chunks have been "consumed"
        // (a few ms). Generous cap, kept high to surface deadlocks
        // rather than match expected runtime.
        let observer = MockObserver::new();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            super::run_loop_borrowed(
                backends,
                event_rx,
                Box::new(PromptResponder),
                on_audio,
                None,
                false,
                Some(Arc::clone(&is_speaking)),
                observer.clone(),
            ),
        )
        .await
        .expect("did not deadlock")
        .expect("loop ok");

        assert_eq!(result, vec!["hi".to_string()]);
        assert!(
            observed_true.load(Ordering::SeqCst),
            "is_speaking flag was true during SPEAK audio commit"
        );
        assert!(
            !is_speaking.load(Ordering::SeqCst),
            "is_speaking flag is cleared after SPEAK"
        );
    }

    /// Test 6 — LLM error fallback: when the responder returns Err,
    /// run_loop synthesises FALLBACK_LINE rather than propagating the
    /// error. Asserts the loop returns Ok(child transcript) and that
    /// the TTS received exactly the FALLBACK_LINE string (proving the
    /// child hears the apology).
    #[tokio::test]
    async fn llm_error_synthesises_fallback_line() {
        use std::sync::Mutex;

        use primer_core::speech::{
            AudioChunk, Named, StreamingTextToSpeech, SynthesisEvent, SynthesisSession, VadEvent,
            VoiceProfile,
        };

        // TTS that records every text fed to its session.
        struct CapturingTts {
            captured: Arc<Mutex<Vec<String>>>,
        }
        impl Named for CapturingTts {
            fn name(&self) -> &str {
                "capturing-tts"
            }
        }
        impl StreamingTextToSpeech for CapturingTts {
            fn sample_rate(&self) -> u32 {
                22_050
            }
            fn open_session(
                &self,
                _voice: &VoiceProfile,
            ) -> primer_core::error::Result<Box<dyn SynthesisSession>> {
                Ok(Box::new(CapturingSession {
                    captured: Arc::clone(&self.captured),
                }))
            }
        }
        struct CapturingSession {
            captured: Arc<Mutex<Vec<String>>>,
        }
        impl SynthesisSession for CapturingSession {
            fn push_text(
                &mut self,
                text: &str,
                on_event: &mut dyn FnMut(SynthesisEvent),
            ) -> primer_core::error::Result<()> {
                if text.is_empty() {
                    return Ok(());
                }
                self.captured.lock().unwrap().push(text.to_string());
                on_event(SynthesisEvent::Audio(AudioChunk {
                    samples: vec![0.5; 64],
                    sample_rate: 22_050,
                }));
                on_event(SynthesisEvent::PhraseEnd);
                Ok(())
            }

            fn finalize(
                self: Box<Self>,
                _on_event: &mut dyn FnMut(SynthesisEvent),
            ) -> primer_core::error::Result<()> {
                Ok(())
            }
        }

        let captured = Arc::new(Mutex::new(Vec::<String>::new()));
        let backends = super::LoopBackends::single_locale(
            Arc::new(MockStreamingStt::new("hello primer")),
            Arc::new(CapturingTts {
                captured: Arc::clone(&captured),
            }),
            VoiceProfile::default(),
            primer_core::i18n::Locale::English,
        );

        let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
        event_tx.try_send(VadEvent::SpeechStart).unwrap();
        event_tx.try_send(VadEvent::SpeechEnd).unwrap();
        drop(event_tx);

        struct ErrResponder;
        impl super::Responder for ErrResponder {
            fn respond<'a>(
                &'a mut self,
                _transcript: &'a str,
                _on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = primer_core::error::Result<String>>
                        + Send
                        + 'a,
                >,
            > {
                Box::pin(async move {
                    Err(primer_core::error::PrimerError::Inference(
                        "rate limit".into(),
                    ))
                })
            }
        }

        let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(|_samples| {});
        let observer = MockObserver::new();
        let result = super::run_loop_borrowed(
            backends,
            event_rx,
            Box::new(ErrResponder),
            on_audio,
            None,
            false,
            None,
            observer.clone(),
        )
        .await
        .expect("loop returns Ok despite responder error");

        assert_eq!(result, vec!["hello primer".to_string()]);
        let texts = captured.lock().unwrap();
        assert_eq!(texts.len(), 1, "TTS got exactly one push_text call");
        assert_eq!(
            texts[0],
            super::FALLBACK_LINE,
            "fallback line was synthesised after LLM error"
        );
        // Observer surfaces the inference error so a GUI banner can fire.
        let events = observer.events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, MockEvent::InferenceError(_))),
            "InferenceError observed: {events:?}"
        );
    }

    #[test]
    fn ensure_active_locale_coverage_ok_after_single_locale_constructor() {
        let backends = super::LoopBackends::single_locale(
            Arc::new(MockStreamingStt::new("")),
            Arc::new(MockStreamingTts::new(64)),
            primer_core::speech::VoiceProfile::default(),
            primer_core::i18n::Locale::German,
        );
        backends
            .ensure_active_locale_coverage()
            .expect("single_locale must satisfy the coverage invariant");
    }

    #[test]
    fn ensure_active_locale_coverage_errors_when_tts_missing() {
        // Hand-roll the maps to simulate a future caller that builds
        // them directly (e.g. from a voice-pack scan) and forgot to
        // include the active locale's voice. v1's `single_locale`
        // can't reach this state.
        let backends = super::LoopBackends {
            stt: Arc::new(MockStreamingStt::new("")),
            tts_by_locale: std::collections::HashMap::new(),
            voice_by_locale: std::collections::HashMap::new(),
            active_locale: primer_core::i18n::Locale::German,
        };
        let err = backends.ensure_active_locale_coverage().unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("'de'"),
            "must name the missing locale by pack id: {msg}"
        );
        assert!(
            msg.contains("--voice-onnx"),
            "must point the user at the corrective flags: {msg}"
        );
        assert!(
            msg.contains("piper-voices"),
            "must point the user at where to find a voice: {msg}"
        );
    }

    #[test]
    fn ensure_active_locale_coverage_errors_when_only_voice_missing() {
        let mut tts_by_locale: std::collections::HashMap<
            primer_core::i18n::Locale,
            Arc<dyn primer_core::speech::StreamingTextToSpeech>,
        > = std::collections::HashMap::new();
        tts_by_locale.insert(
            primer_core::i18n::Locale::English,
            Arc::new(MockStreamingTts::new(64)),
        );
        let backends = super::LoopBackends {
            stt: Arc::new(MockStreamingStt::new("")),
            tts_by_locale,
            voice_by_locale: std::collections::HashMap::new(),
            active_locale: primer_core::i18n::Locale::English,
        };
        let err = backends.ensure_active_locale_coverage().unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("voice profile"),
            "must distinguish voice-profile miss from TTS miss: {msg}"
        );
        assert!(msg.contains("'en'"), "must name the missing locale: {msg}");
    }
}

#[cfg(test)]
mod markdown_tests {
    use super::strip_markdown_for_tts;

    #[test]
    fn strips_paired_emphasis_and_strong() {
        assert_eq!(strip_markdown_for_tts("*why*"), "why");
        assert_eq!(strip_markdown_for_tts("**important**"), "important");
        assert_eq!(
            strip_markdown_for_tts("a *little* bit of **emphasis**"),
            "a little bit of emphasis"
        );
    }

    #[test]
    fn preserves_multiplication_between_digits() {
        assert_eq!(strip_markdown_for_tts("5*3=15"), "5 times 3=15");
        assert_eq!(strip_markdown_for_tts("2 * 3"), "2 * 3");
        assert_eq!(strip_markdown_for_tts("5*3*2"), "5 times 3 times 2");
    }

    #[test]
    fn preserves_exponent_double_star_between_digits() {
        assert_eq!(strip_markdown_for_tts("5**2"), "5 times 2");
    }

    #[test]
    fn leaves_unmatched_star_alone() {
        assert_eq!(strip_markdown_for_tts("a* footnote"), "a* footnote");
        assert_eq!(strip_markdown_for_tts("value *= 5"), "value *= 5");
    }

    #[test]
    fn strips_paired_backticks_only() {
        assert_eq!(strip_markdown_for_tts("`code`"), "code");
        assert_eq!(
            strip_markdown_for_tts("a single ` backtick"),
            "a single ` backtick"
        );
    }

    #[test]
    fn handles_mixed_markdown_and_math() {
        assert_eq!(
            strip_markdown_for_tts("the answer is **5*3=15** indeed"),
            "the answer is 5 times 3=15 indeed"
        );
    }

    #[test]
    fn no_op_on_plain_text() {
        assert_eq!(
            strip_markdown_for_tts("nothing to strip here"),
            "nothing to strip here"
        );
    }

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(strip_markdown_for_tts(""), "");
    }

    /// Triple-`*` runs (bold-italic markdown) are not currently
    /// recognised — the inner closer is rejected by the
    /// "not adjacent to another marker" guard and the outer pair
    /// finds no match. Pinned here so a future refactor doesn't
    /// silently break the current behaviour. If this assertion
    /// ever needs updating, re-derive the right output from first
    /// principles rather than tweaking the test.
    #[test]
    fn triple_star_passes_through_unchanged_for_now() {
        assert_eq!(strip_markdown_for_tts("***foo***"), "***foo***");
    }
}

#[cfg(test)]
mod quit_tests {
    use super::is_quit_phrase;
    use primer_core::i18n::Locale;

    #[test]
    fn detects_goodbye_case_insensitive() {
        assert!(is_quit_phrase("Goodbye!", &Locale::English));
        assert!(is_quit_phrase("GOODBYE", &Locale::English));
    }

    #[test]
    fn detects_bye_primer() {
        assert!(is_quit_phrase("bye primer", &Locale::English));
        assert!(is_quit_phrase("Bye Primer.", &Locale::English));
    }

    #[test]
    fn ignores_unrelated_transcripts() {
        assert!(!is_quit_phrase("why is the sky blue", &Locale::English));
        assert!(!is_quit_phrase("hello", &Locale::English));
        // "bye" alone is NOT a quit phrase — only "bye primer".
        assert!(!is_quit_phrase("bye", &Locale::English));
    }

    /// Embedded-phrase guard: a quit phrase embedded inside a longer
    /// utterance must NOT terminate the session. The pre-fix substring
    /// `contains` would have ended the session on either of these —
    /// exactly the opposite of the child's intent. Word-boundary
    /// matching alone wouldn't fix this (end-of-string is itself a
    /// word boundary); equality-after-normalisation does.
    #[test]
    fn embedded_quit_phrase_does_not_end_session() {
        assert!(!is_quit_phrase(
            "I don't want to stop primer",
            &Locale::English
        ));
        assert!(!is_quit_phrase("alright goodbye then", &Locale::English));
        // ... but the phrase as a complete utterance ends the session.
        assert!(is_quit_phrase("stop primer", &Locale::English));
        // Punctuation around the phrase is fine (Whisper often appends).
        assert!(is_quit_phrase("Stop primer!", &Locale::English));
        // Collapsed internal whitespace is fine too.
        assert!(is_quit_phrase("  bye   primer  ", &Locale::English));
    }

    /// German locale ships its own quit phrases. A German-speaking child
    /// who says "tschüss" or "auf wiedersehen" must be able to end the
    /// session by voice — and an English "goodbye" should NOT match in
    /// a German session.
    #[test]
    fn german_locale_uses_german_quit_phrases() {
        assert!(is_quit_phrase("tschüss", &Locale::German));
        assert!(is_quit_phrase("Tschüss!", &Locale::German));
        assert!(is_quit_phrase("auf wiedersehen", &Locale::German));
        assert!(is_quit_phrase("Auf Wiedersehen.", &Locale::German));
        // English-only phrases don't end a German session.
        assert!(!is_quit_phrase("goodbye", &Locale::German));
        // Primer-direct variants are universal (English loanwords).
        assert!(is_quit_phrase("bye primer", &Locale::German));
    }

    /// English locale must NOT match German-only phrases.
    #[test]
    fn english_locale_rejects_german_only_phrases() {
        assert!(!is_quit_phrase("tschüss", &Locale::English));
        assert!(!is_quit_phrase("auf wiedersehen", &Locale::English));
    }
}
