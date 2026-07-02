//! The LISTEN → LATENT_THINK → SPEAK state-machine body shared by the
//! [`super::run_loop`] (spawn) and [`super::run_loop_borrowed`]
//! (in-place) entry points.

use primer_core::consts::speech::DEFAULT_INTER_PHRASE_SILENCE_MS;
use primer_core::error::Result;
use primer_core::speech::SynthesisEvent;

use crate::voice_loop::observer::{ExitReason, LoopObserver, TurnCompletePayload, VoiceState};
use crate::voice_loop::quit_detect::is_quit_phrase;
use crate::voice_loop::tts_markdown::strip_markdown_for_tts;

use super::handle_llm_err;
use super::{DrainHook, LoopBackends, Responder};

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
pub(super) async fn run_loop_inner<'r, O: LoopObserver>(
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
