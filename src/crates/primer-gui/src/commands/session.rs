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

use chrono::Utc;
use primer_classifier::consts::DEFAULT_HISTORY_DEPTH;
use primer_core::learner::UnderstandingDepth;
use primer_core::vocab::due_concepts;

use crate::state::{ActiveSession, AppState, SessionSnapshot};
use crate::types::{
    ChunkEvent, ComprehensionSummary, ConceptBreakdown, DepthCount, DueConcept, EngagementSummary,
    LearnerProfileView, LearnerSnapshot, LearnerSummary, SessionFullTurn, SessionInfo,
    SessionListingDto, SessionTurnSummary, TurnComplete, TurnSignals,
};
use crate::wiring;

/// Maximum characters of turn text the sidebar's Session list shows
/// inline. Chosen so a single row at the default sidebar width
/// doesn't wrap — the full text is in the chat bubble and on disk.
const TURN_TEXT_PREVIEW_CHARS: usize = 80;

/// Construct an `ActiveSession` from the persisted settings and store
/// it in `AppState`. Errors surface as `String` for inline rendering.
///
/// If a session is already open, it is closed first (no resource leak
/// even if the frontend forgets to close before re-starting). The
/// previous learner's state is saved as part of `close_session`'s
/// internal drain.
#[tauri::command]
pub async fn start_session(state: tauri::State<'_, AppState>) -> Result<SessionInfo, String> {
    prepare_for_session_change(&state).await?;

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

/// Resume a previously-saved session by UUID, replacing any active one.
///
/// Drops any current session (drains its background tasks first), then
/// probes the session's persisted locale, builds a fresh
/// `ActiveSession` using THAT locale (not the GUI's current cfg), loads
/// the named session from disk, and applies it via
/// `DialogueManager::resume_session` — which refreshes the rolling
/// summary if it's stale and rehydrates the classifier trajectory.
///
/// **Locale inheritance.** The persisted learner row carries the locale
/// every prior turn was tagged under. The GUI's current `cfg.learner.locale`
/// is meant for NEW sessions only — using it for a resume would let
/// new concepts extracted in the resumed session land with the wrong
/// `concept_language_tag` and silently corrupt the longitudinal record.
/// So resume_session inherits the session's locale and ignores cfg's
/// for THIS run. The persisted cfg on disk stays untouched; future
/// `start_session` calls still use cfg's locale.
///
/// This differs from the CLI, which errors on locale mismatch and asks
/// the user to drop `--language` or specify the saved one. The CLI has
/// an explicit `--language` flag the user typed; the GUI has neither
/// flag nor mechanism to "drop" anything, so auto-detect is the only
/// non-hostile behaviour.
///
/// Errors:
/// - `session_id` not a valid UUID → inline error.
/// - No session with that id on disk → "session not found" error.
/// - Construction failure (embedder, model resolution, etc.) → wiring-level error.
#[tauri::command]
pub async fn resume_session(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> Result<SessionInfo, String> {
    let uuid = Uuid::parse_str(&session_id)
        .map_err(|e| format!("invalid session id {session_id:?}: {e}"))?;

    prepare_for_session_change(&state).await?;

    let cfg = state.config.lock().await.clone();

    // Issue #86: `build_active_session_for_resume` opens the session DB
    // exactly once, reads the persisted learner inline, and silently
    // inherits the persisted locale on mismatch (the cfg value reflects
    // what the picker would pass for a NEW session, not what this
    // resumed session was originally tagged under). The pre-#86 path
    // opened the DB twice — once for a `probe_learner_locale` helper
    // and again for `build_active_session`.
    let active = wiring::build_active_session_for_resume(&state.home, &cfg).await?;

    let loaded = active
        .session_store
        .load_session(uuid)
        .await
        .map_err(|e| format!("load_session failed: {e}"))?
        .ok_or_else(|| format!("no session found with id {uuid}"))?;

    // Replace DM's freshly-minted session with the loaded one. After
    // this returns, dm.session.id == uuid and recent_assessments are
    // hydrated for the just-resumed session.
    active
        .dialogue_manager
        .lock()
        .await
        .resume_session(loaded)
        .await
        .map_err(|e| format!("resume_session failed: {e}"))?;

    // Snapshot was built with session_id = None at construction.
    // Refresh it now so current_session_info reports the resumed id.
    refresh_snapshot(&active.dialogue_manager, &active.snapshot).await;

    let info = info_from(&active).await;
    *state.session.lock().await = Some(active);
    Ok(info)
}

/// List every persisted session for the picker view.
///
/// Opens a transient `SqliteSessionStore` against the configured
/// session-DB path (or the per-learner default) and runs
/// `list_sessions`. Returns an empty Vec when:
/// - `persistence.no_persist == true` (no on-disk store exists)
/// - the resolved DB file doesn't exist yet (fresh install, never
///   started a session)
///
/// Doesn't reuse a running session's store: list_sessions is invoked
/// from the launch picker, before any session is active. Opening a
/// fresh connection per call is fine — SQLite read-only opens are
/// microseconds, and the picker is a once-per-launch surface.
#[tauri::command]
pub async fn list_sessions(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<SessionListingDto>, String> {
    use primer_core::i18n::Locale;
    use primer_core::storage::SessionStore;

    let cfg = state.config.lock().await.clone();
    if cfg.persistence.no_persist {
        return Ok(Vec::new());
    }
    let session_path = primer_engine::resolve_session_db_path(
        cfg.persistence.session_db.clone(),
        &state.home,
        &cfg.learner.name,
        cfg.persistence.no_persist,
    );
    // Fresh install / never-saved-yet: nothing to list. Don't create
    // the file on a read.
    if !session_path.exists() {
        return Ok(Vec::new());
    }
    let locale = Locale::from_pack_id(&cfg.learner.locale).unwrap_or_default();
    let store = primer_storage::SqliteSessionStore::open_for_locale(&session_path, locale)
        .map_err(|e| format!("opening session-db {}: {e}", session_path.display()))?;
    let listings = store
        .list_sessions()
        .await
        .map_err(|e| format!("list_sessions failed: {e}"))?;
    Ok(listings
        .into_iter()
        .map(|l| SessionListingDto {
            session_id: l.id,
            learner_id: l.learner_id,
            started_at: l.started_at.to_rfc3339(),
            ended_at: l.ended_at.map(|t| t.to_rfc3339()),
            last_activity: l.last_activity.to_rfc3339(),
            turn_count: l.turn_count,
            summary: l.summary,
        })
        .collect())
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

/// Default ceiling on the vocab-due list returned to the sidebar.
/// The CLI's `--vocab-max-per-prompt` setting is for prompt injection
/// (a much smaller cap, ~4); the sidebar's "review queue" is allowed
/// to be longer because it's just a display affordance. Tuned for
/// "see at a glance" without scrolling.
const VOCAB_DUE_DISPLAY_LIMIT: usize = 8;

/// Maximum number of recent engagement states the sidebar shows as a
/// sparkline-style dot strip. The in-memory `recent_assessments` Vec
/// is trimmed to `ClassifierSettings::history_depth` on every push
/// (see `dialogue_manager::apply::apply_assessment`), so the dot strip
/// can never exceed that bound today. We pin the display cap to the
/// same default so the named limit reflects what's actually rendered;
/// a future change that hydrates from `turn_classifications` (the
/// persisted longitudinal record) would let this bound grow
/// independently.
const RECENT_ENGAGEMENT_DISPLAY_LIMIT: usize = DEFAULT_HISTORY_DEPTH;

/// Return the longitudinal learner snapshot — profile + vocab-due list
/// + depth distribution + recent engagement strip. Same DM-lock-once
/// pattern as `get_turn_signals` so the sidebar can refresh both
/// sections from the same trigger without contending.
///
/// Returns `Ok(None)` when no session is active.
#[tauri::command]
pub async fn get_learner_state(
    state: tauri::State<'_, AppState>,
) -> Result<Option<LearnerSnapshot>, String> {
    let session_guard = state.session.lock().await;
    let active = match session_guard.as_ref() {
        Some(a) => a,
        None => return Ok(None),
    };
    let dm_arc = Arc::clone(&active.dialogue_manager);
    drop(session_guard);

    let dm = dm_arc.lock().await;
    Ok(Some(read_learner(&dm)))
}

/// Return every turn in the active session with FULL text — for the
/// chat-replay path after `resume_session` populates DM with a loaded
/// `Session`. Distinct from `list_session_turns` (truncated, for the
/// sidebar) so the sidebar's per-turn-complete refresh doesn't ship
/// every full-text turn across IPC on every update.
///
/// Returns `Ok(None)` when no session is active.
#[tauri::command]
pub async fn get_full_session_turns(
    state: tauri::State<'_, AppState>,
) -> Result<Option<Vec<SessionFullTurn>>, String> {
    let session_guard = state.session.lock().await;
    let active = match session_guard.as_ref() {
        Some(a) => a,
        None => return Ok(None),
    };
    let dm_arc = Arc::clone(&active.dialogue_manager);
    drop(session_guard);

    let dm = dm_arc.lock().await;
    let turns = dm
        .session
        .turns
        .iter()
        .enumerate()
        .map(|(i, t)| SessionFullTurn {
            index: i,
            speaker: speaker_name(t.speaker).to_string(),
            text: t.text.clone(),
        })
        .collect();
    Ok(Some(turns))
}

/// Return the turn-by-turn list that the sidebar's "Session" section
/// renders. Reads from the in-memory `dm.session.turns` — same source
/// the chat bubbles render from — so the list is always consistent
/// with what's on screen without round-tripping through the DB.
///
/// One DM-mutex lock per refresh, same pattern as the other sidebar
/// readers. Returns `Ok(None)` when no session is active.
#[tauri::command]
pub async fn list_session_turns(
    state: tauri::State<'_, AppState>,
) -> Result<Option<Vec<SessionTurnSummary>>, String> {
    let session_guard = state.session.lock().await;
    let active = match session_guard.as_ref() {
        Some(a) => a,
        None => return Ok(None),
    };
    let dm_arc = Arc::clone(&active.dialogue_manager);
    drop(session_guard);

    let dm = dm_arc.lock().await;
    Ok(Some(read_turn_list(&dm)))
}

/// Pure shape mapping from a held DM into the sidebar turn list.
/// Split out so a unit test can call it without a Tauri state.
/// No `.await` — caller must already hold the DM lock.
pub(crate) fn read_turn_list(dm: &DialogueManager) -> Vec<SessionTurnSummary> {
    dm.session
        .turns
        .iter()
        .enumerate()
        .map(|(i, turn)| {
            let (text_preview, truncated) =
                truncate_to_preview(&turn.text, TURN_TEXT_PREVIEW_CHARS);
            SessionTurnSummary {
                index: i,
                speaker: speaker_name(turn.speaker).to_string(),
                text_preview,
                truncated,
                intent: turn.intent.map(|i| i.name().to_string()),
                concepts: turn.concepts.clone(),
                timestamp: turn.timestamp.to_rfc3339(),
            }
        })
        .collect()
}

/// Canonical lowercase name for a `Speaker`. Mirrors the
/// `EngagementState::name()` / `PedagogicalIntent::name()` convention
/// used elsewhere in this crate. The returned string flows out to the
/// frontend via [`SessionTurnSummary::speaker`] and is consumed as a
/// `[data-speaker=…]` selector hook; do not rename.
///
/// `Speaker` itself has no `name()` method (only `ALL`) so this helper
/// lives here rather than on the core enum.
pub(crate) fn speaker_name(s: primer_core::conversation::Speaker) -> &'static str {
    match s {
        primer_core::conversation::Speaker::Child => "child",
        primer_core::conversation::Speaker::Primer => "primer",
    }
}

/// Truncate `s` to at most `max_chars` *characters* (not bytes — never
/// splits a multibyte codepoint). Adds an ellipsis when truncated.
/// Returns `(preview, truncated)`.
fn truncate_to_preview(s: &str, max_chars: usize) -> (String, bool) {
    let count = s.chars().count();
    if count <= max_chars {
        (s.trim().to_string(), false)
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        (format!("{}…", truncated.trim_end()), true)
    }
}

/// Pure shape mapping from a held DM into a [`LearnerSnapshot`].
/// Split out so a unit test can call it without round-tripping
/// through a Tauri state. No `.await` — caller must already hold the
/// DM lock.
pub(crate) fn read_learner(dm: &DialogueManager) -> LearnerSnapshot {
    let learner = &dm.learner;
    let now = Utc::now();

    let vocab_due = due_concepts(learner, now, VOCAB_DUE_DISPLAY_LIMIT)
        .into_iter()
        .map(|c| DueConcept {
            concept_id: c.concept_id.clone(),
            box_level: c.box_level,
            depth: c.depth.name().to_string(),
            days_until_due: days_until_due(c, now),
        })
        .collect();

    let mut depth_counts: Vec<DepthCount> = UnderstandingDepth::ALL
        .iter()
        .map(|d| DepthCount {
            depth: d.name().to_string(),
            count: 0,
        })
        .collect();
    for concept in &learner.concepts {
        if let Some(row) = depth_counts
            .iter_mut()
            .find(|r| r.depth == concept.depth.name())
        {
            row.count += 1;
        }
    }

    // recent_assessments is push-back/remove-front so it's already
    // oldest-first; take the tail slice for the dot strip.
    let start = learner
        .recent_assessments
        .len()
        .saturating_sub(RECENT_ENGAGEMENT_DISPLAY_LIMIT);
    let recent_engagement = learner.recent_assessments[start..]
        .iter()
        .map(|a| a.state.name().to_string())
        .collect();

    LearnerSnapshot {
        profile: LearnerProfileView {
            id: learner.profile.id,
            name: learner.profile.name.clone(),
            age: learner.profile.age,
            locale: learner.profile.locale.pack_id().to_string(),
        },
        vocab_due,
        depth_distribution: depth_counts,
        recent_engagement,
        concept_count: learner.concepts.len(),
    }
}

/// Days until a concept's next due date. Negative = already overdue.
/// `chrono::Duration::num_days` truncates toward zero, so sub-day
/// remainders on both sides round to 0 — "0.4 days" reads as "due
/// now" rather than "due tomorrow", and "-0.4 days" reads as "due
/// now" rather than "1 day late". That's the rendering we want from
/// a "next review" timer; the asymmetric-overdue side is the
/// deliberate forgiving choice over a true floor.
fn days_until_due(c: &primer_core::learner::ConceptState, now: chrono::DateTime<Utc>) -> i64 {
    use primer_core::consts::vocab::BOX_INTERVALS_DAYS;
    let Some(last) = c.last_encountered else {
        return 0;
    };
    let box_idx = (c.box_level as usize).min(BOX_INTERVALS_DAYS.len() - 1);
    let interval = chrono::Duration::days(BOX_INTERVALS_DAYS[box_idx] as i64);
    let due_at = last + interval;
    (due_at - now).num_days()
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
///
/// Also called by `commands::voice::start_voice_mode` so that switching
/// to voice mode cleanly drains any active text session first.
///
/// Takes `&AppState` rather than `&tauri::State<…>` so the unit tests
/// for `prepare_for_session_change` can drive it without a Tauri runtime.
/// Deref coercion lets `tauri::State<'_, AppState>` callers continue to
/// pass `&state`.
pub(crate) async fn close_session_inner(state: &AppState) -> Result<(), String> {
    let active = state.session.lock().await.take();
    if let Some(active) = active {
        let mut dm = active.dialogue_manager.lock().await;
        dm.close_session().await;
    }
    Ok(())
}

/// Tear down both the active voice loop (if any) and the active text
/// session (if any) before switching to a new session. On non-speech
/// builds the voice teardown is a compile-time no-op (the
/// `#[cfg(feature = "speech")]`-guarded call below disappears entirely),
/// so this collapses to just `close_session_inner` and remains correct.
///
/// Order matters: `voice::stop_voice_mode_inner` ALSO drops
/// `state.session` (because the voice loop's responder owns the same
/// DM Arc the GUI session held). Calling it first means
/// `close_session_inner` becomes a no-op when voice mode was active,
/// and the reverse when only a text session was open. This restores
/// the invariant that `start_session` / `resume_session` always
/// rebuild backends — including the locale-bound voice ones — from
/// the new config (closes #102).
///
/// Without this teardown, a session switch from `de` → `en` would
/// leave the voice loop running with its original German-locale
/// Whisper + Piper backends until the GUI was fully restarted.
///
/// **Sticky-toggle preservation.** Passes `preserve_toggle = true` so
/// `speech.voice_mode_enabled` stays at its current value across the
/// teardown. The frontend reads the still-`true` flag after
/// `start_session` / `resume_session` returns and auto-invokes
/// `start_voice_mode` against the new locale — the user sees voice
/// mode flow seamlessly into the new session instead of needing to
/// re-toggle it. (Without this, every session switch silently flipped
/// voice mode off and required a manual re-enable.)
async fn prepare_for_session_change(state: &AppState) -> Result<(), String> {
    #[cfg(feature = "speech")]
    super::voice::stop_voice_mode_inner(state, true).await.ok();
    close_session_inner(state).await
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
async fn cancel_active_turn(active: &ActiveSession) {
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
        voice_mode_available: cfg!(feature = "speech"),
    }
}

#[cfg(test)]
mod tests;
