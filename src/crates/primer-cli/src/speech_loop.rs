//! Voice round-trip REPL — the `--speech` mode of `primer-cli`.
//!
//! State machine: `LISTEN → LATENT_THINK → SPEAK → LISTEN`, with the
//! mic open through LISTEN and LATENT_THINK so the Primer never barges
//! in on a child mid-thought. Closes the mic on the commit boundary
//! (first audio chunk reaches the speaker) so the child never speaks
//! over the Primer.
//!
//! See `docs/superpowers/specs/2026-05-02-voice-roundtrip-poc-design.md`
//! for the full design.

use std::path::Path;

use primer_core::error::Result;

/// Phrases that, if heard in the child's transcript, end the session.
/// Case-insensitive substring match. Three flavours so the child can
/// quit whether they say the formal "goodbye", a Primer-direct
/// "bye primer", or the very direct "stop primer".
const QUIT_PHRASES: &[&str] = &["goodbye", "bye primer", "stop primer"];

/// Spoken when the LLM call fails (rate limit, network, etc.). Goes
/// through Piper just like any normal Primer turn — the child hears
/// the apology, then we loop back to LISTEN.
const FALLBACK_LINE: &str = "Sorry, I had trouble with that. Could you ask again?";

/// Returns true if `transcript` contains any quit phrase (case-insensitive).
fn is_quit_phrase(transcript: &str) -> bool {
    let lower = transcript.to_lowercase();
    QUIT_PHRASES.iter().any(|p| lower.contains(p))
}

/// Strip markdown emphasis markers so Piper's espeak phonemizer doesn't
/// pronounce them ("*why*" → "asterisks why asterisks"). Paired
/// `*emphasis*` and `**strong**` markers are removed; paired
/// `` `code` `` markers are removed. Bare unmatched `*` or `` ` `` are
/// left in place. A `*` (or run of `*`) sandwiched between digits is
/// treated as multiplication and replaced with " times " so `5*3=15`
/// reads as "5 times 3=15" instead of "53=15". Underscore-emphasis is
/// rare and ambiguous (shows up in identifiers too) — left alone.
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

/// Configuration passed into `run` from `main`.
pub struct SpeechLoopConfig<'a> {
    pub whisper_model: &'a Path,
    pub voice_onnx: &'a Path,
    pub voice_config: &'a Path,
    pub voice_id: &'a str,
    pub mic_silence_ms: u32,
    pub verbose: bool,
}

use std::sync::Arc;

use primer_core::speech::{StreamingSpeechToText, StreamingTextToSpeech};
// Production audio-thread paths call methods on `SileroVad` via the
// `VoiceActivityDetector` trait. Imported only when the speech feature
// is enabled — the trait is otherwise unused in this module.
#[cfg(feature = "speech")]
use primer_core::speech::VoiceActivityDetector;

/// Bound on the VAD event channel. At ~32 events/s (silero on 512-sample
/// chunks at 16 kHz), 256 holds ~8 seconds of accumulated events. The
/// audio thread sends via `blocking_send`, so saturation back-pressures
/// the audio thread (it stops draining the mic ringbuf) rather than
/// dropping events — drops would break SpeechStart/SpeechEnd pairing.
/// The cap is sized large enough that this never triggers in steady
/// state; if `run_loop` falls 8 s behind, the audio thread will block
/// briefly until the consumer catches up.
const VAD_EVENT_CHANNEL_CAPACITY: usize = 256;

/// Shared receiver for the audio thread → run_loop transcript channel.
/// `Arc<Mutex<...>>` only because the `StreamingSpeechToText` trait hands
/// out sessions through `&self`; there is exactly one consumer.
#[cfg(feature = "speech")]
type TranscriptRx = Arc<std::sync::Mutex<std::sync::mpsc::Receiver<String>>>;

/// Trait-injected backends consumed by `run_loop`. Production wires real
/// whisper / piper instances; tests wire mocks. The VAD lives on the
/// audio capture thread (production) or is stubbed out via direct VAD
/// events on the channel (tests), so it's not part of this struct.
pub struct LoopBackends {
    pub stt: Arc<dyn StreamingSpeechToText>,
    pub tts: Arc<dyn StreamingTextToSpeech>,
    /// Voice profile passed to `tts.open_session(...)`. Production wires
    /// the model_id from `--voice` (e.g. `en_GB-alba-medium`); tests can
    /// use `VoiceProfile::default()`. Piper rejects mismatches, so this
    /// must align with the loaded voice ONNX file stem.
    pub voice: primer_core::speech::VoiceProfile,
}

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

/// Drive the state machine until a quit phrase or exhausted VAD events.
/// `events` is the source of `VadEvent`s the loop reads (production
/// wires the audio capture task; tests wire a `tokio::sync::mpsc`
/// pre-filled with a script).
///
/// The `'r` lifetime on the boxed responder lets `DialogueResponder`
/// (Task 21) borrow `&mut DialogueManager` rather than own it.
///
/// `verbose` gates `[vad]`/`[stt]` debug lines on stderr.
/// `[child]` and `[primer]` lines are always printed on stdout.
///
/// `is_speaking` is the gate the audio thread checks to decide whether to
/// process or discard mic samples. `run_loop` flips it true at the start
/// of SPEAK and back to false after the synthesised audio has had time
/// to drain to the speaker. Tests pass `None` (mocks have no audio thread).
pub async fn run_loop<'r>(
    backends: LoopBackends,
    mut events: tokio::sync::mpsc::Receiver<primer_core::speech::VadEvent>,
    mut responder: Box<dyn Responder + 'r>,
    mut on_committed_audio: Box<dyn FnMut(Vec<f32>) + Send>,
    verbose: bool,
    is_speaking: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<Vec<String>> {
    use primer_core::speech::VadEvent;

    let mut transcripts: Vec<String> = Vec::new();

    'outer: loop {
        // ── LISTEN ────────────────────────────────────────────────────
        let mut stt_session = backends.stt.open_session()?;
        let mut in_speech = false;
        loop {
            let Some(event) = events.recv().await else {
                break 'outer;
            };
            match event {
                VadEvent::SpeechStart => in_speech = true,
                VadEvent::SpeechEnd if in_speech => break,
                _ => {}
            }
        }

        // ── LATENT_THINK ──────────────────────────────────────────────
        // Loop here so a SpeechStart-cancel can resume listening with
        // the same whisper session and re-attempt the LLM call once the
        // child finishes their continuation.
        let mut transcript_so_far: String;
        let mut accumulated = String::new();
        loop {
            // Peek (finalize-and-reopen since whisper-cpp-plus has no
            // partial-extract API exposed here): we accept the slight
            // mock-friendliness — production whisper supports peeking via
            // process_step but the trait surface is finalize-only today.
            let segments = stt_session.finalize()?;
            transcript_so_far = segments
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join("")
                .trim()
                .to_string();
            stt_session = backends.stt.open_session()?;

            if transcript_so_far.is_empty() {
                if verbose {
                    eprintln!("[stt] empty transcript, looping");
                }
                break;
            }

            // Drive the LLM. `respond` returns the full accumulated text
            // as Ok(String); the on_chunk callback is for live streaming
            // (e.g. terminal echo). For unit tests we ignore chunks and
            // rely on the final Result. The borrow on `transcript_so_far`
            // ends when this select! resolves — earlier code cloned the
            // string redundantly because `DialogueResponder` already
            // copies the transcript into the future internally.
            let llm_fut = responder.respond(&transcript_so_far, Box::new(|_chunk: &str| {}));
            tokio::pin!(llm_fut);

            // Wait for either: (a) llm done, (b) VAD SpeechStart (cancel).
            // If the VAD event channel is both closed AND drained, we can
            // complete the LLM unconditionally — no more events will arrive.
            // Note: is_closed() alone returns true even with buffered messages
            // (when all senders are dropped), so we must also check is_empty().
            let cancelled = if events.is_closed() && events.is_empty() {
                // No more VAD events possible: complete the LLM unconditionally.
                accumulated = match llm_fut.await {
                    Ok(text) => text,
                    Err(e) => {
                        tracing::error!("LLM error: {e}");
                        FALLBACK_LINE.to_string()
                    }
                };
                false
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
                    res = &mut llm_fut => {
                        accumulated = match res {
                            Ok(text) => text,
                            Err(e) => {
                                tracing::error!("LLM error: {e}");
                                FALLBACK_LINE.to_string()
                            }
                        };
                        false
                    }
                    event = events.recv() => {
                        match event {
                            Some(VadEvent::SpeechStart) => {
                                // Cancel: drop the future, loop back, keep listening.
                                if verbose {
                                    eprintln!("[vad] aborted (resumed speech)");
                                }
                                true
                            }
                            Some(VadEvent::SpeechEnd) | Some(VadEvent::None) => {
                                // Spurious — shouldn't happen during LATENT_THINK
                                // since we entered on SpeechEnd. Treat as
                                // continue-waiting by completing the LLM.
                                accumulated = match llm_fut.await {
                                    Ok(text) => text,
                                    Err(e) => {
                                        tracing::error!("LLM error: {e}");
                                        FALLBACK_LINE.to_string()
                                    }
                                };
                                false
                            }
                            None => {
                                // Channel just closed mid-select: complete the LLM.
                                accumulated = match llm_fut.await {
                                    Ok(text) => text,
                                    Err(e) => {
                                        tracing::error!("LLM error: {e}");
                                        FALLBACK_LINE.to_string()
                                    }
                                };
                                false
                            }
                        }
                    }
                }
            };

            if cancelled {
                // Wait for the next SpeechEnd to retry the LLM call.
                loop {
                    let Some(event) = events.recv().await else {
                        return Ok(transcripts);
                    };
                    if event == VadEvent::SpeechEnd {
                        break;
                    }
                }
                continue;
            }

            break;
        }

        // ── Quit check + commit transcript ────────────────────────────
        if transcript_so_far.is_empty() {
            continue;
        }
        println!("[child] {}", transcript_so_far);
        if is_quit_phrase(&transcript_so_far) {
            transcripts.push(transcript_so_far);
            break 'outer;
        }
        transcripts.push(transcript_so_far);

        // ── SPEAK ─────────────────────────────────────────────────────
        if !accumulated.is_empty() {
            println!("[primer] {}", accumulated);
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
            let mut session = backends.tts.open_session(&backends.voice)?;
            let tts_rate = backends.tts.sample_rate();
            // ~200 ms of silence inserted between AudioChunks (each
            // chunk is one phrase). Gives the listener a perceptible
            // pause at sentence boundaries without adding much to
            // total response time.
            const INTER_PHRASE_SILENCE_MS: u32 = 200;
            let inter_phrase_silence_samples = (tts_rate * INTER_PHRASE_SILENCE_MS / 1000) as usize;
            for chunk in session.push_text(&tts_text)? {
                on_committed_audio(chunk.samples);
                on_committed_audio(vec![0.0_f32; inter_phrase_silence_samples]);
            }
            for chunk in session.finalize()? {
                on_committed_audio(chunk.samples);
                on_committed_audio(vec![0.0_f32; inter_phrase_silence_samples]);
            }
            // Flush sentinel: empty Vec signals on_audio to drain any
            // resampler-leftover tail AND (in production) wait for cpal
            // to actually empty the ringbuf before returning. Mock
            // callbacks no-op on empty input. Replaces the old
            // `samples / tts_rate + 0.4s` heuristic sleep that lived
            // here — the exact "speaker has played everything" signal
            // now lives in the on_audio callback (where the producer
            // is in scope to query `occupied_len`).
            on_committed_audio(Vec::new());
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
    }

    Ok(transcripts)
}

/// Adapter that lets `&mut DialogueManager` satisfy the [`Responder`]
/// trait. The lifetime `'a` is the dialogue manager's borrowed-backends
/// lifetime; `'b` is how long this adapter lives. `'a: 'b` keeps the
/// nested-borrow well-formed.
struct DialogueResponder<'a, 'b> {
    dialogue: &'b mut primer_pedagogy::DialogueManager<'a>,
}

impl<'a, 'b> Responder for DialogueResponder<'a, 'b>
where
    'a: 'b,
{
    fn respond<'r>(
        &'r mut self,
        transcript: &'r str,
        mut on_chunk: Box<dyn FnMut(&str) + Send + 'r>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'r>> {
        // We own a `String` copy of the transcript inside the future so
        // the borrow on `transcript` doesn't have to outlive the await.
        let transcript = transcript.to_string();
        Box::pin(async move {
            self.dialogue
                .respond_to_streaming(&transcript, |chunk| on_chunk(chunk))
                .await
        })
    }
}

/// `StreamingSpeechToText` adapter: hands out sessions whose `finalize`
/// pulls a transcript from a `std::sync::mpsc` channel populated by the
/// audio capture thread. This decouples the audio thread (which owns
/// the real `WhisperStream` and pushes mic samples into it) from
/// `run_loop` (which only reads `VadEvent`s and calls `finalize` on the
/// session it opened).
///
/// **Ordering contract:** the audio thread MUST send the transcript on
/// `tx` BEFORE emitting `VadEvent::SpeechEnd` on the event channel. That
/// way, by the time `run_loop` calls `session.finalize()` (after seeing
/// `SpeechEnd`), the transcript is already buffered in the channel.
#[cfg(feature = "speech")]
struct ChannelStt {
    rx: TranscriptRx,
}

#[cfg(feature = "speech")]
impl primer_core::speech::Named for ChannelStt {
    fn name(&self) -> &str {
        "channel-stt"
    }
}

#[cfg(feature = "speech")]
impl StreamingSpeechToText for ChannelStt {
    fn sample_rate(&self) -> u32 {
        16_000
    }
    fn open_session(&self) -> Result<Box<dyn primer_core::speech::TranscriptionSession>> {
        Ok(Box::new(ChannelSttSession {
            rx: Arc::clone(&self.rx),
        }))
    }
}

#[cfg(feature = "speech")]
struct ChannelSttSession {
    rx: TranscriptRx,
}

#[cfg(feature = "speech")]
impl primer_core::speech::TranscriptionSession for ChannelSttSession {
    fn push_audio(
        &mut self,
        _samples: &[f32],
    ) -> Result<Vec<primer_core::speech::TranscriptSegment>> {
        // run_loop never calls push_audio on this session (samples are
        // pushed directly into whisper inside the audio thread). If
        // someone wires it up differently this is a silent no-op.
        Ok(vec![])
    }
    fn finalize(self: Box<Self>) -> Result<Vec<primer_core::speech::TranscriptSegment>> {
        // The audio thread sends BEFORE emitting SpeechEnd, so by the
        // time run_loop calls us the transcript is normally already
        // buffered. Spin try_recv briefly to tolerate scheduling
        // jitter — but fail loudly rather than blocking, so a contract
        // violation surfaces as a quick "empty utterance" instead of a
        // half-second stall in the async runtime.
        let rx = self.rx.lock().map_err(|_| {
            primer_core::error::PrimerError::Speech(
                "ChannelSttSession: transcript receiver mutex poisoned".into(),
            )
        })?;
        const TRANSCRIPT_RECV_RETRIES: u32 = 5;
        const TRANSCRIPT_RECV_BACKOFF: std::time::Duration = std::time::Duration::from_millis(2);
        for _ in 0..TRANSCRIPT_RECV_RETRIES {
            match rx.try_recv() {
                Ok(text) => {
                    return Ok(vec![primer_core::speech::TranscriptSegment {
                        text,
                        start_ms: 0,
                        end_ms: 0,
                    }]);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    std::thread::sleep(TRANSCRIPT_RECV_BACKOFF);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err(primer_core::error::PrimerError::Speech(
                        "audio thread disconnected before producing transcript".into(),
                    ));
                }
            }
        }
        // Contract violation (no transcript queued before SpeechEnd) —
        // empty utterance. run_loop short-circuits on empty.
        Ok(vec![])
    }
}

/// Entry point: run the voice REPL until Ctrl+C or a quit phrase is heard.
#[cfg(feature = "speech")]
pub async fn run<'a>(
    cfg: SpeechLoopConfig<'_>,
    dialogue: &mut primer_pedagogy::DialogueManager<'a>,
) -> Result<()> {
    use primer_core::speech::VadEvent;
    use primer_speech::{
        MicCapture, PiperTts, Resampler, SileroVad, SileroVadParams, SpeakerSink, WhisperStt,
    };

    // ── Build VAD (real, lives in audio thread) ─────────────────────
    let vad_params = SileroVadParams {
        min_silence_ms: cfg.mic_silence_ms,
        ..SileroVadParams::default()
    };
    let mut audio_vad = SileroVad::new(vad_params)?;

    // ── Build STT (real whisper.cpp; the streaming session lives in
    //    the audio thread; run_loop sees a ChannelStt wrapper) ─────────
    let whisper = std::sync::Arc::new(WhisperStt::new(cfg.whisper_model)?);

    // ── Build TTS (real piper) ─────────────────────────────────────
    // PIPER_ESPEAKNG_DATA_DIRECTORY is probed and set by `main()` before
    // the tokio runtime starts (see `probe_espeak_ng_data`). Doing it here
    // would be unsound — the runtime worker threads already exist.
    let tts: std::sync::Arc<dyn StreamingTextToSpeech> =
        std::sync::Arc::new(PiperTts::new(cfg.voice_onnx, cfg.voice_config)?);
    let tts_sample_rate = tts.sample_rate();

    // ── Open mic ───────────────────────────────────────────────────
    let (mic, mic_cons) = MicCapture::start()?;
    let mic_rate = mic.sample_rate;
    if cfg.verbose {
        eprintln!(
            "[speech] mic opened: {}Hz, {} channels",
            mic_rate, mic.channels
        );
    }

    // ── Open speaker ───────────────────────────────────────────────
    let (spk, mut spk_prod) = SpeakerSink::start()?;
    let spk_rate = spk.sample_rate;
    // Shared "stream errored" flag — set by cpal's err_callback if the
    // output device disappears mid-session. The on_audio retry loop
    // checks this so it bails instead of spinning forever on a dead
    // stream where push_slice will return 0 indefinitely.
    let spk_errored = spk.errored_flag();
    if cfg.verbose {
        eprintln!(
            "[speech] speaker opened: {}Hz, {} channels",
            spk_rate, spk.channels
        );
    }

    // ── Input resampler: mic_rate → 16 kHz for VAD/whisper ─────────
    // We want each Resampler::process call to yield exactly
    // CHUNK_SAMPLES (512) at 16 kHz. Pick the input chunk size as
    // 512 * mic_rate / 16_000 (e.g. 1536 at 48 kHz).
    let vad_rate = audio_vad.sample_rate();
    let vad_chunk = audio_vad.chunk_samples();
    let in_chunk_samples: usize = (vad_chunk as u64 * mic_rate as u64 / vad_rate as u64) as usize;
    let mut input_resampler: Option<Resampler> = if mic_rate != vad_rate {
        Some(Resampler::new(mic_rate, vad_rate, in_chunk_samples)?)
    } else {
        None
    };

    // ── Output resampler factory: piper_rate → spk_rate, lazy
    //    constructed inside the on_audio callback so chunk sizing is
    //    decided once (per chunk size). For simplicity we resample with a
    //    fixed input chunk size that we pad/truncate the TTS chunk to. ─
    let need_output_resample = tts_sample_rate != spk_rate;
    // Output resampler operates on a fixed chunk size; build per-callback
    // padding/buffering to feed it. We keep it inside a Mutex<Option<…>>
    // so the on_audio FnMut can mutate it across calls.
    let output_chunk_in: usize = 1024;
    let output_resampler = std::sync::Arc::new(std::sync::Mutex::new(if need_output_resample {
        Some(Resampler::new(tts_sample_rate, spk_rate, output_chunk_in)?)
    } else {
        None
    }));

    // ── Channels ───────────────────────────────────────────────────
    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<VadEvent>(VAD_EVENT_CHANNEL_CAPACITY);
    let (transcript_tx, transcript_rx) = std::sync::mpsc::channel::<String>();
    // Cancellation flag — set when run_loop exits to tell the audio
    // thread to stop.
    let stop_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Speaking gate — set by run_loop's SPEAK phase, checked by the
    // audio thread. While true, the audio thread drains the mic ringbuf
    // but discards the samples (no VAD, no whisper push). This is the
    // anti-feedback guard: without it, the Primer's own TTS leaks
    // through the speaker → mic → whisper loop and gets transcribed
    // as the next "child" utterance.
    let is_speaking = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    // ── Spawn audio capture thread ─────────────────────────────────
    // Owns: mic_cons, input_resampler, audio_vad, whisper (Arc),
    //       event_tx, transcript_tx, stop_flag, is_speaking.
    // Drives: the LISTEN-side state machine (raw mic → resample →
    //         silero → whisper push), at audio rate.
    let whisper_for_thread = std::sync::Arc::clone(&whisper);
    let stop_flag_thread = std::sync::Arc::clone(&stop_flag);
    let is_speaking_thread = std::sync::Arc::clone(&is_speaking);
    let audio_thread = std::thread::Builder::new()
        .name("primer-speech-audio".into())
        .spawn(move || {
            run_audio_thread(
                mic_cons,
                input_resampler.take(),
                in_chunk_samples,
                vad_chunk,
                &mut audio_vad,
                whisper_for_thread,
                event_tx,
                transcript_tx,
                stop_flag_thread,
                is_speaking_thread,
            )
        })
        .map_err(|e| primer_core::error::PrimerError::Speech(format!("spawn audio thread: {e}")))?;

    // ── Build LoopBackends with the channel STT wrapper ────────────
    let backends = LoopBackends {
        stt: Arc::new(ChannelStt {
            rx: Arc::new(std::sync::Mutex::new(transcript_rx)),
        }),
        tts: std::sync::Arc::clone(&tts),
        voice: primer_core::speech::VoiceProfile {
            model_id: cfg.voice_id.to_string(),
            // Slow Piper slightly: length_scale = 1.0 / rate, so
            // rate < 1.0 stretches each phoneme. 0.9 is barely
            // perceptible but easier for younger children to follow.
            rate: 0.9,
            ..primer_core::speech::VoiceProfile::default()
        },
    };

    // ── on_audio callback: push synth chunks to the speaker ─────────
    // Uses a leftover buffer carried across calls so partial tails of
    // each phrase are PREPENDED to the next call's samples instead of
    // zero-padded. An empty input Vec is the FLUSH sentinel: zero-pad
    // whatever's still in the leftover and process it as the very last
    // block (this is end-of-turn, so the zero-pad artifact at the tail
    // is silence anyway).
    let output_resampler_for_cb = std::sync::Arc::clone(&output_resampler);
    let mut output_leftover: Vec<f32> = Vec::with_capacity(output_chunk_in);
    let spk_errored_for_cb = std::sync::Arc::clone(&spk_errored);
    let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(move |samples| {
        let is_flush = samples.is_empty();
        let mut samples = samples;
        if need_output_resample {
            let mut guard = output_resampler_for_cb.lock().unwrap();
            if let Some(resampler) = guard.as_mut() {
                // Prepend leftover from prior call, then process as many
                // full output_chunk_in blocks as possible.
                let mut combined: Vec<f32> =
                    Vec::with_capacity(output_leftover.len() + samples.len());
                combined.append(&mut output_leftover);
                combined.append(&mut samples);
                let usable = (combined.len() / output_chunk_in) * output_chunk_in;
                let mut out_buf: Vec<f32> = Vec::with_capacity(combined.len() * 2);
                let mut i = 0;
                while i + output_chunk_in <= usable {
                    let block = &combined[i..i + output_chunk_in];
                    match resampler.process(block) {
                        Ok(o) => out_buf.extend(o),
                        Err(e) => {
                            tracing::warn!("output resampler error: {e}");
                            return;
                        }
                    }
                    i += output_chunk_in;
                }
                let tail: Vec<f32> = combined[usable..].to_vec();
                if is_flush {
                    // End of turn: zero-pad the leftover to chunk_in
                    // and process. The zero-pad lands on trailing
                    // silence; subsequent silence chunks push out any
                    // remaining FFT-buffered output. process_partial()
                    // alone proved insufficient on FftFixedIn — the
                    // last syllable still got swallowed. Feeding 4
                    // silence chunks (≈186 ms of input silence) reliably
                    // drains the internal latency. Trailing silence
                    // is harmless; cpal just plays it.
                    if !tail.is_empty() {
                        let mut padded = tail;
                        padded.resize(output_chunk_in, 0.0);
                        if let Ok(o) = resampler.process(&padded) {
                            out_buf.extend(o);
                        }
                    }
                    let silence = vec![0.0_f32; output_chunk_in];
                    for _ in 0..4 {
                        match resampler.process(&silence) {
                            Ok(o) => out_buf.extend(o),
                            Err(_) => break,
                        }
                    }
                    output_leftover = Vec::new();
                } else {
                    output_leftover = tail;
                }
                samples = out_buf;
            }
        }
        // Push to speaker ringbuf, blocking until it accepts every
        // sample. The previous 1-second drop-on-timeout was the very
        // bug we hunted for the swallowed-end-of-response: when the
        // producer outpaces cpal's drain (synthesis is faster than
        // playback), we briefly fill the buffer and need to wait, not
        // give up. The retry bails when `spk_errored` flips true (cpal
        // output stream errored, device removed, etc.) instead of
        // spinning forever on a dead stream.
        if !samples.is_empty() {
            primer_speech::push_all_with_bail(
                &mut spk_prod,
                &samples,
                &spk_errored_for_cb,
                std::time::Duration::from_millis(5),
            );
        }
        // End-of-turn (flush sentinel): wait for cpal to actually drain
        // the ringbuf before returning, so the mic gate in run_loop
        // stays closed until the speaker is silent. Replaces the old
        // heuristic `samples / tts_rate + 0.4s` sleep — exact instead
        // of "fixed margin that's too short on slow hardware and too
        // long on fast hardware". Bails on errored stream; capped at 5 s
        // sanity timeout so a stuck buffer can't hang the REPL.
        if is_flush {
            let _ = primer_speech::wait_for_drain(
                &spk_prod,
                &spk_errored_for_cb,
                std::time::Duration::from_millis(10),
                3,
                std::time::Duration::from_millis(80),
                std::time::Duration::from_secs(5),
            );
        }
    });

    // ── Wire DialogueManager via the Responder adapter ─────────────
    let responder: Box<dyn Responder + '_> = Box::new(DialogueResponder { dialogue });

    // ── Drive the loop ─────────────────────────────────────────────
    let result = run_loop(
        backends,
        event_rx,
        responder,
        on_audio,
        cfg.verbose,
        Some(std::sync::Arc::clone(&is_speaking)),
    )
    .await;

    // ── Tell the audio thread to stop and wait for it ──────────────
    stop_flag.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Err(panic) = audio_thread.join() {
        tracing::warn!("audio thread panicked: {panic:?}");
    }

    // Keep cpal streams alive until here (drop now via end of scope).
    drop(mic);
    drop(spk);

    let transcripts = result?;
    if cfg.verbose {
        eprintln!("[speech] session ended after {} turn(s)", transcripts.len());
    }
    Ok(())
}

/// Stub implementation when the `speech` feature is disabled. Returns
/// an error so the binary fails fast if `--speech` is somehow set
/// without the feature.
#[cfg(not(feature = "speech"))]
pub async fn run<'a>(
    _cfg: SpeechLoopConfig<'_>,
    _dialogue: &mut primer_pedagogy::DialogueManager<'a>,
) -> Result<()> {
    Err(primer_core::error::PrimerError::Speech(
        "primer-cli was built without the `speech` feature".into(),
    ))
}

/// Body of the audio capture thread.
///
/// Pulls mic samples from `mic_cons`, resamples to the VAD rate when
/// needed, runs the VAD on fixed-size chunks, and drives the per-
/// utterance whisper session: open on `SpeechStart`, push every chunk
/// while in speech, finalize on `SpeechEnd` and SEND the transcript on
/// `transcript_tx` BEFORE forwarding the `SpeechEnd` event on
/// `event_tx`. That ordering guarantees `ChannelSttSession::finalize`
/// (called by `run_loop` after seeing the event) finds a transcript.
///
/// Polling cadence: a 5 ms sleep between empty mic-buffer reads. cpal's
/// callback fires every few ms, so this stays well within real-time
/// tolerances. A `tokio::Notify` would be marginally better but a
/// 5 ms idle sleep is invisible to humans.
#[cfg(feature = "speech")]
#[allow(clippy::too_many_arguments)]
fn run_audio_thread(
    mut mic_cons: ringbuf::HeapCons<f32>,
    mut input_resampler: Option<primer_speech::Resampler>,
    in_chunk_samples: usize,
    vad_chunk_samples: usize,
    vad: &mut primer_speech::SileroVad,
    whisper: std::sync::Arc<primer_speech::WhisperStt>,
    event_tx: tokio::sync::mpsc::Sender<primer_core::speech::VadEvent>,
    transcript_tx: std::sync::mpsc::Sender<String>,
    stop_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    is_speaking: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    use primer_core::speech::{StreamingSpeechToText, TranscriptionSession, VadEvent};
    use ringbuf::traits::Consumer as _;

    let mut raw_buf: Vec<f32> = Vec::with_capacity(in_chunk_samples * 2);
    let mut vad_in_buf: Vec<f32> = Vec::with_capacity(vad_chunk_samples * 2);
    // Reusable scratch buffer for the per-iteration VAD chunk; avoids
    // allocating ~32 Vecs/sec for the hot path.
    let mut vad_chunk: Vec<f32> = Vec::with_capacity(vad_chunk_samples);
    // The active whisper session, if we're in speech.
    let mut active_session: Option<Box<dyn TranscriptionSession>> = None;

    loop {
        if stop_flag.load(std::sync::atomic::Ordering::SeqCst) {
            // Drain whisper if mid-utterance.
            if let Some(s) = active_session.take() {
                let _ = s.finalize();
            }
            return Ok(());
        }

        // Anti-feedback gate: while run_loop is in SPEAK, drop everything
        // the mic captures (the Primer's own voice would otherwise be
        // transcribed). Also discard any partial whisper session; we'll
        // open a fresh one when the gate reopens.
        if is_speaking.load(std::sync::atomic::Ordering::SeqCst) {
            while mic_cons.try_pop().is_some() {}
            if let Some(s) = active_session.take() {
                let _ = s.finalize();
            }
            raw_buf.clear();
            vad_in_buf.clear();
            // Reset VAD's internal speech-state debouncer so we don't
            // emit a stale SpeechEnd as soon as the gate reopens.
            vad.reset();
            std::thread::sleep(std::time::Duration::from_millis(20));
            continue;
        }

        // Drain available samples from the mic ringbuf.
        let mut produced_any = false;
        while let Some(s) = mic_cons.try_pop() {
            raw_buf.push(s);
            produced_any = true;
            if raw_buf.len() >= 8192 {
                // Hard cap: don't let the buffer balloon if we're
                // resampling unevenly.
                break;
            }
        }
        if !produced_any {
            std::thread::sleep(std::time::Duration::from_millis(5));
            continue;
        }

        // Convert raw_buf into VAD-rate chunks.
        if let Some(resampler) = input_resampler.as_mut() {
            // Process exactly `in_chunk_samples`-sized blocks.
            let usable = (raw_buf.len() / in_chunk_samples) * in_chunk_samples;
            let mut consumed = 0;
            while consumed + in_chunk_samples <= usable {
                let block = &raw_buf[consumed..consumed + in_chunk_samples];
                match resampler.process(block) {
                    Ok(out) => vad_in_buf.extend(out),
                    Err(e) => {
                        tracing::warn!("input resampler error: {e}");
                        // Skip the block.
                    }
                }
                consumed += in_chunk_samples;
            }
            // Drop consumed samples from raw_buf.
            raw_buf.drain(..consumed);
        } else {
            // Native rate already matches VAD rate.
            vad_in_buf.append(&mut raw_buf);
        }

        // Process VAD chunks while we have enough samples.
        while vad_in_buf.len() >= vad_chunk_samples {
            vad_chunk.clear();
            vad_chunk.extend(vad_in_buf.drain(..vad_chunk_samples));
            // Push into whisper if we have an active session.
            if let Some(session) = active_session.as_mut() {
                if let Err(e) = session.push_audio(&vad_chunk) {
                    tracing::warn!("whisper push_audio: {e}");
                }
            }
            let frame = match vad.process_chunk(&vad_chunk) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("vad process_chunk: {e}");
                    continue;
                }
            };
            match frame.event {
                VadEvent::SpeechStart => {
                    // Open a fresh whisper session if one isn't already open.
                    if active_session.is_none() {
                        match whisper.open_session() {
                            Ok(s) => active_session = Some(s),
                            Err(e) => {
                                tracing::warn!("whisper open_session: {e}");
                            }
                        }
                    }
                    if event_tx.blocking_send(VadEvent::SpeechStart).is_err() {
                        // run_loop has dropped; exit thread.
                        return Ok(());
                    }
                }
                VadEvent::SpeechEnd => {
                    // Finalize whisper, send transcript BEFORE the event.
                    let segments = match active_session.take() {
                        Some(s) => match s.finalize() {
                            Ok(segs) => segs,
                            Err(e) => {
                                tracing::warn!("whisper finalize: {e}");
                                Vec::new()
                            }
                        },
                        None => Vec::new(),
                    };
                    let text: String = segments
                        .iter()
                        .map(|s| s.text.as_str())
                        .collect::<Vec<_>>()
                        .join("")
                        .trim()
                        .to_string();
                    if transcript_tx.send(text).is_err() {
                        return Ok(());
                    }
                    if event_tx.blocking_send(VadEvent::SpeechEnd).is_err() {
                        return Ok(());
                    }
                }
                VadEvent::None => {}
            }
        }
    }
}

#[cfg(test)]
mod mocks {
    use std::sync::{Arc, Mutex};

    use primer_core::error::Result;
    use primer_core::speech::{
        AudioChunk, Named, StreamingSpeechToText, StreamingTextToSpeech, SynthesisSession,
        TranscriptSegment, TranscriptionSession, VoiceProfile,
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
        fn push_text(&mut self, text: &str) -> Result<Vec<AudioChunk>> {
            if text.is_empty() {
                return Ok(vec![]);
            }
            Ok(vec![AudioChunk {
                samples: vec![0.5; self.chunk_samples],
                sample_rate: 22_050,
            }])
        }
        fn finalize(self: Box<Self>) -> Result<Vec<AudioChunk>> {
            Ok(vec![])
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
        assert_eq!(session.push_text("hi.").unwrap().len(), 1);
        assert_eq!(session.push_text("").unwrap().len(), 0);
    }

    /// Test 1 — happy path: scripted SpeechEnd → LLM called with expected
    /// transcript → audio chunks committed → run_loop returns transcripts.
    #[tokio::test]
    async fn happy_path_records_one_round_trip() {
        use std::sync::Mutex;

        use primer_core::speech::VadEvent;

        let backends = super::LoopBackends {
            stt: Arc::new(MockStreamingStt::new("hello primer")),
            tts: Arc::new(MockStreamingTts::new(64)),
            voice: primer_core::speech::VoiceProfile::default(),
        };

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

        let result = super::run_loop(backends, event_rx, responder, on_audio, false, None).await;
        let transcripts = result.expect("loop ok");
        assert_eq!(transcripts, vec!["hello primer".to_string()]);
        assert_eq!(*captured_transcript.lock().unwrap(), "hello primer");
        assert!(!committed.lock().unwrap().is_empty(), "audio was committed");
    }

    /// Test 4 — quit phrase short-circuits SPEAK: child says "goodbye",
    /// the responder returns an empty string. The loop pushes the
    /// transcript, hits the quit-phrase branch, and exits before
    /// reaching SPEAK — so no audio is committed regardless of the
    /// (empty) responder output.
    #[tokio::test]
    async fn quit_phrase_short_circuits_speak() {
        use primer_core::speech::VadEvent;

        let backends = super::LoopBackends {
            stt: Arc::new(MockStreamingStt::new("goodbye")),
            tts: Arc::new(MockStreamingTts::new(64)),
            voice: primer_core::speech::VoiceProfile::default(),
        };

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

        let result = super::run_loop(
            backends,
            event_rx,
            Box::new(EmptyResponder),
            on_audio,
            false,
            None,
        )
        .await;
        let transcripts = result.expect("loop ok");
        assert_eq!(transcripts, vec!["goodbye".to_string()]);
        assert!(
            committed.lock().unwrap().is_empty(),
            "quit phrase exits before SPEAK"
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
        let backends = super::LoopBackends {
            stt: Arc::new(MockStreamingStt::new("why does the sky look blue")),
            tts: Arc::new(MockStreamingTts::new(64)),
            voice: primer_core::speech::VoiceProfile::default(),
        };

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

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            super::run_loop(
                backends,
                event_rx,
                Box::new(CountingResponder {
                    count: cc_clone,
                    cancel_drops: cd_clone,
                }),
                on_audio,
                false,
                None,
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
    }

    /// Test 3 — commit on first audio: synthesis fires before any
    /// resumed speech. Audio reaches the speaker callback; subsequent
    /// VAD events arriving after commit do not affect the in-flight
    /// SPEAK phase.
    #[tokio::test]
    async fn commit_on_first_chunk_proceeds_to_speak() {
        use primer_core::speech::VadEvent;

        let backends = super::LoopBackends {
            stt: Arc::new(MockStreamingStt::new("hi primer")),
            tts: Arc::new(MockStreamingTts::new(64)),
            voice: primer_core::speech::VoiceProfile::default(),
        };

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

        let result = super::run_loop(
            backends,
            event_rx,
            Box::new(PromptResponder),
            on_audio,
            false,
            None,
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

        let backends = super::LoopBackends {
            stt: Arc::new(MockStreamingStt::new("hi")),
            tts: Arc::new(MockStreamingTts::new(64)),
            voice: primer_core::speech::VoiceProfile::default(),
        };

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
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            super::run_loop(
                backends,
                event_rx,
                Box::new(PromptResponder),
                on_audio,
                false,
                Some(Arc::clone(&is_speaking)),
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
            AudioChunk, Named, StreamingTextToSpeech, SynthesisSession, VadEvent, VoiceProfile,
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
            fn push_text(&mut self, text: &str) -> primer_core::error::Result<Vec<AudioChunk>> {
                if text.is_empty() {
                    return Ok(vec![]);
                }
                self.captured.lock().unwrap().push(text.to_string());
                Ok(vec![AudioChunk {
                    samples: vec![0.5; 64],
                    sample_rate: 22_050,
                }])
            }
            fn finalize(self: Box<Self>) -> primer_core::error::Result<Vec<AudioChunk>> {
                Ok(vec![])
            }
        }

        let captured = Arc::new(Mutex::new(Vec::<String>::new()));
        let backends = super::LoopBackends {
            stt: Arc::new(MockStreamingStt::new("hello primer")),
            tts: Arc::new(CapturingTts {
                captured: Arc::clone(&captured),
            }),
            voice: VoiceProfile::default(),
        };

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
        let result = super::run_loop(
            backends,
            event_rx,
            Box::new(ErrResponder),
            on_audio,
            false,
            None,
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
}

#[cfg(test)]
mod quit_tests {
    use super::is_quit_phrase;

    #[test]
    fn detects_goodbye_case_insensitive() {
        assert!(is_quit_phrase("Goodbye!"));
        assert!(is_quit_phrase("alright, GOODBYE then"));
    }

    #[test]
    fn detects_bye_primer() {
        assert!(is_quit_phrase("bye primer"));
        assert!(is_quit_phrase("Bye Primer."));
    }

    #[test]
    fn ignores_unrelated_transcripts() {
        assert!(!is_quit_phrase("why is the sky blue"));
        assert!(!is_quit_phrase("hello"));
        // "bye" alone is NOT a quit phrase — only "bye primer".
        assert!(!is_quit_phrase("bye"));
    }
}
