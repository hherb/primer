//! SPEAK: synthesise the reply with the mic gated, then wait for the
//! speaker to drain. Split out of `inner.rs` for #325 (stage 1);
//! [`speak`] pushes a finished reply phrase by phrase (stage 2) and
//! [`speak_streaming`] pushes a reply while it is still being generated
//! (stage 3).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use primer_core::consts::speech::DEFAULT_INTER_PHRASE_SILENCE_MS;
use primer_core::error::Result;
use primer_core::speech::{SynthesisEvent, SynthesisSession, VadEvent};

use crate::phrase_split::PhraseSplitter;
use crate::voice_loop::observer::{LoopObserver, VoiceState};
use crate::voice_loop::tts_markdown::{raw_phrases, tts_phrase, tts_phrases};

use super::think::to_inference_error;
use super::{DrainHook, FALLBACK_LINE, LoopBackends};

/// The audio side of SPEAK, lent by [`super::inner::run_loop_inner`] for
/// one turn: see there for `is_speaking` and the drain hook.
pub(super) struct SpeakIo<'a> {
    pub on_committed_audio: &'a mut (dyn FnMut(Vec<f32>) + Send),
    pub wait_for_speaker_drain: &'a mut Option<DrainHook>,
    pub is_speaking: &'a Option<Arc<AtomicBool>>,
}

/// How a streamed SPEAK ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SpeakOutcome {
    /// The reply (or the fallback after an error) was spoken in full.
    Finished,
    /// Stop pressed between phrases; no further phrases were pushed.
    Cancelled,
    /// External stop; the caller ends the loop.
    Stopped,
}

/// Announce SPEAK and gate the mic: from here until the speaker has
/// drained, the audio thread discards incoming samples (the Primer's
/// own voice would otherwise be transcribed as the next child utterance
/// via mic→speaker acoustic feedback).
fn begin<O: LoopObserver>(io: &SpeakIo<'_>, observer: &mut O) {
    observer.on_state_change(VoiceState::Speak, None);
    if let Some(flag) = io.is_speaking.as_ref() {
        flag.store(true, Ordering::SeqCst);
    }
}

/// Open a TTS session for the active locale. Returns it with the number
/// of silence samples to insert at each `PhraseEnd`
/// (`DEFAULT_INTER_PHRASE_SILENCE_MS` at the TTS's rate).
///
/// v1 has a single locale entry so these lookups can't miss in
/// practice; a miss is a Speech error (rather than a panic) because
/// `LoopBackends` is a public-ish struct that future code might
/// construct without the convenience helper.
fn open_tts(backends: &LoopBackends) -> Result<(Box<dyn SynthesisSession>, usize)> {
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
    let session = active_tts.open_session(active_voice)?;
    let silence = (active_tts.sample_rate() * DEFAULT_INTER_PHRASE_SILENCE_MS / 1000) as usize;
    Ok((session, silence))
}

/// The end-of-turn tail every exit from a started SPEAK runs: flush
/// sentinel, drain hook, un-gate, drop stale VAD events. Skipping it
/// would leave the mic gated forever; un-gating before the drain would
/// let the speaker tail be transcribed as the next child turn.
async fn end_of_turn(io: &mut SpeakIo<'_>, events: &mut tokio::sync::mpsc::Receiver<VadEvent>) {
    // Flush sentinel: empty Vec signals on_audio to drain any
    // resampler-leftover tail. Mock callbacks no-op on empty input.
    (io.on_committed_audio)(Vec::new());
    // Wait for cpal to actually empty the speaker ringbuf
    // before clearing the mic gate. Going through `spawn_blocking`
    // (in the hook the caller wired) keeps this off the tokio
    // worker so other async work isn't starved during the
    // drain wait. Replaces the old `samples / tts_rate + 0.4s`
    // heuristic sleep — exact instead of "fixed margin that's
    // too short on slow hardware and too long on fast hardware".
    if let Some(hook) = io.wait_for_speaker_drain.as_mut() {
        hook().await;
    }
    if let Some(flag) = io.is_speaking.as_ref() {
        flag.store(false, Ordering::SeqCst);
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

/// SPEAK for a reply that is already complete: one markdown-stripped
/// phrase per `push_text` (#325 stage 2). The text shown to the user
/// keeps the markdown; only the audio input is stripped.
pub(super) async fn speak<O: LoopObserver>(
    accumulated: &str,
    backends: &LoopBackends,
    io: &mut SpeakIo<'_>,
    events: &mut tokio::sync::mpsc::Receiver<VadEvent>,
    observer: &mut O,
) -> Result<()> {
    begin(io, observer);
    let (mut session, silence) = open_tts(backends)?;
    {
        let on_committed_audio = &mut *io.on_committed_audio;
        let mut on_event = |event: SynthesisEvent| match event {
            SynthesisEvent::Audio(chunk) => on_committed_audio(chunk.samples),
            SynthesisEvent::PhraseEnd => on_committed_audio(vec![0.0_f32; silence]),
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
        for phrase in tts_phrases(accumulated) {
            session.push_text(&phrase, &mut on_event)?;
        }
        session.finalize(&mut on_event)?;
    }
    end_of_turn(io, events).await;
    Ok(())
}

/// SPEAK while the reply is still being generated (#325 stage 3).
///
/// Called by [`super::think::think`] once the first phrase is ready and
/// the think hold has passed, with those `first` phrases, the
/// `splitter` holding the partial sentence after them, the still-running
/// `llm_fut`, and its `deltas`. Each later delta goes to the observer at
/// once and to TTS phrase by phrase. VAD events are ignored: the mic is
/// gated. `push_text` stays on this task, as in [`speak`] (see the
/// `spawn_blocking` rationale there); deltas arriving during a push wait
/// in `deltas`.
///
/// Stop and cancel are checked between phrases; either drops the session
/// without `finalize`. An LLM error *appends* `FALLBACK_LINE` to what
/// was already spoken (the spoken prefix is not recorded in the session:
/// owner decision D4). Every exit runs the end-of-turn tail.
#[allow(clippy::too_many_arguments)]
pub(super) async fn speak_streaming<O: LoopObserver, F: Future<Output = Result<String>>>(
    first: Vec<String>,
    mut splitter: PhraseSplitter,
    mut llm_fut: Pin<&mut F>,
    deltas: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
    backends: &LoopBackends,
    io: &mut SpeakIo<'_>,
    events: &mut tokio::sync::mpsc::Receiver<VadEvent>,
    observer: &mut O,
    primer_turn_index: usize,
    mut external_stop: Pin<&mut tokio::sync::oneshot::Receiver<()>>,
    cancel_response: &mut tokio::sync::mpsc::Receiver<()>,
) -> Result<SpeakOutcome> {
    begin(io, observer);
    let (mut session, silence) = open_tts(backends)?;
    let outcome = {
        let on_committed_audio = &mut *io.on_committed_audio;
        let mut on_event = |event: SynthesisEvent| match event {
            SynthesisEvent::Audio(chunk) => on_committed_audio(chunk.samples),
            SynthesisEvent::PhraseEnd => on_committed_audio(vec![0.0_f32; silence]),
        };
        let mut push = |session: &mut Box<dyn SynthesisSession>, raw: &str| -> Result<()> {
            match tts_phrase(raw) {
                Some(phrase) => session.push_text(&phrase, &mut on_event),
                None => Ok(()),
            }
        };
        for phrase in &first {
            push(&mut session, phrase)?;
        }
        let outcome = loop {
            tokio::select! {
                biased;
                _ = &mut external_stop => break SpeakOutcome::Stopped,
                _ = cancel_response.recv() => break SpeakOutcome::Cancelled,
                res = &mut llm_fut => {
                    match res {
                        Ok(_) => {
                            // Every delta was sent before the future resolved.
                            while let Ok(d) = deltas.try_recv() {
                                observer.on_response_chunk(primer_turn_index, &d);
                                for phrase in splitter.push(&d) {
                                    push(&mut session, &phrase)?;
                                }
                            }
                            if let Some(tail) = splitter.flush() {
                                push(&mut session, &tail)?;
                            }
                        }
                        Err(e) => {
                            observer.on_inference_error(&to_inference_error(e));
                            observer.on_response_chunk(primer_turn_index, FALLBACK_LINE);
                            for phrase in raw_phrases(FALLBACK_LINE) {
                                push(&mut session, &phrase)?;
                            }
                        }
                    }
                    break SpeakOutcome::Finished;
                }
                Some(d) = deltas.recv() => {
                    observer.on_response_chunk(primer_turn_index, &d);
                    for phrase in splitter.push(&d) {
                        push(&mut session, &phrase)?;
                    }
                }
            }
        };
        match outcome {
            SpeakOutcome::Finished => session.finalize(&mut on_event)?,
            _ => drop(session),
        }
        outcome
    };
    end_of_turn(io, events).await;
    Ok(outcome)
}
