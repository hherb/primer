//! Sidebar reader commands + the pure shape-mapping helpers they call.
//!
//! Every command here follows the same lock-once pattern: clone the DM
//! Arc out of the session guard, release the session guard, then lock
//! the DM briefly to read a snapshot. The pure `read_*` mappers are
//! split out so unit tests can call them against a held DM without a
//! Tauri state.

use std::sync::Arc;

use chrono::Utc;
use primer_classifier::consts::DEFAULT_HISTORY_DEPTH;
use primer_core::learner::UnderstandingDepth;
use primer_core::vocab::due_concepts;
use primer_pedagogy::DialogueManager;

use crate::state::AppState;
use crate::types::{
    ComprehensionSummary, ConceptBreakdown, DepthCount, DueConcept, EngagementSummary,
    LearnerProfileView, LearnerSnapshot, SessionFullTurn, SessionTurnSummary, TurnSignals,
};

/// Maximum characters of turn text the sidebar's Session list shows
/// inline. Chosen so a single row at the default sidebar width
/// doesn't wrap — the full text is in the chat bubble and on disk.
const TURN_TEXT_PREVIEW_CHARS: usize = 80;

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
pub(crate) const RECENT_ENGAGEMENT_DISPLAY_LIMIT: usize = DEFAULT_HISTORY_DEPTH;

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
pub(crate) fn truncate_to_preview(s: &str, max_chars: usize) -> (String, bool) {
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
