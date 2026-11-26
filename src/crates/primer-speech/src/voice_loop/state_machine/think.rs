//! LATENT_THINK: finalize the child's utterance, drive the LLM, and
//! handle the VAD / user cancels that can interrupt it. Split out of
//! `inner.rs` for #325 (stage 1, behaviour-preserving).

use std::pin::Pin;

use primer_core::consts::speech::MIN_THINK_HOLD;
use primer_core::error::{InferenceError, PrimerError, Result};
use primer_core::speech::{TranscriptionSession, VadEvent};

use crate::phrase_split::PhraseSplitter;
use crate::voice_loop::noise_transcript::strip_noise;
use crate::voice_loop::observer::{ExitReason, LoopObserver, VoiceState};
use crate::voice_loop::quit_detect::is_quit_phrase;

use super::speak::{SpeakIo, SpeakOutcome, speak_streaming};
use super::{FALLBACK_LINE, LoopBackends, Responder};

/// How LATENT_THINK ended.
pub(super) enum Think {
    /// External stop; `Exit` / `UserStop` were already emitted.
    Stopped,
    /// User cancel; the `Listen` + `user_cancel` transition was already
    /// emitted. The caller goes back to LISTEN.
    Cancelled,
    /// The utterance (empty when it was only noise), the reply, and the
    /// reply's chunks in arrival order for the observer replay.
    Heard {
        transcript: String,
        accumulated: String,
        chunks: Vec<String>,
    },
    /// A quit phrase, caught before any LLM call (owner decision D2).
    Quit { transcript: String },
    /// The reply streamed into TTS while it was generated (#325 stage 3);
    /// the transcript and the chunks were already sent to the observer.
    Spoke {
        transcript: String,
        outcome: SpeakOutcome,
    },
}

/// Handle an LLM error inside the LATENT_THINK select arms.
///
/// Used only before audio has started (after that, `speak_streaming`
/// appends the fallback instead). Surfaces the typed error to the
/// observer, then **drops** any chunks the partial attempt collected and
/// replaces them with a single synthetic FALLBACK_LINE chunk. The replay loop
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
    err: PrimerError,
    chunks: &mut Vec<String>,
    observer: &mut O,
) -> String {
    observer.on_inference_error(&to_inference_error(err));
    chunks.clear();
    chunks.push(FALLBACK_LINE.to_string());
    FALLBACK_LINE.to_string()
}

/// The typed error for the observer: an `Inference` error keeps its
/// variant; any other `PrimerError` becomes `Other` (dev-facing text the
/// i18n layer redacts).
pub(super) fn to_inference_error(err: PrimerError) -> InferenceError {
    match err {
        PrimerError::Inference(inf) => inf,
        other => other.to_string().into(),
    }
}

/// The LATENT_THINK state: see [`super::inner::run_loop_inner`] for the
/// meaning of each channel.
#[allow(clippy::too_many_arguments)]
pub(super) async fn think<'r, O: LoopObserver>(
    backends: &LoopBackends,
    mut stt_session: Box<dyn TranscriptionSession>,
    events: &mut tokio::sync::mpsc::Receiver<VadEvent>,
    responder: &mut (dyn Responder + 'r),
    verbose: bool,
    observer: &mut O,
    mut external_stop: Pin<&mut tokio::sync::oneshot::Receiver<()>>,
    cancel_response: &mut tokio::sync::mpsc::Receiver<()>,
    io: &mut SpeakIo<'_>,
    primer_turn_index: usize,
) -> Result<Think> {
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
    loop {
        // Peek (finalize-and-reopen since whisper-cpp-plus has no
        // partial-extract API exposed here): we accept the slight
        // mock-friendliness — production whisper supports peeking via
        // process_step but the trait surface is finalize-only today.
        let segments = stt_session.finalize()?;
        let raw_text = segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join("");
        stt_session = backends.stt.open_session()?;
        // STT noise ("[BLANK_AUDIO]", "Thank you.") is never appended
        // (#326, #419). Judged per iteration, so a noise continuation
        // after a cancel neither joins nor erases the child's real
        // question; noise alone leaves the transcript empty: no LLM
        // call, no turn.
        let new_text = strip_noise(&raw_text, &backends.active_locale);
        if new_text != raw_text.trim() {
            tracing::debug!(target: "primer::speech", raw = %raw_text, kept = %new_text, "dropped STT noise");
        }
        if !new_text.is_empty() {
            if !transcript_so_far.is_empty() {
                transcript_so_far.push(' ');
            }
            transcript_so_far.push_str(new_text);
        }

        if transcript_so_far.is_empty() {
            if verbose {
                eprintln!("[stt] empty transcript, looping");
            }
            return Ok(Think::Heard {
                transcript: transcript_so_far,
                accumulated: String::new(),
                chunks: Vec::new(),
            });
        }

        // D2 (#325): a quit phrase ends the session before any LLM call,
        // so it records no DM turn and nothing is said in reply.
        if is_quit_phrase(&transcript_so_far, &backends.active_locale) {
            return Ok(Think::Quit {
                transcript: transcript_so_far,
            });
        }

        // A cancel only applies to the response it was pressed against;
        // presses queued during LISTEN/SPEAK are stale (#423).
        while cancel_response.try_recv().is_ok() {}

        // Drive the LLM. `on_chunk` only forwards each delta; it never
        // touches TTS, so `push_text` stays on this task. The channel and
        // the splitter are fresh per attempt, so a cancelled attempt's
        // partial sentence never prefixes the retry's first phrase.
        let (delta_tx, mut delta_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let on_chunk: Box<dyn FnMut(&str) + Send + 'r> = Box::new(move |c: &str| {
            let _ = delta_tx.send(c.to_string());
        });
        let llm_fut = responder.respond(&transcript_so_far, on_chunk);
        tokio::pin!(llm_fut);

        // Deltas so far, for the observer; complete phrases not yet
        // spoken; the partial sentence after them.
        let mut chunks: Vec<String> = Vec::new();
        let mut ready: Vec<String> = Vec::new();
        let mut splitter = PhraseSplitter::new();
        // D1: speech starts no earlier than this, so the child can still
        // resume (and the turn is retried) even when a phrase is ready.
        let hold_until = tokio::time::Instant::now() + MIN_THINK_HOLD;
        let mut events_open = true;
        let mut accumulated = String::new();

        enum LatentResult {
            Completed,
            CancelledByVad,
            CancelledByUser,
            Stopped,
            StartSpeaking,
        }
        let outcome = loop {
            // `biased` ensures the LLM future gets polled before
            // events.recv() — without it, select!'s random order can
            // resolve a queued SpeechStart-cancel before the LLM
            // future is ever polled, leaving the LLM call's destructor
            // un-run because it never started. Bias makes
            // cancellation observable and testable: if cancel fires,
            // the parking future has been polled at least once.
            tokio::select! {
                biased;
                _ = &mut external_stop => break LatentResult::Stopped,
                _ = cancel_response.recv() => break LatentResult::CancelledByUser,
                res = &mut llm_fut => {
                    accumulated = match res {
                        Ok(text) => {
                            // Every delta was sent before the future resolved.
                            while let Ok(d) = delta_rx.try_recv() {
                                chunks.push(d);
                            }
                            text
                        }
                        Err(e) => handle_llm_err(e, &mut chunks, observer),
                    };
                    break LatentResult::Completed;
                }
                event = events.recv(), if events_open => match event {
                    // The child resumed: drop the future, keep listening.
                    Some(VadEvent::SpeechStart) => break LatentResult::CancelledByVad,
                    // Spurious (we entered on SpeechEnd): keep waiting.
                    Some(_) => {}
                    // Channel closed and drained: no more VAD events can
                    // arrive, so wait for the LLM alone.
                    None => events_open = false,
                },
                Some(d) = delta_rx.recv() => {
                    ready.extend(splitter.push(&d));
                    chunks.push(d);
                }
                _ = tokio::time::sleep_until(hold_until), if !ready.is_empty() => {
                    break LatentResult::StartSpeaking;
                }
            }
        };

        match outcome {
            LatentResult::Stopped => {
                observer.on_state_change(VoiceState::Exit, None);
                observer.on_exit(ExitReason::UserStop);
                return Ok(Think::Stopped);
            }
            LatentResult::CancelledByUser => {
                // Back to LISTEN with a `user_cancel` hint; this attempt's
                // chunks are dropped with it.
                observer.on_state_change(VoiceState::Listen, Some("user_cancel"));
                return Ok(Think::Cancelled);
            }
            LatentResult::CancelledByVad => {
                // VAD-cancel-on-resumed-speech: signal the LISTEN
                // transition, then wait for the next SpeechEnd to retry.
                observer.on_state_change(VoiceState::Listen, Some("child_resumed"));
                loop {
                    tokio::select! {
                        biased;
                        _ = &mut external_stop => {
                            observer.on_state_change(VoiceState::Exit, None);
                            observer.on_exit(ExitReason::UserStop);
                            return Ok(Think::Stopped);
                        }
                        evt = events.recv() => {
                            let Some(event) = evt else {
                                observer.on_state_change(VoiceState::Exit, None);
                                observer.on_exit(ExitReason::UserStop);
                                return Ok(Think::Stopped);
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
            LatentResult::Completed => {
                return Ok(Think::Heard {
                    transcript: transcript_so_far.clone(),
                    accumulated,
                    chunks,
                });
            }
            LatentResult::StartSpeaking => {
                observer.on_transcript_finalized(&transcript_so_far);
                for c in &chunks {
                    observer.on_response_chunk(primer_turn_index, c);
                }
                let outcome = speak_streaming(
                    ready,
                    splitter,
                    llm_fut.as_mut(),
                    &mut delta_rx,
                    backends,
                    io,
                    events,
                    observer,
                    primer_turn_index,
                    external_stop.as_mut(),
                    cancel_response,
                )
                .await?;
                return Ok(Think::Spoke {
                    transcript: transcript_so_far.clone(),
                    outcome,
                });
            }
        }
    }
}
