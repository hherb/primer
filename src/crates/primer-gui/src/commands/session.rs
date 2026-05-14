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

    close_session_inner(&state).await?;

    let mut cfg = state.config.lock().await.clone();

    // Probe the session DB to read the learner row's persisted locale
    // BEFORE building the full active session. If it differs from cfg's,
    // override cfg.learner.locale for THIS resume so the KB + session
    // store + prompt pack all line up with the saved data. The user's
    // persisted gui-config.json is untouched.
    if let Some(session_locale) = probe_learner_locale(&state.home, &cfg).await? {
        if session_locale.pack_id() != cfg.learner.locale {
            tracing::info!(
                target: "primer_gui::resume",
                cfg_locale = %cfg.learner.locale,
                session_locale = session_locale.pack_id(),
                "resume: inheriting session's saved locale (differs from cfg)"
            );
            cfg.learner.locale = session_locale.pack_id().to_string();
        }
    }

    let active = wiring::build_active_session(&state.home, &cfg).await?;

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
pub(crate) async fn close_session_inner(state: &tauri::State<'_, AppState>) -> Result<(), String> {
    let active = state.session.lock().await.take();
    if let Some(active) = active {
        let mut dm = active.dialogue_manager.lock().await;
        dm.close_session().await;
    }
    Ok(())
}

/// Read the persisted learner's locale from disk without constructing
/// a full active session. Returns `Ok(None)` when there's no on-disk
/// session DB to probe (fresh install, or `no_persist` is on) — both
/// cases mean cfg's locale wins by default. Errors bubble up as
/// user-facing strings so they can land in the resume failure banner.
///
/// The store is opened with `Locale::default()` because the probe is a
/// pure read: `LearnerStore::load_learner` doesn't touch the locale
/// field at all (it returns whatever's stored in `learners.locale`).
/// Using a deterministic locale here also keeps `--reembed`-style
/// concept-tag writes that *do* depend on the store's locale out of
/// this read-only code path.
async fn probe_learner_locale(
    home: &std::path::Path,
    cfg: &crate::config::GuiConfig,
) -> Result<Option<primer_core::i18n::Locale>, String> {
    use primer_core::storage::LearnerStore;

    if cfg.persistence.no_persist {
        return Ok(None);
    }
    let session_path = primer_engine::resolve_session_db_path(
        cfg.persistence.session_db.clone(),
        home,
        &cfg.learner.name,
        false,
    );
    if !session_path.exists() {
        return Ok(None);
    }
    let store = primer_storage::SqliteSessionStore::open_for_locale(
        &session_path,
        primer_core::i18n::Locale::default(),
    )
    .map_err(|e| {
        format!(
            "probe: opening session-db {} for locale read: {e}",
            session_path.display()
        )
    })?;
    let learner = store
        .load_learner()
        .await
        .map_err(|e| format!("probe: load_learner: {e}"))?;
    Ok(learner.map(|l| l.profile.locale))
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
    async fn resume_path_swaps_dm_session_to_loaded_one() {
        // Models the resume_session command: build active, run a turn
        // to land a session row, drop, build a second active (which
        // mints a fresh session), then load + resume to swap DM's
        // session in place. End state: dm.session.id matches the
        // originally-persisted id.
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());

        // First active: run one turn so a session row exists on disk.
        let first_active = build_active_session(home.path(), &cfg).await.unwrap();
        let first_dm = Arc::clone(&first_active.dialogue_manager);
        let payload = run_turn(&first_dm, "hello", |_, _| {}).await.unwrap();
        let original_id = payload.session_id;
        // Drain background tasks before drop so the row is committed.
        first_dm.lock().await.close_session().await;
        drop(first_active);

        // Second active: brand-new DM, brand-new minted session id.
        let second_active = build_active_session(home.path(), &cfg).await.unwrap();
        let fresh_id_before_resume = second_active.dialogue_manager.lock().await.session.id;
        assert_ne!(
            fresh_id_before_resume, original_id,
            "fresh build mints a fresh session id"
        );

        // Resume: load the original session via the stored Arc, then
        // swap it in via DM::resume_session. After this the DM is
        // logically continuing the persisted conversation.
        let loaded = second_active
            .session_store
            .load_session(original_id)
            .await
            .unwrap()
            .expect("loaded session must exist on disk");
        second_active
            .dialogue_manager
            .lock()
            .await
            .resume_session(loaded)
            .await
            .unwrap();

        let after = second_active.dialogue_manager.lock().await.session.id;
        assert_eq!(
            after, original_id,
            "after resume_session, dm.session.id matches the loaded one"
        );

        // And the loaded session carries the persisted turn count.
        assert_eq!(
            second_active
                .dialogue_manager
                .lock()
                .await
                .session
                .turns
                .len(),
            2,
            "resumed session carries both turns of the original exchange"
        );
    }

    #[tokio::test]
    async fn list_sessions_via_store_after_one_turn() {
        // Builds a session through wiring, runs a turn (the only way to
        // land a sessions row through DM), then uses the same store Arc
        // ActiveSession exposes to read the listing back. Validates the
        // wiring contract — list_sessions sees what send_message wrote
        // — without needing a Tauri state injection harness.

        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();
        let dm_arc = Arc::clone(&active.dialogue_manager);
        let store = Arc::clone(&active.session_store);

        let payload = run_turn(&dm_arc, "what is curiosity?", |_, _| {})
            .await
            .unwrap();
        dm_arc.lock().await.close_session().await;

        let listings = store.list_sessions().await.unwrap();
        assert_eq!(listings.len(), 1, "exactly one persisted session");
        assert_eq!(listings[0].id, payload.session_id);
        assert_eq!(
            listings[0].turn_count, 2,
            "child + primer turns both counted"
        );
    }

    #[test]
    fn resume_rejects_invalid_uuid_inline() {
        // The first thing resume_session does is parse the session_id
        // string into a Uuid; an invalid id must produce a helpful
        // error string the picker can render rather than panicking.
        let err = Uuid::parse_str("not-a-uuid")
            .map_err(|e| format!("invalid session id {:?}: {e}", "not-a-uuid"))
            .unwrap_err();
        assert!(
            err.contains("invalid session id"),
            "user-facing prefix preserved: {err}"
        );
        assert!(
            err.contains("\"not-a-uuid\""),
            "echoes the bad input verbatim so the user can spot the typo: {err}"
        );
    }

    #[tokio::test]
    async fn resume_returns_not_found_for_unknown_uuid() {
        // Mirrors the "no session found" branch in resume_session: a
        // syntactically-valid UUID that no session row backs must
        // produce an Ok(None) at the store layer, which the command
        // turns into a user-facing error string.
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();

        let random_id = Uuid::new_v4();
        let loaded = active.session_store.load_session(random_id).await.unwrap();
        assert!(
            loaded.is_none(),
            "load_session on a never-persisted id yields None"
        );

        // Emulate the command's `.ok_or_else(...)` mapping so the test
        // pins the actual user-facing error shape.
        let err: String = loaded
            .map(|_| String::new())
            .ok_or_else(|| format!("no session found with id {random_id}"))
            .unwrap_err();
        assert!(err.starts_with("no session found with id "));
        assert!(err.contains(&random_id.to_string()));
    }

    #[tokio::test]
    async fn probe_learner_locale_reads_stored_locale() {
        // Resume-on-mismatch behaviour: instead of erroring like the CLI,
        // the GUI probes the persisted learner's locale on disk and uses
        // THAT for the resume regardless of cfg.learner.locale. This test
        // exercises the probe helper directly — same pattern the Tauri
        // command's resume path uses. The companion concept_language_tag
        // invariant is preserved because the rebuilt ActiveSession opens
        // the store under the probed (saved) locale, so any newly
        // extracted concepts in the resumed session land with the
        // matching tag.
        let home = TempDir::new().unwrap();

        // Step 1: build + save under English so the learner row lands
        // with locale=en.
        let cfg_en = stub_config_with_persistence(home.path());
        let active_en = build_active_session(home.path(), &cfg_en).await.unwrap();
        let dm_en = Arc::clone(&active_en.dialogue_manager);
        run_turn(&dm_en, "hello", |_, _| {}).await.unwrap();
        dm_en.lock().await.close_session().await;
        drop(active_en);

        // Step 2: probe with a cfg that asks for German. The probe
        // should return English (the stored locale), not German (cfg's
        // request).
        let mut cfg_de = stub_config_with_persistence(home.path());
        cfg_de.learner.locale = "de".to_string();
        let probed = super::probe_learner_locale(home.path(), &cfg_de)
            .await
            .unwrap();
        assert_eq!(
            probed,
            Some(primer_core::i18n::Locale::English),
            "probe returns the persisted locale, not cfg's"
        );

        // Step 3: a probe on a fresh home with a fresh cfg (no session
        // DB yet) returns None so the resume path falls through to
        // cfg's locale.
        let fresh = TempDir::new().unwrap();
        let cfg_fresh = stub_config_with_persistence(fresh.path());
        let probed_fresh = super::probe_learner_locale(fresh.path(), &cfg_fresh)
            .await
            .unwrap();
        assert!(
            probed_fresh.is_none(),
            "no session DB → probe returns None: got {probed_fresh:?}"
        );
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

    #[test]
    fn truncate_short_text_passes_through() {
        let (preview, truncated) = truncate_to_preview("hello", 80);
        assert_eq!(preview, "hello");
        assert!(!truncated);
    }

    #[test]
    fn truncate_long_text_adds_ellipsis() {
        let s = "a".repeat(200);
        let (preview, truncated) = truncate_to_preview(&s, 80);
        assert!(truncated);
        assert!(preview.ends_with('…'));
        // 80 a's + ellipsis = 81 chars
        assert_eq!(preview.chars().count(), 81);
    }

    #[test]
    fn truncate_respects_codepoint_boundaries() {
        // A run of multibyte characters; max_chars is a *char* limit,
        // so we must not split a codepoint.
        let s = "🌟".repeat(10); // 10 chars, 40 bytes
        let (preview, truncated) = truncate_to_preview(&s, 5);
        assert!(truncated);
        // 5 stars + ellipsis = 6 chars
        assert_eq!(preview.chars().count(), 6);
        assert!(preview.starts_with("🌟🌟🌟🌟🌟"));
    }

    #[tokio::test]
    async fn read_turn_list_empty_for_fresh_session() {
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();
        let dm = active.dialogue_manager.lock().await;
        let list = read_turn_list(&dm);
        assert!(list.is_empty(), "no turns before first send_message");
    }

    #[tokio::test]
    async fn read_turn_list_after_one_exchange() {
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();
        let dm_arc = Arc::clone(&active.dialogue_manager);

        run_turn(&dm_arc, "hello", |_, _| {}).await.unwrap();

        let dm = dm_arc.lock().await;
        let list = read_turn_list(&dm);
        assert_eq!(list.len(), 2, "one exchange = child + primer turns");
        assert_eq!(list[0].index, 0);
        assert_eq!(list[0].speaker, "child");
        assert_eq!(list[0].text_preview, "hello");
        assert!(!list[0].truncated);
        assert!(list[0].intent.is_none(), "child turns have no intent");

        assert_eq!(list[1].index, 1);
        assert_eq!(list[1].speaker, "primer");
        assert!(
            list[1].intent.is_some(),
            "primer turn carries the decided intent"
        );
    }

    #[tokio::test]
    async fn read_learner_fresh_session_shape() {
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();
        let dm = active.dialogue_manager.lock().await;
        let snap = read_learner(&dm);

        assert_eq!(snap.profile.name, cfg.learner.name);
        assert_eq!(snap.profile.age, cfg.learner.age);
        assert_eq!(snap.profile.locale, cfg.learner.locale);
        assert_eq!(snap.concept_count, 0);
        assert!(snap.vocab_due.is_empty());
        assert!(snap.recent_engagement.is_empty());
        // Distribution is always six entries — depths the learner
        // has never reached carry count=0. Canonical order matches
        // UnderstandingDepth::ALL.
        let names: Vec<&str> = snap
            .depth_distribution
            .iter()
            .map(|r| r.depth.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "Unknown",
                "Aware",
                "Recall",
                "Comprehension",
                "Application",
                "Analysis"
            ]
        );
        for row in &snap.depth_distribution {
            assert_eq!(
                row.count, 0,
                "fresh learner has no concepts at {}",
                row.depth
            );
        }
    }

    #[tokio::test]
    async fn read_learner_counts_concepts_by_depth() {
        use primer_core::learner::{ConceptState, UnderstandingDepth};
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();
        {
            let mut dm = active.dialogue_manager.lock().await;
            // Inject concepts directly into the in-memory learner —
            // the extractor stub returns empty, so this is the only
            // way to exercise the populated counting path in a unit test.
            dm.learner.concepts.push(ConceptState {
                concept_id: "physics:gravity".into(),
                depth: UnderstandingDepth::Aware,
                confidence: 0.6,
                encounter_count: 1,
                last_encountered: Some(Utc::now()),
                notes: vec![],
                box_level: 0,
            });
            dm.learner.concepts.push(ConceptState {
                concept_id: "biology:photosynthesis".into(),
                depth: UnderstandingDepth::Recall,
                confidence: 0.8,
                encounter_count: 2,
                last_encountered: Some(Utc::now() - chrono::Duration::days(2)),
                notes: vec![],
                box_level: 0,
            });
            dm.learner.concepts.push(ConceptState {
                concept_id: "physics:mass".into(),
                depth: UnderstandingDepth::Aware,
                confidence: 0.5,
                encounter_count: 1,
                last_encountered: Some(Utc::now()),
                notes: vec![],
                box_level: 0,
            });
        }
        let dm = active.dialogue_manager.lock().await;
        let snap = read_learner(&dm);

        assert_eq!(snap.concept_count, 3);
        let by_depth: std::collections::HashMap<_, _> = snap
            .depth_distribution
            .iter()
            .map(|r| (r.depth.as_str(), r.count))
            .collect();
        assert_eq!(by_depth["Aware"], 2);
        assert_eq!(by_depth["Recall"], 1);
        assert_eq!(by_depth["Analysis"], 0);
        // Vocab due: photosynthesis is 2 days past its 1-day box-0
        // interval, so it lands in the due list. Mass and gravity
        // were "just encountered" so are not yet due.
        let due_ids: Vec<&str> = snap
            .vocab_due
            .iter()
            .map(|c| c.concept_id.as_str())
            .collect();
        assert_eq!(due_ids, vec!["biology:photosynthesis"]);
        assert!(snap.vocab_due[0].days_until_due <= 0, "must be overdue");
    }

    #[tokio::test]
    async fn read_learner_recent_engagement_oldest_first_and_clamped() {
        use primer_core::classifier::EngagementAssessment;
        use primer_core::learner::EngagementState;
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();
        // Inject more than DEFAULT_HISTORY_DEPTH assessments in
        // chronological order; the snapshot must preserve order
        // (oldest first) and clamp to the display limit. Pushed
        // directly because the in-memory cap from apply_assessment is
        // exactly what we want to exercise from the snapshot side.
        let states = [
            EngagementState::Disengaging,
            EngagementState::Reflecting,
            EngagementState::Engaged,
            EngagementState::FrustratedTrying,
            EngagementState::Engaged,
        ];
        {
            let mut dm = active.dialogue_manager.lock().await;
            for s in states {
                dm.learner.recent_assessments.push(EngagementAssessment {
                    state: s,
                    confidence: 0.8,
                    reasoning: None,
                });
            }
        }
        let dm = active.dialogue_manager.lock().await;
        let snap = read_learner(&dm);

        assert_eq!(
            snap.recent_engagement.len(),
            RECENT_ENGAGEMENT_DISPLAY_LIMIT,
            "clamped to the display limit when source exceeds it"
        );
        // Tail-slice preserves order — the displayed slice is the
        // most-recent N states in the same order they were appended.
        let tail_start = states.len() - RECENT_ENGAGEMENT_DISPLAY_LIMIT;
        let expected: Vec<String> = states[tail_start..]
            .iter()
            .map(|s| s.name().to_string())
            .collect();
        assert_eq!(snap.recent_engagement, expected);
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

    // ─── Cancel-mid-stream tests ──────────────────────────────────────

    /// Validates the contract `cancel_response` relies on:
    /// `JoinHandle::abort()` on a still-pending task results in a
    /// `JoinError::is_cancelled() == true` join result. This is a
    /// tokio invariant our cancel path is built on; the test exists
    /// to fail loudly if a future tokio bump changes the semantics
    /// (which would silently break our cancel path).
    #[tokio::test]
    async fn abort_handle_yields_cancelled_join_error() {
        let task = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            "unreachable"
        });
        let handle = task.abort_handle();
        // Yield once so the task enters its sleep.
        tokio::task::yield_now().await;
        handle.abort();
        let err = task.await.unwrap_err();
        assert!(
            err.is_cancelled(),
            "abort() should produce a cancelled JoinError, got {err:?}"
        );
    }

    /// `current_turn_abort` starts None and stays None after a normal
    /// turn — the send_message path's "clear-on-completion" step keeps
    /// a stale handle from sitting around between turns.
    #[tokio::test]
    async fn current_turn_abort_slot_lifecycle() {
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();

        assert!(
            active.current_turn_abort.lock().await.is_none(),
            "starts empty"
        );

        // Mirror the spawn + store + await + clear sequence from
        // send_message.
        let dm_arc = Arc::clone(&active.dialogue_manager);
        let task = tokio::spawn(async move { run_turn(&dm_arc, "hello", |_, _| {}).await });
        *active.current_turn_abort.lock().await = Some(task.abort_handle());

        let result = task.await.expect("task completes without panic");
        assert!(result.is_ok(), "stub turn succeeds");

        *active.current_turn_abort.lock().await = None;
        assert!(
            active.current_turn_abort.lock().await.is_none(),
            "cleared after completion"
        );
    }

    /// Calling the cancel sequence on a session with no in-flight turn
    /// is safe — the optional handle is None and the abort branch is
    /// skipped without panic. Mirrors what `cancel_response` does when
    /// the user clicks Cancel a moment after the response landed.
    #[tokio::test]
    async fn cancel_with_idle_session_is_noop() {
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();

        // Drive the production helper directly. With an empty slot
        // this must return cleanly without panicking.
        cancel_active_turn(&active).await;
        assert!(active.current_turn_abort.lock().await.is_none());
    }

    /// End-to-end smoke for the cancel path's effect on a live task:
    /// spawn a pending task, stash its abort handle, drive
    /// `cancel_active_turn`, and verify the join result reports
    /// cancellation. Pins the abort *wiring* (not just the slot
    /// mechanics) without needing a Tauri runtime in scope.
    #[tokio::test]
    async fn cancel_active_turn_aborts_pending_task() {
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();

        // A long sleep stands in for an in-flight respond_to_streaming.
        // What matters is that `.abort()` on the stashed handle drops
        // the future and yields a cancelled JoinError, which is the
        // exact contract `send_message`'s match arm relies on.
        let task = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        });
        *active.current_turn_abort.lock().await = Some(task.abort_handle());
        tokio::task::yield_now().await;

        cancel_active_turn(&active).await;

        let join_err = task.await.expect_err("task should be cancelled");
        assert!(
            join_err.is_cancelled(),
            "expected cancelled JoinError, got {join_err:?}"
        );
    }

    /// The frontend (`ui/app.js`) matches `CANCEL_SENTINEL` against
    /// this exact string to suppress the error banner on
    /// user-initiated cancel. A one-sided rename here without an
    /// equivalent change in `ui/app.js` would silently re-surface
    /// cancel messages as errors — pin the value so CI catches the
    /// drift the moment it lands.
    #[test]
    fn cancelled_message_is_stable_machine_token() {
        assert_eq!(CANCELLED_MESSAGE, "primer:turn_cancelled");
    }

    #[tokio::test]
    async fn session_info_carries_voice_mode_available_flag() {
        let home = TempDir::new().unwrap();
        let cfg = stub_config_with_persistence(home.path());
        let active = build_active_session(home.path(), &cfg).await.unwrap();
        let info = info_from(&active).await;
        // The flag matches whatever feature the test binary was built with.
        assert_eq!(info.voice_mode_available, cfg!(feature = "speech"));
    }
}
