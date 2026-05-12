//! Session lifecycle + per-turn streaming commands.
//!
//! `start_session` builds an [`ActiveSession`] (which carries a
//! long-lived [`DialogueManager`]) from the persisted `GuiConfig` and
//! stores it in `AppState`. `close_session` drops it, draining any
//! in-flight background tasks first via `dm.close_session()`.
//!
//! `send_message` clones the DM Arc out of the session guard, releases
//! the session guard, then locks the DM independently for the duration
//! of the turn — so the long streaming wallclock doesn't keep other
//! commands (e.g. `current_session_info`) waiting. After each
//! successful turn it refreshes [`ActiveSession::snapshot`] so reader
//! commands never touch the DM lock at all.

use std::sync::Arc;

use primer_pedagogy::DialogueManager;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::state::{ActiveSession, AppState, SessionSnapshot};
use crate::types::{
    ChunkEvent, ComprehensionSummary, ConceptBreakdown, EngagementSummary, LearnerSummary,
    SessionInfo, TurnComplete, TurnSignals,
};
use crate::wiring;

/// Construct an `ActiveSession` from the persisted settings and store
/// it in `AppState`. Errors surface as `String` for inline rendering.
///
/// If a session is already open, it is closed first (no resource leak
/// even if the frontend forgets to close before re-starting). The
/// previous learner's state is saved as part of `close_session`'s
/// internal drain.
#[tauri::command]
pub async fn start_session(state: tauri::State<'_, AppState>) -> Result<SessionInfo, String> {
    close_session_inner(&state).await?;

    let cfg = state.config.lock().await.clone();
    let active = wiring::build_active_session(&state.home, &cfg).await?;
    let info = info_from(&active).await;
    *state.session.lock().await = Some(active);
    Ok(info)
}

/// Drop the active session, if any. Idempotent — calling it with no
/// active session is a no-op (returns Ok).
///
/// Drains the DM's background tasks (classifier / extractor /
/// comprehension) before drop so the final turn's analysis lands on
/// disk before the Arcs are released.
#[tauri::command]
pub async fn close_session(state: tauri::State<'_, AppState>) -> Result<(), String> {
    close_session_inner(&state).await
}

/// Return a summary of the active session, or `None` if no session is
/// open. Used by the frontend on launch to decide whether to render
/// the picker or the chat view.
#[tauri::command]
pub async fn current_session_info(
    state: tauri::State<'_, AppState>,
) -> Result<Option<SessionInfo>, String> {
    let guard = state.session.lock().await;
    if let Some(active) = guard.as_ref() {
        Ok(Some(info_from(active).await))
    } else {
        Ok(None)
    }
}

/// Return the pedagogical signals for the most-recently completed
/// exchange (intent, engagement, concepts, comprehension).
///
/// **DM-lock duration: brief.** Locks the DM mutex only long enough to
/// clone a handful of `last_*` accessor outputs; no `.await` inside
/// the locked region. A `current_session_info` request issued in
/// parallel with this one therefore queues for microseconds, not the
/// streaming wallclock. Holding the lock for an in-flight
/// `send_message` is fine — the lock-wait just defers the signal
/// refresh until the response finishes streaming, which is the
/// correct UX (the signals don't change until then anyway).
///
/// **Why not via `SessionSnapshot` like `current_session_info`?** That
/// snapshot exists so reader commands NEVER touch the DM mutex, since
/// they may fire during a streaming turn. Signals don't have that
/// constraint: the frontend refreshes them on `primer://turn_complete`
/// (post-stream, DM free) and on launch — never during a turn. A brief
/// post-stream DM lock costs less than the per-turn snapshot-write
/// fan-out we'd otherwise add to `refresh_snapshot`. If a future
/// caller ever needs live mid-stream signals, fold them into
/// `SessionSnapshot` instead of relaxing this invariant.
///
/// Returns `Ok(None)` when no session is active.
#[tauri::command]
pub async fn get_turn_signals(
    state: tauri::State<'_, AppState>,
) -> Result<Option<TurnSignals>, String> {
    let session_guard = state.session.lock().await;
    let active = match session_guard.as_ref() {
        Some(a) => a,
        None => return Ok(None),
    };
    let dm_arc = Arc::clone(&active.dialogue_manager);
    drop(session_guard);

    let dm = dm_arc.lock().await;
    Ok(Some(read_signals(&dm)))
}

/// Pure-ish read of the DM's `last_*` accessors. Split out from
/// `get_turn_signals` so step 6's learner snapshot can reuse the same
/// pattern (one DM lock per sidebar refresh) without duplicating the
/// shape mapping. No `.await` — caller must already hold the DM lock.
pub(crate) fn read_signals(dm: &DialogueManager) -> TurnSignals {
    // Wire strings come from each enum's `name()` — the same canonical
    // identifiers the storage lookup tables use. Don't switch back to
    // `format!("{:?}", ...)`: Debug output is not a stable contract and
    // frontend CSS/keys depend on these exact strings.
    let intent = dm.last_intent().map(|i| i.name().to_string());
    let engagement = dm.last_assessment().map(|a| EngagementSummary {
        state: a.state.name().to_string(),
        confidence: a.confidence,
        reasoning: a.reasoning.clone(),
    });
    let concepts = dm
        .last_extraction()
        .map(|e| ConceptBreakdown {
            child: e.child_concepts.clone(),
            primer: e.primer_concepts.clone(),
        })
        .unwrap_or_default();
    let comprehension = dm
        .last_comprehension()
        .map(|r| {
            r.assessments
                .iter()
                .map(|a| ComprehensionSummary {
                    concept: a.concept.clone(),
                    depth: a.depth.name().to_string(),
                    confidence: a.confidence,
                    evidence: a.evidence.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    TurnSignals {
        intent,
        engagement,
        concepts,
        comprehension,
        classifier_identifier: dm.classifier_identifier().to_string(),
        extractor_identifier: dm.extractor_identifier().to_string(),
        comprehension_identifier: dm.comprehension_identifier().to_string(),
    }
}

/// Internal helper used by both `close_session` and `start_session`.
///
/// Two-step lock dance: pop the `ActiveSession` out of the session
/// mutex first (so other commands aren't blocked while DM drain runs),
/// then lock the DM mutex and call `close_session` on it. The DM mutex
/// lock will WAIT for any in-flight `send_message` to finish — exactly
/// the right behaviour so a "Close" click never aborts a partially-
/// streamed response.
async fn close_session_inner(state: &tauri::State<'_, AppState>) -> Result<(), String> {
    let active = state.session.lock().await.take();
    if let Some(active) = active {
        let mut dm = active.dialogue_manager.lock().await;
        dm.close_session().await;
    }
    Ok(())
}

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
    // Clone both Arcs under the session mutex, then release it. The
    // DM Arc drives the turn; the snapshot Arc is refreshed at the
    // end so reader commands see fresh learner/session-id fields
    // without ever locking the DM.
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

    let payload = run_turn(&dm_arc, &input, |primer_turn_index, chunk| {
        let payload = ChunkEvent {
            primer_turn_index,
            text: chunk.to_string(),
        };
        if let Err(e) = app.emit("primer://chunk", &payload) {
            tracing::warn!("emit primer://chunk failed: {e}");
        }
    })
    .await?;

    refresh_snapshot(&dm_arc, &snapshot_arc).await;

    if let Err(e) = app.emit("primer://turn_complete", &payload) {
        tracing::warn!("emit primer://turn_complete failed: {e}");
    }
    Ok(payload)
}

/// Re-read learner + session-id fields from the just-completed DM and
/// publish them into the snapshot. The DM lock is held only for the
/// few field reads — no `.await` inside — so any concurrent
/// `current_session_info` waiting on the snapshot mutex returns in
/// microseconds, not the streaming wallclock.
async fn refresh_snapshot(
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
async fn run_turn<F>(
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

async fn info_from(active: &ActiveSession) -> SessionInfo {
    // Reads ONLY from the snapshot — never touches the DM mutex —
    // so a sidebar refresh during a streaming turn returns immediately
    // instead of queueing behind the entire response wallclock.
    let snapshot = active.snapshot.lock().await;
    SessionInfo {
        session_id: snapshot.session_id,
        learner: LearnerSummary {
            id: snapshot.learner_id,
            name: snapshot.learner_name.clone(),
            age: snapshot.learner_age,
            concept_count: snapshot.concept_count,
        },
        backend_kind: active.backend_name.clone(),
        main_model: active.main_model.clone(),
        locale: active.locale.pack_id().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GuiConfig;
    use crate::wiring::build_active_session;
    use primer_core::conversation::PedagogicalIntent;
    use primer_core::learner::EngagementState;
    use tempfile::TempDir;

    fn stub_config_with_persistence(home: &std::path::Path) -> GuiConfig {
        // Persist to a real on-disk session DB so the second turn's
        // resume_session path is exercised against actual storage.
        let mut cfg = GuiConfig::default();
        cfg.persistence.session_db = Some(home.join("test_session.db"));
        cfg
    }

    #[tokio::test]
    async fn first_turn_creates_session_and_returns_payload() {
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();
        let dm_arc = Arc::clone(&active.dialogue_manager);

        let mut chunks = Vec::<(usize, String)>::new();
        let payload = run_turn(&dm_arc, "hello", |i, c| chunks.push((i, c.to_string())))
            .await
            .unwrap();

        assert_eq!(payload.child_turn_index, 0, "child lands at index 0");
        assert_eq!(payload.primer_turn_index, 1, "primer lands at index 1");
        assert!(!chunks.is_empty(), "stub backend emits at least one chunk");
        for (idx, _) in &chunks {
            assert_eq!(*idx, payload.primer_turn_index);
        }
    }

    #[tokio::test]
    async fn second_turn_continues_same_session() {
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();
        let dm_arc = Arc::clone(&active.dialogue_manager);

        let first = run_turn(&dm_arc, "hello", |_, _| {}).await.unwrap();
        let second = run_turn(&dm_arc, "tell me more", |_, _| {}).await.unwrap();

        assert_eq!(
            first.session_id, second.session_id,
            "long-lived DM holds the same Session across turns"
        );
        assert_eq!(
            second.child_turn_index, 2,
            "child #2 lands after first exchange"
        );
        assert_eq!(second.primer_turn_index, 3, "primer #2 lands at index 3");
    }

    #[tokio::test]
    async fn turn_persists_to_session_store() {
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();
        let dm_arc = Arc::clone(&active.dialogue_manager);

        let payload = run_turn(&dm_arc, "what is curiosity?", |_, _| {})
            .await
            .unwrap();

        // Drain the DM's background tasks before re-opening the same
        // DB from a second connection so we don't race a still-running
        // extractor/comprehension/embedding write through the first.
        dm_arc.lock().await.close_session().await;

        // Re-open via the test config's session-db path so we validate
        // the actual on-disk artefact independently of any DM-internal
        // caching.
        let session_db = home.path().join("test_session.db");
        let store = primer_storage::SqliteSessionStore::open_for_locale(&session_db, active.locale)
            .unwrap();
        let loaded = primer_core::storage::SessionStore::load_session(&store, payload.session_id)
            .await
            .unwrap()
            .expect("session must be loadable after first turn");
        assert!(
            loaded.turns.len() >= 2,
            "session must persist both the child and primer turns; got {} turns",
            loaded.turns.len()
        );
    }

    #[tokio::test]
    async fn read_signals_empty_before_any_turn() {
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();
        let dm = active.dialogue_manager.lock().await;
        let signals = read_signals(&dm);
        assert!(signals.intent.is_none(), "no intent before any turn");
        assert!(
            signals.engagement.is_none(),
            "no engagement before any turn"
        );
        assert!(signals.concepts.child.is_empty());
        assert!(signals.concepts.primer.is_empty());
        assert!(signals.comprehension.is_empty());
        // Identifiers are populated at construction (subsystems always exist).
        assert_eq!(signals.classifier_identifier, "stub");
        assert_eq!(signals.extractor_identifier, "stub");
        assert_eq!(signals.comprehension_identifier, "stub");
    }

    #[tokio::test]
    async fn read_signals_after_first_turn_has_intent_only() {
        // After exactly one respond_to_streaming, intent is current
        // (decided in-turn) but the classifier / extractor /
        // comprehension tasks for that turn haven't been drained —
        // that drain happens at the TOP of turn 2's respond_to_streaming.
        // This is the lag boundary the UI banner promises; pin it.
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();
        let dm_arc = Arc::clone(&active.dialogue_manager);

        run_turn(&dm_arc, "hello", |_, _| {}).await.unwrap();

        let dm = dm_arc.lock().await;
        let signals = read_signals(&dm);
        assert!(
            signals.intent.is_some(),
            "intent is decided in-turn — populated after first respond_to_streaming"
        );
        assert!(
            signals.engagement.is_none(),
            "engagement task is still pending — drain happens at top of turn 2"
        );
        assert!(
            signals.concepts.child.is_empty() && signals.concepts.primer.is_empty(),
            "extractor task is still pending — drain happens at top of turn 2"
        );
        assert!(
            signals.comprehension.is_empty(),
            "comprehension task is still pending — drain happens at top of turn 2"
        );
    }

    #[tokio::test]
    async fn read_signals_populates_after_second_turn() {
        // The DM drains the previous turn's background tasks at the
        // TOP of the next respond_to_streaming. So after turn 2,
        // last_* reflects turn 1's analysis — a populated path the
        // stub classifier/extractor/comprehension all exercise.
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();
        let dm_arc = Arc::clone(&active.dialogue_manager);

        run_turn(&dm_arc, "hello", |_, _| {}).await.unwrap();
        run_turn(&dm_arc, "tell me more", |_, _| {}).await.unwrap();

        let dm = dm_arc.lock().await;
        let signals = read_signals(&dm);
        // Intent is current (decided during turn 2); always populated
        // after at least one respond_to_streaming has run.
        let intent = signals
            .intent
            .as_deref()
            .expect("intent is current — set during turn 2");
        // Stable wire format from PedagogicalIntent::name() — must
        // match one of the canonical variant names. If this assertion
        // ever fires, either a variant was added/renamed in primer-core
        // (update the list below + the CSS) or somebody put the
        // `format!("{:?}", ...)` back. Both need to be caught.
        assert!(
            PedagogicalIntent::ALL.iter().any(|v| v.name() == intent),
            "intent {intent:?} must be a canonical PedagogicalIntent::name()"
        );
        // Stub classifier produces a deterministic Engaged-default
        // assessment — populated after the turn-1 task drain at top
        // of turn 2.
        let eng = signals
            .engagement
            .expect("engagement populated after second turn drains turn-1 classifier task");
        assert!(
            EngagementState::ALL.iter().any(|v| v.name() == eng.state),
            "engagement state {:?} must be a canonical EngagementState::name()",
            eng.state
        );
        assert!(
            (0.0..=1.0).contains(&eng.confidence),
            "confidence in valid range"
        );
    }

    /// Pre-turn `current_session_info` (via `info_from`) returns the
    /// initial snapshot (no `session_id` yet) without ever touching
    /// the DM mutex; after `send_message`-style snapshot refresh, the
    /// session_id and concept count appear.
    #[tokio::test]
    async fn snapshot_decouples_info_from_dm_lock() {
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();

        let before = info_from(&active).await;
        assert!(
            before.session_id.is_none(),
            "pre-turn snapshot has no session_id"
        );
        assert_eq!(before.learner.name, cfg.learner.name);
        assert_eq!(before.learner.age, cfg.learner.age);

        // Hold the DM lock for the whole duration of the snapshot
        // refresh + reader call — if `info_from` were still touching
        // the DM mutex, the second `info_from` below would deadlock
        // here (current task holds DM lock, info_from would block
        // waiting for it). Reaching the `assert!` proves info_from
        // never blocks on the DM.
        let dm_arc = Arc::clone(&active.dialogue_manager);
        let _guard = dm_arc.lock().await;
        let during_stream = info_from(&active).await;
        assert_eq!(
            during_stream.learner.id, before.learner.id,
            "info_from returns while DM is locked elsewhere"
        );
        drop(_guard);

        let dm_arc = Arc::clone(&active.dialogue_manager);
        let _payload = run_turn(&dm_arc, "hello", |_, _| {}).await.unwrap();
        refresh_snapshot(&dm_arc, &active.snapshot).await;

        let after = info_from(&active).await;
        assert!(
            after.session_id.is_some(),
            "post-turn snapshot carries the session id"
        );
    }
}
