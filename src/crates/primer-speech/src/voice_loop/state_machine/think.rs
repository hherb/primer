//! LATENT_THINK: finalize the child's utterance, drive the LLM, and
//! handle the VAD / user cancels that can interrupt it. Split out of
//! `inner.rs` for #325 (stage 1, behaviour-preserving).

use std::pin::Pin;

use primer_core::error::Result;
use primer_core::speech::{TranscriptionSession, VadEvent};

use crate::voice_loop::noise_transcript::strip_noise;
use crate::voice_loop::observer::{ExitReason, LoopObserver, VoiceState};

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
}

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
            break;
        }

        // Reset the chunk buffer at the top of each LLM attempt so
        // a cancelled-then-retried turn delivers only the second
        // attempt's chunks to the observer.
        chunk_buffer.lock().unwrap().clear();
        // A cancel only applies to the response it was pressed against;
        // presses queued during LISTEN/SPEAK are stale (#423).
        while cancel_response.try_recv().is_ok() {}

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
                        Err(e) => handle_llm_err(e, &chunk_buffer, observer),
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
                        Err(e) => handle_llm_err(e, &chunk_buffer, observer),
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
                                        Err(e) => handle_llm_err(e, &chunk_buffer, observer),
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
                                        Err(e) => handle_llm_err(e, &chunk_buffer, observer),
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
                return Ok(Think::Stopped);
            }
            LatentResult::CancelledByUser => {
                // Back to LISTEN with a `user_cancel` hint. Drop the
                // chunks accumulated for this aborted attempt.
                chunk_buffer.lock().unwrap().clear();
                observer.on_state_change(VoiceState::Listen, Some("user_cancel"));
                return Ok(Think::Cancelled);
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
            LatentResult::Completed => break,
        }
    }

    let chunks = std::mem::take(&mut *chunk_buffer.lock().unwrap());
    Ok(Think::Heard {
        transcript: transcript_so_far,
        accumulated,
        chunks,
    })
}
