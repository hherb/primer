//! Spaced-repetition vocabulary scheduler.
//!
//! Three pure functions implementing the Leitner-box review schedule
//! defined in `docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md`.
//!
//! - [`apply_box_transition`] — decide the new box level given a comprehension
//!   assessment.
//! - [`is_due`] — true if a concept's review interval has elapsed.
//! - [`due_concepts`] — top-K most-overdue concepts, sorted by overdue-amount
//!   descending.
//!
//! All functions are pure: they take their wallclock dependency (`now`) as a
//! parameter so test code can drive them deterministically. The production
//! call site reads `chrono::Utc::now()` once per turn.

use crate::consts::vocab::{BOX_INTERVALS_DAYS, MAX_BOX_LEVEL, MIN_CONF_FOR_BOX_PROMOTION};
use crate::learner::{ConceptState, LearnerModel, UnderstandingDepth};

/// Box-level transition driven by a comprehension assessment.
///
/// Three-zone policy:
/// - `confidence < MIN_CONF_FOR_BOX_PROMOTION` → reset to 0.
/// - `depth = Aware` → reset to 0 (regardless of confidence).
/// - `depth ≥ Recall` AND `confidence ≥ MIN_CONF_FOR_BOX_PROMOTION` →
///   `current_box + 1` capped at `MAX_BOX_LEVEL`.
///
/// Pure — returns the new box level. The caller decides whether to write
/// it back to [`ConceptState`].
pub fn apply_box_transition(
    current_box: u8,
    depth: UnderstandingDepth,
    confidence: f32,
) -> u8 {
    if confidence < MIN_CONF_FOR_BOX_PROMOTION {
        return 0;
    }
    if depth == UnderstandingDepth::Aware {
        return 0;
    }
    (current_box + 1).min(MAX_BOX_LEVEL)
}

/// True if a concept is due for review now.
///
/// Returns false for concepts never encountered (`last_encountered = None`).
/// Returns true for concepts with `last_encountered` older than the box's
/// interval. Out-of-range `box_level` (defensive — should never happen)
/// clamps to `MAX_BOX_LEVEL`.
pub fn is_due(concept: &ConceptState, now: chrono::DateTime<chrono::Utc>) -> bool {
    let Some(last) = concept.last_encountered else {
        return false;
    };
    let box_idx = (concept.box_level as usize).min(BOX_INTERVALS_DAYS.len() - 1);
    let interval = chrono::Duration::days(BOX_INTERVALS_DAYS[box_idx] as i64);
    now - last > interval
}

/// Top `max_count` due concepts from the learner model, sorted by
/// overdue-ness descending (most overdue first).
///
/// Pure — borrows from the learner; returns at most `max_count` references.
/// Fewer if the learner has fewer due concepts. `max_count = 0` short-circuits
/// to an empty Vec.
pub fn due_concepts<'a>(
    learner: &'a LearnerModel,
    now: chrono::DateTime<chrono::Utc>,
    max_count: usize,
) -> Vec<&'a ConceptState> {
    if max_count == 0 {
        return Vec::new();
    }
    let mut due: Vec<&ConceptState> = learner
        .concepts
        .iter()
        .filter(|c| is_due(c, now))
        .collect();
    due.sort_by(|a, b| {
        let a_over = overdue_amount(a, now);
        let b_over = overdue_amount(b, now);
        b_over.cmp(&a_over)
    });
    due.truncate(max_count);
    due
}

/// How far past due a concept is. Zero for not-yet-due (callers should have
/// filtered already, but safe regardless). Out-of-range `box_level` clamps to
/// `MAX_BOX_LEVEL` mirroring [`is_due`].
fn overdue_amount(
    concept: &ConceptState,
    now: chrono::DateTime<chrono::Utc>,
) -> chrono::Duration {
    let Some(last) = concept.last_encountered else {
        return chrono::Duration::zero();
    };
    let box_idx = (concept.box_level as usize).min(BOX_INTERVALS_DAYS.len() - 1);
    let interval = chrono::Duration::days(BOX_INTERVALS_DAYS[box_idx] as i64);
    (now - last - interval).max(chrono::Duration::zero())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone, Utc};

    // ─── apply_box_transition ────────────────────────────────────────────

    #[test]
    fn promotes_from_zero_on_recall_with_strong_confidence() {
        assert_eq!(apply_box_transition(0, UnderstandingDepth::Recall, 0.7), 1);
    }

    #[test]
    fn promotes_from_zero_on_comprehension() {
        assert_eq!(
            apply_box_transition(0, UnderstandingDepth::Comprehension, 0.9),
            1
        );
    }

    #[test]
    fn promotes_mid_box_on_application() {
        assert_eq!(
            apply_box_transition(2, UnderstandingDepth::Application, 0.95),
            3
        );
    }

    #[test]
    fn caps_at_max_box_level() {
        assert_eq!(
            apply_box_transition(MAX_BOX_LEVEL, UnderstandingDepth::Analysis, 1.0),
            MAX_BOX_LEVEL
        );
    }

    #[test]
    fn resets_on_aware_regardless_of_confidence() {
        assert_eq!(apply_box_transition(3, UnderstandingDepth::Aware, 0.9), 0);
        assert_eq!(apply_box_transition(4, UnderstandingDepth::Aware, 1.0), 0);
    }

    #[test]
    fn resets_on_low_confidence() {
        assert_eq!(
            apply_box_transition(3, UnderstandingDepth::Comprehension, 0.4),
            0
        );
        assert_eq!(apply_box_transition(4, UnderstandingDepth::Analysis, 0.59), 0);
    }

    #[test]
    fn boundary_confidence_inclusive_at_threshold() {
        // confidence == MIN_CONF_FOR_BOX_PROMOTION (0.6 exactly) promotes.
        assert_eq!(apply_box_transition(0, UnderstandingDepth::Recall, 0.6), 1);
    }

    #[test]
    fn unknown_depth_with_strong_confidence_promotes() {
        // The function only special-cases Aware; Unknown is < Aware so falls
        // through to the promotion arm. The comprehension classifier never
        // emits Unknown in practice; this test documents the contract.
        assert_eq!(apply_box_transition(0, UnderstandingDepth::Unknown, 0.9), 1);
    }

    // ─── is_due ─────────────────────────────────────────────────────────

    fn concept_with(
        box_level: u8,
        last_encountered: Option<chrono::DateTime<Utc>>,
    ) -> ConceptState {
        ConceptState {
            concept_id: "test".into(),
            depth: UnderstandingDepth::Aware,
            confidence: 0.5,
            encounter_count: 1,
            last_encountered,
            notes: vec![],
            box_level,
        }
    }

    fn fixed_now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap()
    }

    #[test]
    fn is_due_false_for_never_encountered_concept() {
        let c = concept_with(0, None);
        assert!(!is_due(&c, fixed_now()));
    }

    #[test]
    fn is_due_false_when_within_box0_interval() {
        let now = fixed_now();
        let c = concept_with(0, Some(now - Duration::hours(12)));
        assert!(!is_due(&c, now));
    }

    #[test]
    fn is_due_true_when_past_box0_interval() {
        let now = fixed_now();
        let c = concept_with(0, Some(now - Duration::hours(25)));
        assert!(is_due(&c, now));
    }

    #[test]
    fn is_due_false_for_box4_within_30d() {
        let now = fixed_now();
        let c = concept_with(4, Some(now - Duration::days(29)));
        assert!(!is_due(&c, now));
    }

    #[test]
    fn is_due_true_for_box4_after_31d() {
        let now = fixed_now();
        let c = concept_with(4, Some(now - Duration::days(31)));
        assert!(is_due(&c, now));
    }

    #[test]
    fn is_due_clamps_out_of_range_box_level() {
        // box_level = 99 should clamp to box 4 (interval 30d). 31d gap → due.
        let now = fixed_now();
        let c = concept_with(99, Some(now - Duration::days(31)));
        assert!(is_due(&c, now));
        // 29d gap with clamped box 4 → not due.
        let c = concept_with(99, Some(now - Duration::days(29)));
        assert!(!is_due(&c, now));
    }

    // ─── due_concepts ────────────────────────────────────────────────────

    fn empty_learner() -> LearnerModel {
        LearnerModel {
            profile: crate::learner::LearnerProfile {
                id: uuid::Uuid::new_v4(),
                name: "Test".into(),
                age: 9,
                languages: vec!["en".into()],
                locale: crate::i18n::Locale::English,
                created_at: Utc::now(),
                last_active: Utc::now(),
            },
            concepts: vec![],
            preferences: crate::learner::LearningPreferences::default(),
            current_engagement: crate::learner::EngagementState::Engaged,
            recent_assessments: vec![],
        }
    }

    #[test]
    fn due_concepts_empty_when_max_count_is_zero() {
        let mut learner = empty_learner();
        learner
            .concepts
            .push(concept_with(0, Some(Utc::now() - Duration::days(2))));
        assert!(due_concepts(&learner, fixed_now(), 0).is_empty());
    }

    #[test]
    fn due_concepts_empty_when_no_concepts() {
        let learner = empty_learner();
        assert!(due_concepts(&learner, fixed_now(), 4).is_empty());
    }

    #[test]
    fn due_concepts_empty_when_none_due() {
        let now = fixed_now();
        let mut learner = empty_learner();
        learner
            .concepts
            .push(concept_with(0, Some(now - Duration::hours(12))));
        learner
            .concepts
            .push(concept_with(2, Some(now - Duration::days(2))));
        assert!(due_concepts(&learner, now, 4).is_empty());
    }

    #[test]
    fn due_concepts_filters_out_never_encountered() {
        let now = fixed_now();
        let mut learner = empty_learner();
        learner.concepts.push(concept_with(0, None));
        assert!(due_concepts(&learner, now, 4).is_empty());
    }

    #[test]
    fn due_concepts_sorts_by_overdue_descending_and_truncates() {
        let now = fixed_now();
        let mut learner = empty_learner();
        // box 0 (interval 1d), gap=10d → overdue=9d
        let mut c1 = concept_with(0, Some(now - Duration::days(10)));
        c1.concept_id = "a".into();
        learner.concepts.push(c1);
        // box 4 (interval 30d), gap=31d → overdue=1d
        let mut c2 = concept_with(4, Some(now - Duration::days(31)));
        c2.concept_id = "b".into();
        learner.concepts.push(c2);
        // box 2 (interval 7d), gap=20d → overdue=13d
        let mut c3 = concept_with(2, Some(now - Duration::days(20)));
        c3.concept_id = "c".into();
        learner.concepts.push(c3);
        // box 0 (interval 1d), gap=5d → overdue=4d
        let mut c4 = concept_with(0, Some(now - Duration::days(5)));
        c4.concept_id = "d".into();
        learner.concepts.push(c4);
        // box 0 not due
        let mut c5 = concept_with(0, Some(now - Duration::hours(6)));
        c5.concept_id = "e".into();
        learner.concepts.push(c5);

        let result: Vec<&str> = due_concepts(&learner, now, 4)
            .iter()
            .map(|c| c.concept_id.as_str())
            .collect();
        // Expected order by overdue desc: c=13d, a=9d, d=4d, b=1d. e is not due.
        assert_eq!(result, vec!["c", "a", "d", "b"]);
    }

    #[test]
    fn due_concepts_max_count_truncates_to_subset() {
        let now = fixed_now();
        let mut learner = empty_learner();
        let mut c1 = concept_with(0, Some(now - Duration::days(10)));
        c1.concept_id = "a".into();
        learner.concepts.push(c1);
        let mut c2 = concept_with(2, Some(now - Duration::days(20)));
        c2.concept_id = "b".into();
        learner.concepts.push(c2);
        let mut c3 = concept_with(0, Some(now - Duration::days(5)));
        c3.concept_id = "c".into();
        learner.concepts.push(c3);

        // max=2: should return the top 2 most overdue: b=13d, a=9d.
        let result: Vec<&str> = due_concepts(&learner, now, 2)
            .iter()
            .map(|c| c.concept_id.as_str())
            .collect();
        assert_eq!(result, vec!["b", "a"]);
    }
}
