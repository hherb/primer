//! Entry points wrapping the state-machine body: the spawn-based
//! [`run_loop`] (production) and the in-place [`run_loop_borrowed`]
//! (tests / callers that share borrowed state with the loop).

use primer_core::error::Result;

use crate::voice_loop::observer::LoopObserver;

use super::inner::run_loop_inner;
use super::{DrainHook, LoopBackends, LoopHandle, Responder, VoiceLoopError};

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
