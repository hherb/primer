//! Session lifecycle commands: start / close / resume / list.
//!
//! `start_session` builds an [`ActiveSession`] (which carries a
//! long-lived [`DialogueManager`](primer_pedagogy::DialogueManager))
//! from the persisted `GuiConfig` and stores it in `AppState`.
//! `close_session` drops it, draining any in-flight background tasks
//! first via `dm.close_session()`. `resume_session` reloads a saved
//! session by UUID; `list_sessions` powers the launch picker.

use uuid::Uuid;

use primer_core::storage::SessionStore;

use crate::state::{ActiveSession, AppState};
use crate::types::{LearnerSummary, SessionInfo, SessionListingDto};
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
    super::refresh_snapshot(&active.dialogue_manager, &active.snapshot).await;

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
pub(crate) async fn prepare_for_session_change(state: &AppState) -> Result<(), String> {
    #[cfg(feature = "speech")]
    crate::commands::voice::stop_voice_mode_inner(state, true)
        .await
        .ok();
    close_session_inner(state).await
}

/// Build a [`SessionInfo`] purely from the [`ActiveSession`]'s snapshot.
///
/// Reads ONLY from the snapshot — never touches the DM mutex — so a
/// sidebar refresh during a streaming turn returns immediately instead
/// of queueing behind the entire response wallclock.
pub(crate) async fn info_from(active: &ActiveSession) -> SessionInfo {
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
