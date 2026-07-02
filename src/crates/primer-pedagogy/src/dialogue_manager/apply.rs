//! Pure helper functions that mutate `LearnerModel` and `Session.turns`
//! based on classifier / extractor / comprehension outputs.
//!
//! These are deliberately free functions, not methods on `DialogueManager`,
//! because they have no dependency on the manager's borrowed inference /
//! knowledge backends. Keeping them pure makes the unit tests in this
//! file straightforward — each test constructs a `LearnerModel`, calls
//! the function, asserts the result.
//!
//! Visibility is `pub(super)` so the dialogue manager modules can call
//! them while staying private to the rest of the crate.

use chrono::Utc;
use primer_core::conversation::Turn;

/// Push an `EngagementAssessment` into the learner's history buffer and,
/// when confidence is high enough, update `current_engagement`.
///
/// History is a FIFO ring of depth `settings.history_depth`. Every
/// assessment — even low-confidence ones — is recorded so the trajectory
/// is visible to later logic. Only assessments that meet or exceed
/// `settings.confidence_threshold` update `current_engagement`; below
/// that threshold the field is left unchanged so a single noisy read
/// doesn't yank the intent-selection state.
pub(super) fn apply_assessment(
    learner: &mut primer_core::learner::LearnerModel,
    a: primer_core::classifier::EngagementAssessment,
    settings: &primer_classifier::ClassifierSettings,
) {
    learner.recent_assessments.push(a.clone());
    while learner.recent_assessments.len() > settings.history_depth {
        learner.recent_assessments.remove(0);
    }
    if a.confidence >= settings.confidence_threshold {
        learner.current_engagement = a.state;
    }
    // Low-confidence assessments are still recorded in history (signal for
    // trajectory) but current_engagement stays unchanged.
}

/// Merge a `ConceptExtraction` into the in-memory `LearnerModel.concepts`.
///
/// Adds new `ConceptState` rows (depth = `Aware`, confidence =
/// `consts::INITIAL_CONCEPT_CONFIDENCE`) for concepts not yet seen;
/// for concepts already in the learner model, increments
/// `encounter_count` and refreshes `last_encountered`. The updated
/// state is what `LearnerStore::save_learner` will persist on the
/// next save (idempotent upsert into `learner_concepts` — monotonic
/// across the child's lifetime).
///
/// Both `child_concepts` and `primer_concepts` feed into the same
/// `learner.concepts` store, but a concept appearing in BOTH lists
/// counts as a single encounter — one exchange in which this concept
/// was mentioned, regardless of which speaker(s) used it. Today the
/// model doesn't distinguish "a concept the child surfaced" from "a
/// concept the Primer introduced"; future work could add a per-side
/// `encounter_count_by_speaker`.
pub(super) fn apply_extraction(
    learner: &mut primer_core::learner::LearnerModel,
    extraction: &primer_core::extractor::ConceptExtraction,
) -> bool {
    use primer_core::learner::{ConceptState, UnderstandingDepth};
    use std::collections::HashSet;

    let now = Utc::now();
    let mut changed = false;

    // Dedupe across (child, primer) — one exchange = at most one
    // encounter per concept name, even if both speakers used it.
    let mut seen: HashSet<&str> = HashSet::new();
    let unique_names = extraction
        .child_concepts
        .iter()
        .chain(extraction.primer_concepts.iter())
        .filter(|name| seen.insert(name.as_str()));

    for name in unique_names {
        if let Some(existing) = learner.concepts.iter_mut().find(|c| c.concept_id == *name) {
            existing.encounter_count = existing.encounter_count.saturating_add(1);
            existing.last_encountered = Some(now);
            changed = true;
        } else {
            learner.concepts.push(ConceptState {
                concept_id: name.clone(),
                depth: UnderstandingDepth::Aware,
                confidence: crate::consts::INITIAL_CONCEPT_CONFIDENCE,
                encounter_count: 1,
                last_encountered: Some(now),
                notes: vec![],
                box_level: 0,
            });
            changed = true;
        }
    }
    changed
}

/// Apply a `ComprehensionResult` to the in-memory `LearnerModel`.
///
/// For each assessment whose `confidence >= settings.confidence_threshold`,
/// promote `learner.concepts[concept].depth` via monotonic max
/// (never demote — that's an explicit forgetting event handled
/// algorithmically over `turn_comprehensions`, not here).
///
/// Sub-threshold assessments are still persisted to disk by the
/// caller (full longitudinal record) but don't update in-memory state.
///
/// Concepts not already in `learner.concepts` are skipped — insertion
/// is the responsibility of `apply_extraction` (which always runs
/// before this function in the await sequence). The corner case of
/// an assessment for an unknown concept is tolerated (parser-layer
/// drops them) but documented here as defensive.
///
/// Returns `true` if any depth or confidence was updated; the caller
/// uses this to set `learner_dirty` so the per-turn save flushes.
pub(super) fn apply_comprehension(
    learner: &mut primer_core::learner::LearnerModel,
    result: &primer_core::comprehension::ComprehensionResult,
    settings: &primer_comprehension::ComprehensionSettings,
) -> bool {
    use primer_core::learner::UnderstandingDepth;

    let mut changed = false;
    for a in &result.assessments {
        if a.confidence < settings.confidence_threshold {
            continue;
        }
        // An `Unknown` assessment asserts NO evidence of understanding.
        // The parser tolerates the label defensively (the prompt no
        // longer offers it), but it must not touch learner state: the
        // monotonic max below already can't demote depth, and
        // `apply_box_transition` only special-cases `Aware` — without
        // this guard a high-confidence Unknown row would ADVANCE the
        // Leitner box (vocab.rs pins that contract), expanding the
        // review interval off a no-evidence reading.
        if a.depth == UnderstandingDepth::Unknown {
            continue;
        }
        if let Some(c) = learner
            .concepts
            .iter_mut()
            .find(|c| c.concept_id == a.concept)
        {
            if a.depth > c.depth {
                c.depth = a.depth;
                // Confidence reflects belief in the *current* depth label.
                // When depth promotes, adopt this turn's confidence rather
                // than max'ing with the prior — the prior measured belief
                // in a different (lower) depth and is no longer applicable.
                c.confidence = a.confidence;
                changed = true;
            }
            // Box transition runs on EVERY accepted assessment, regardless
            // of whether depth moved. Successful re-confirmation at the same
            // depth is the SR mechanism for expanding intervals — without
            // this, a concept stuck at Comprehension would never advance
            // even after years of confident re-engagement. The inner
            // `confidence < MIN_CONF_FOR_BOX_PROMOTION` reset branch in
            // `apply_box_transition` is redundant in the default config
            // (numerically equal to the outer `confidence_threshold` of
            // 0.6), but stays load-bearing if a future researcher lowers
            // `comprehension_settings.confidence_threshold` below 0.6 to
            // accept noisier comprehension signal — the const-keyed inner
            // guard then prevents weak signal from driving box promotion.
            // See docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md
            // ("Subtle but deliberate") for the policy split.
            let new_box =
                primer_core::vocab::apply_box_transition(c.box_level, a.depth, a.confidence);
            if new_box != c.box_level {
                c.box_level = new_box;
                changed = true;
            }
        }
    }
    changed
}

/// Append `new_concepts` to `turns[index].concepts`, preserving order
/// and skipping names already present. Used by `apply_post_response_outcome`
/// to keep the in-memory `Session.turns` in sync with what the spawned
/// extractor task wrote to disk via `update_exchange_concepts`. A
/// silently-out-of-bounds index is treated as a no-op since the
/// session is append-only and the index was captured at spawn time;
/// pruning would only happen after the session is reset, in which case
/// the in-memory update has no consumer anyway.
pub(super) fn merge_concepts_into_turn(turns: &mut [Turn], index: usize, new_concepts: &[String]) {
    let Some(turn) = turns.get_mut(index) else {
        return;
    };
    for name in new_concepts {
        if !turn.concepts.iter().any(|existing| existing == name) {
            turn.concepts.push(name.clone());
        }
    }
}

#[cfg(test)]
mod tests;
