//! The LISTEN → LATENT_THINK → SPEAK state-machine body shared by the
//! [`super::run_loop`] (spawn) and [`super::run_loop_borrowed`]
//! (in-place) entry points.

use primer_core::error::Result;

use crate::voice_loop::observer::{ExitReason, LoopObserver, TurnCompletePayload, VoiceState};

use super::speak::{SpeakIo, SpeakOutcome, speak};
use super::think::{Think, think};
use super::{DrainHook, LoopBackends, Responder};

/// Internal state-machine body shared by [`super::run_loop`] (spawn) and
/// [`super::run_loop_borrowed`] (in-place).
///
/// `verbose` gates `[stt]` debug lines on stderr (the only
/// stderr-printing site left after observer integration). Per-state
/// transitions, transcripts, chunks, completion, errors, and exits are
/// all delivered through `observer`. An `Err` (a TTS or STT backend
/// failure) also reports `Exit` + [`ExitReason::Error`] before returning.
///
/// `is_speaking` is the gate the audio thread checks to decide whether to
/// process or discard mic samples. The state machine flips it true at the
/// start of SPEAK and back to false after the synthesised audio has had
/// time to drain to the speaker. Tests pass `None` (mocks have no audio
/// thread).
///
/// `wait_for_speaker_drain` is awaited (in production) after the flush
/// sentinel returns and before `is_speaking` is cleared. Production wires
/// it to a `spawn_blocking` around `primer_speech::wait_for_drain` (`cpal`-gated);
/// tests pass `None` (mock speakers have no real ringbuf to drain).
///
/// `external_stop` ends the loop entirely (LISTEN, LATENT_THINK, SPEAK
/// — all states observe it as a cancel signal). `cancel_response` aborts
/// the in-flight LLM call + TTS synthesis and returns to LISTEN.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_loop_inner<'r, O: LoopObserver>(
    backends: LoopBackends,
    events: tokio::sync::mpsc::Receiver<primer_core::speech::VadEvent>,
    mut responder: Box<dyn Responder + 'r>,
    on_committed_audio: Box<dyn FnMut(Vec<f32>) + Send>,
    wait_for_speaker_drain: Option<DrainHook>,
    verbose: bool,
    is_speaking: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    mut observer: O,
    external_stop: tokio::sync::oneshot::Receiver<()>,
    cancel_response: tokio::sync::mpsc::Receiver<()>,
) -> Result<Vec<String>> {
    let result = run_loop_body(
        backends,
        events,
        responder.as_mut(),
        on_committed_audio,
        wait_for_speaker_drain,
        verbose,
        is_speaking,
        &mut observer,
        external_stop,
        cancel_response,
    )
    .await;
    if let Err(e) = &result {
        // The body's `?` exits report nothing; without this the UI stays
        // on its last state until the user ends voice mode (#429).
        tracing::warn!(target: "primer::speech", "voice loop ended with an error: {e}");
        observer.on_state_change(VoiceState::Exit, None);
        observer.on_exit(ExitReason::Error);
    }
    result
}

/// The loop itself; every `Ok` exit reports its own reason.
#[allow(clippy::too_many_arguments)]
async fn run_loop_body<O: LoopObserver>(
    backends: LoopBackends,
    mut events: tokio::sync::mpsc::Receiver<primer_core::speech::VadEvent>,
    responder: &mut (dyn Responder + '_),
    mut on_committed_audio: Box<dyn FnMut(Vec<f32>) + Send>,
    mut wait_for_speaker_drain: Option<DrainHook>,
    verbose: bool,
    is_speaking: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    observer: &mut O,
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
        let stt_session = backends.stt.open_session()?;
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

        // ── LATENT_THINK (and a streamed SPEAK, #325) ────────────────
        let mut io = SpeakIo {
            on_committed_audio: on_committed_audio.as_mut(),
            wait_for_speaker_drain: &mut wait_for_speaker_drain,
            is_speaking: &is_speaking,
        };
        let (transcript_so_far, accumulated, chunks) = match think(
            &backends,
            stt_session,
            &mut events,
            &mut *responder,
            verbose,
            observer,
            external_stop.as_mut(),
            &mut cancel_response,
            &mut io,
            primer_turn_index,
        )
        .await?
        {
            Think::Stopped => return Ok(transcripts),
            Think::Cancelled => continue 'outer,
            Think::Quit { transcript } => {
                observer.on_transcript_finalized(&transcript);
                transcripts.push(transcript);
                observer.on_state_change(VoiceState::Exit, None);
                observer.on_exit(ExitReason::Keyword);
                return Ok(transcripts);
            }
            Think::Spoke {
                transcript,
                outcome,
            } => {
                let child_turn_index = transcripts.len();
                transcripts.push(transcript);
                if outcome == SpeakOutcome::Stopped {
                    observer.on_state_change(VoiceState::Exit, None);
                    observer.on_exit(ExitReason::UserStop);
                    return Ok(transcripts);
                }
                observer.on_response_complete(TurnCompletePayload {
                    session_id: uuid::Uuid::nil(),
                    child_turn_index,
                    primer_turn_index,
                });
                primer_turn_index += 1;
                if outcome == SpeakOutcome::Cancelled {
                    observer.on_state_change(VoiceState::Listen, Some("user_cancel"));
                }
                continue 'outer;
            }
            Think::Heard {
                transcript,
                accumulated,
                chunks,
            } => (transcript, accumulated, chunks),
        };

        // ── Commit transcript ─────────────────────────────────────────
        if transcript_so_far.is_empty() {
            continue;
        }
        observer.on_transcript_finalized(&transcript_so_far);
        // Replay accumulated chunks onto the observer so streaming
        // semantics (one observer.on_response_chunk per actual chunk)
        // are preserved while we still get to take `observer` by `&mut`.
        for c in chunks {
            observer.on_response_chunk(primer_turn_index, &c);
        }
        let child_turn_index = transcripts.len();
        transcripts.push(transcript_so_far);

        // ── SPEAK ─────────────────────────────────────────────────────
        if !accumulated.is_empty() {
            speak(&accumulated, &backends, &mut io, &mut events, observer).await?;
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
