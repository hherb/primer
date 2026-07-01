//! Per-turn streaming + cancellation.
//!
//! `send_message` clones the DM Arc out of the session guard, releases
//! the session guard, then locks the DM independently for the duration
//! of the turn — so the long streaming wallclock doesn't keep other
//! commands (e.g. `current_session_info`) waiting. After each
//! successful turn it refreshes [`SessionSnapshot`] so reader commands
//! never touch the DM lock at all. `cancel_response` aborts the
//! in-flight turn via its stashed abort handle.

use std::sync::Arc;

use primer_pedagogy::DialogueManager;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::state::{ActiveSession, AppState, SessionSnapshot};
use crate::types::{ChunkEvent, TurnComplete};

/// Drive one streaming dialogue turn end-to-end.
///
/// Flow:
/// 1. Clone the DM Arc out of the session guard and release the
///    session guard. Other commands (`current_session_info`,
///    `update_settings`, …) keep running for the duration of the turn.
/// 2. Lock the DM mutex. Concurrent `send_message` calls serialise
///    here — there can only be one in-flight turn at a time, which is
///    what the pedagogy crate expects.
/// 3. Capture the Primer-turn index, then run `respond_to_streaming`.
///    `await_pending_background` at the top of that method drains the
///    PREVIOUS turn's classifier / extractor / comprehension tasks
///    inside the natural inter-turn pause — that's the latency
///    property we get back from holding DM long-lived.
/// 4. Emit `primer://turn_complete` immediately on success. The current
///    turn's spawned background tasks keep running in the background
///    and will be awaited at the start of the next turn (or at
///    `close_session`).
#[tauri::command]
pub async fn send_message(
    state: tauri::State<'_, AppState>,
    app: AppHandle,
    input: String,
) -> Result<TurnComplete, String> {
    // Clone the Arcs under the session mutex, then release it. The DM
    // Arc drives the turn; the snapshot Arc is refreshed at the end so
    // reader commands see fresh learner/session-id fields without ever
    // locking the DM. The `current_turn_abort` slot is set / cleared
    // via brief re-locks of `state.session` below so the spawned task
    // doesn't hold the session lock for its full streaming wallclock.
    let (dm_arc, snapshot_arc) = {
        let guard = state.session.lock().await;
        let active = guard
            .as_ref()
            .ok_or_else(|| "no active session — call start_session first".to_string())?;
        (
            Arc::clone(&active.dialogue_manager),
            Arc::clone(&active.snapshot),
        )
    };

    // Spawn the turn in a dedicated task so `cancel_response` can abort
    // it. The chunk emitter lives inside the spawn because closures
    // crossing the spawn boundary need owned captures.
    //
    // There is a microseconds-wide window between this `spawn` and the
    // slot-stash below in which a `cancel_response` would observe an
    // empty slot and silently no-op. Unobservable at human reaction
    // times; closing it would mean pre-allocating a slot before the
    // task exists, which adds complexity for no realistic gain.
    let dm_clone = Arc::clone(&dm_arc);
    let app_clone = app.clone();
    let input_clone = input.clone();
    let task = tokio::spawn(async move {
        run_turn(&dm_clone, &input_clone, |primer_turn_index, chunk| {
            let payload = ChunkEvent {
                primer_turn_index,
                text: chunk.to_string(),
            };
            if let Err(e) = app_clone.emit("primer://chunk", &payload) {
                tracing::warn!("emit primer://chunk failed: {e}");
            }
        })
        .await
    });

    // Stash the abort handle so `cancel_response` can target this turn.
    // Held only briefly; cleared on completion below.
    {
        let guard = state.session.lock().await;
        if let Some(active) = guard.as_ref() {
            *active.current_turn_abort.lock().await = Some(task.abort_handle());
        }
    }

    let join_result = task.await;

    // Clear the abort slot whether we succeeded, failed, or were
    // cancelled — leaving a stale handle behind would let a second
    // cancel hit a no-op task.
    {
        let guard = state.session.lock().await;
        if let Some(active) = guard.as_ref() {
            *active.current_turn_abort.lock().await = None;
        }
    }

    let payload = match join_result {
        Ok(Ok(payload)) => payload,
        Ok(Err(e)) => return Err(e),
        Err(join_err) if join_err.is_cancelled() => {
            // User-initiated cancel. The child turn stays in the in-memory
            // session; the partial Primer turn drops per DM's existing
            // "mid-stream error" semantic. The frontend's cancellation
            // path already knows what to do with the streaming bubble.
            return Err(CANCELLED_MESSAGE.to_string());
        }
        Err(join_err) => return Err(format!("turn task crashed: {join_err}")),
    };

    refresh_snapshot(&dm_arc, &snapshot_arc).await;

    if let Err(e) = app.emit("primer://turn_complete", &payload) {
        tracing::warn!("emit primer://turn_complete failed: {e}");
    }
    Ok(payload)
}

/// Sentinel returned by `send_message` when the user cancelled the
/// turn via `cancel_response`. Deliberately a machine-readable token
/// (`namespace:event`) rather than a user-facing sentence: the
/// frontend matches the exact value to suppress the error banner,
/// and any user-facing wording is the frontend's concern. If the
/// token ever leaks past the frontend match it reads as an obvious
/// bug rather than masquerading as a localised string.
///
/// Must stay in lockstep with `CANCEL_SENTINEL` in `ui/app.js`. The
/// `cancelled_message_is_stable_machine_token` unit test pins the
/// value so a one-sided change here breaks CI immediately.
pub const CANCELLED_MESSAGE: &str = "primer:turn_cancelled";

/// Abort the in-flight turn associated with `active`, if any. Pure
/// helper (no Tauri state) so unit tests can drive it directly. The
/// `cancel_response` command is a thin lookup-then-delegate wrapper
/// around this.
pub(crate) async fn cancel_active_turn(active: &ActiveSession) {
    let abort_guard = active.current_turn_abort.lock().await;
    if let Some(handle) = abort_guard.as_ref() {
        handle.abort();
    }
}

/// Abort the in-flight turn, if any. Idempotent — calling with no
/// active turn (or no active session) is a no-op and returns Ok.
///
/// Triggers `JoinHandle::abort()` on the spawned task. The aborted
/// future drops mid-`respond_to_streaming`, which:
/// - Releases the DM lock guard.
/// - Leaves the already-appended child turn in `dm.session.turns`
///   (so the next `send_message` continues the same conversation).
/// - Drops the partial Primer response per DM's "mid-stream error
///   → no Primer turn recorded" invariant.
///
/// `send_message` observes the abort via `JoinError::is_cancelled`
/// and returns the [`CANCELLED_MESSAGE`] sentinel so the frontend
/// can distinguish user-cancel from a genuine error.
#[tauri::command]
pub async fn cancel_response(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let guard = state.session.lock().await;
    if let Some(active) = guard.as_ref() {
        cancel_active_turn(active).await;
    }
    Ok(())
}

/// Re-read learner + session-id fields from the just-completed DM and
/// publish them into the snapshot. The DM lock is held only for the
/// few field reads — no `.await` inside — so any concurrent
/// `current_session_info` waiting on the snapshot mutex returns in
/// microseconds, not the streaming wallclock.
pub(crate) async fn refresh_snapshot(
    dm_arc: &Arc<Mutex<DialogueManager>>,
    snapshot_arc: &Arc<Mutex<SessionSnapshot>>,
) {
    let new_snapshot = {
        let dm = dm_arc.lock().await;
        SessionSnapshot {
            session_id: Some(dm.session.id),
            learner_id: dm.learner.profile.id,
            learner_name: dm.learner.profile.name.clone(),
            learner_age: dm.learner.profile.age,
            concept_count: dm.learner.concepts.len(),
        }
    };
    *snapshot_arc.lock().await = new_snapshot;
}

/// Drive one streaming turn against a held DM.
///
/// Split out from `send_message` so unit tests can exercise the full
/// lock / respond / unlock flow without a Tauri `AppHandle` in scope.
///
/// `on_chunk` is invoked once per streamed token with the Primer-turn
/// index and the chunk text.
pub(crate) async fn run_turn<F>(
    dm_arc: &Arc<Mutex<DialogueManager>>,
    input: &str,
    mut on_chunk: F,
) -> Result<TurnComplete, String>
where
    F: FnMut(usize, &str),
{
    let mut dm = dm_arc.lock().await;

    // The Primer turn lands at `turns.len() + 1` after the child turn
    // appends at `turns.len()`. Captured before respond_to_streaming
    // runs so the chunk callback can address its bubble.
    let primer_turn_index = dm.session.turns.len() + 1;
    let session_id: Uuid = dm.session.id;

    dm.respond_to_streaming(input, |chunk| on_chunk(primer_turn_index, chunk))
        .await
        .map_err(|e| e.to_string())?;

    Ok(TurnComplete {
        session_id,
        child_turn_index: primer_turn_index - 1,
        primer_turn_index,
    })
}
