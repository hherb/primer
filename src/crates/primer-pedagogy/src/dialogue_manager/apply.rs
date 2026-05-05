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
    let mut changed = false;
    for a in &result.assessments {
        if a.confidence < settings.confidence_threshold {
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
            // even after years of confident re-engagement. Note that
            // MIN_CONF_FOR_BOX_PROMOTION (0.6) numerically equals the outer
            // confidence_threshold today, so the inner reset branch is
            // currently dead code in the default config. The constants are
            // namespaced apart so a future researcher can tune them
            // independently. See docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md
            // ("Subtle but deliberate") for the policy split.
            let new_box = primer_core::vocab::apply_box_transition(c.box_level, a.depth, a.confidence);
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
mod tests {
    use super::*;
    use primer_classifier::ClassifierSettings;
    use primer_core::learner::{
        EngagementState, LearnerModel, LearnerProfile, LearningPreferences,
    };
    use uuid::Uuid;

    fn test_learner() -> LearnerModel {
        LearnerModel {
            profile: LearnerProfile {
                id: Uuid::new_v4(),
                name: "Tester".to_string(),
                age: 8,
                languages: vec!["en".to_string()],
                locale: primer_core::i18n::Locale::English,
                created_at: Utc::now(),
                last_active: Utc::now(),
            },
            concepts: vec![],
            preferences: LearningPreferences::default(),
            current_engagement: EngagementState::Engaged,
            recent_assessments: vec![],
        }
    }

    // ─── apply_assessment ─────────────────────────────────────────────

    #[test]
    fn apply_assessment_pushes_to_recent_assessments() {
        let mut learner = test_learner();
        let settings = ClassifierSettings::default();
        let a = primer_core::classifier::EngagementAssessment {
            state: EngagementState::Reflecting,
            confidence: 0.9,
            reasoning: None,
        };
        apply_assessment(&mut learner, a.clone(), &settings);
        assert_eq!(learner.recent_assessments.len(), 1);
        assert_eq!(
            learner.recent_assessments[0].state,
            EngagementState::Reflecting
        );
    }

    #[test]
    fn apply_assessment_evicts_oldest_when_buffer_full() {
        let mut learner = test_learner();
        let settings = ClassifierSettings {
            history_depth: 2,
            ..Default::default()
        };
        for state in [
            EngagementState::Engaged,
            EngagementState::Reflecting,
            EngagementState::FrustratedStuck,
        ] {
            apply_assessment(
                &mut learner,
                primer_core::classifier::EngagementAssessment {
                    state,
                    confidence: 0.9,
                    reasoning: None,
                },
                &settings,
            );
        }
        assert_eq!(learner.recent_assessments.len(), 2);
        assert_eq!(
            learner.recent_assessments[0].state,
            EngagementState::Reflecting
        );
        assert_eq!(
            learner.recent_assessments[1].state,
            EngagementState::FrustratedStuck
        );
    }

    #[test]
    fn apply_assessment_updates_current_engagement_when_confident() {
        let mut learner = test_learner();
        let settings = ClassifierSettings {
            confidence_threshold: 0.6,
            ..Default::default()
        };
        apply_assessment(
            &mut learner,
            primer_core::classifier::EngagementAssessment {
                state: EngagementState::FrustratedTrying,
                confidence: 0.8,
                reasoning: None,
            },
            &settings,
        );
        assert_eq!(
            learner.current_engagement,
            EngagementState::FrustratedTrying
        );
    }

    #[test]
    fn apply_assessment_keeps_current_engagement_when_low_confidence() {
        let mut learner = test_learner();
        let initial = learner.current_engagement;
        let settings = ClassifierSettings {
            confidence_threshold: 0.6,
            ..Default::default()
        };
        apply_assessment(
            &mut learner,
            primer_core::classifier::EngagementAssessment {
                state: EngagementState::FrustratedTrying,
                confidence: 0.3,
                reasoning: None,
            },
            &settings,
        );
        assert_eq!(
            learner.current_engagement, initial,
            "low-confidence assessment must NOT change current_engagement"
        );
        assert_eq!(
            learner.recent_assessments.len(),
            1,
            "low-confidence assessment IS still recorded in history"
        );
    }

    // ─── apply_comprehension ──────────────────────────────────────────

    #[test]
    fn apply_comprehension_promotes_depth_via_monotonic_max() {
        use primer_comprehension::ComprehensionSettings;
        use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};
        use primer_core::learner::{ConceptState, UnderstandingDepth};

        let mut learner = test_learner();
        learner.concepts.push(ConceptState {
            concept_id: "gravity".into(),
            depth: UnderstandingDepth::Aware,
            confidence: 0.5,
            encounter_count: 1,
            last_encountered: None,
            notes: vec![],
            box_level: 0,
        });

        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "gravity".into(),
                depth: UnderstandingDepth::Comprehension,
                confidence: 0.85,
                evidence: None,
            }],
        };
        let settings = ComprehensionSettings::default();
        let changed = apply_comprehension(&mut learner, &result, &settings);
        assert!(changed);
        assert_eq!(
            learner
                .concepts
                .iter()
                .find(|c| c.concept_id == "gravity")
                .unwrap()
                .depth,
            UnderstandingDepth::Comprehension,
        );
    }

    #[test]
    fn apply_comprehension_does_not_demote() {
        use primer_comprehension::ComprehensionSettings;
        use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};
        use primer_core::learner::{ConceptState, UnderstandingDepth};

        let mut learner = test_learner();
        learner.concepts.push(ConceptState {
            concept_id: "gravity".into(),
            depth: UnderstandingDepth::Comprehension,
            confidence: 0.85,
            encounter_count: 5,
            last_encountered: None,
            notes: vec![],
            box_level: 0,
        });
        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "gravity".into(),
                depth: UnderstandingDepth::Aware,
                confidence: 0.95,
                evidence: None,
            }],
        };
        let settings = ComprehensionSettings::default();
        let changed = apply_comprehension(&mut learner, &result, &settings);
        assert!(!changed);
        assert_eq!(
            learner
                .concepts
                .iter()
                .find(|c| c.concept_id == "gravity")
                .unwrap()
                .depth,
            UnderstandingDepth::Comprehension,
        );
    }

    #[test]
    fn apply_comprehension_skips_below_confidence_threshold() {
        use primer_comprehension::ComprehensionSettings;
        use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};
        use primer_core::learner::{ConceptState, UnderstandingDepth};

        let mut learner = test_learner();
        learner.concepts.push(ConceptState {
            concept_id: "gravity".into(),
            depth: UnderstandingDepth::Aware,
            confidence: 0.5,
            encounter_count: 1,
            last_encountered: None,
            notes: vec![],
            box_level: 0,
        });
        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "gravity".into(),
                depth: UnderstandingDepth::Comprehension,
                confidence: 0.3, // below default threshold
                evidence: None,
            }],
        };
        let settings = ComprehensionSettings::default();
        let changed = apply_comprehension(&mut learner, &result, &settings);
        assert!(!changed);
        assert_eq!(
            learner
                .concepts
                .iter()
                .find(|c| c.concept_id == "gravity")
                .unwrap()
                .depth,
            UnderstandingDepth::Aware,
        );
    }

    #[test]
    fn apply_comprehension_skips_concept_not_in_learner_model() {
        use primer_comprehension::ComprehensionSettings;
        use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};
        use primer_core::learner::UnderstandingDepth;

        let mut learner = test_learner(); // empty concepts
        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "missing".into(),
                depth: UnderstandingDepth::Comprehension,
                confidence: 0.9,
                evidence: None,
            }],
        };
        let settings = ComprehensionSettings::default();
        let changed = apply_comprehension(&mut learner, &result, &settings);
        assert!(!changed);
        // No insertion — apply_extraction is the only insertion path.
        assert!(!learner.concepts.iter().any(|c| c.concept_id == "missing"));
    }

    // ─── apply_comprehension box-transition ───────────────────────────

    #[test]
    fn apply_comprehension_advances_box_on_strong_assessment_with_depth_promotion() {
        use primer_comprehension::ComprehensionSettings;
        use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};
        use primer_core::learner::{ConceptState, UnderstandingDepth};

        let mut learner = test_learner();
        learner.concepts.push(ConceptState {
            concept_id: "x".into(),
            depth: UnderstandingDepth::Aware,
            confidence: 0.5,
            encounter_count: 1,
            last_encountered: Some(Utc::now()),
            notes: vec![],
            box_level: 0,
        });
        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "x".into(),
                depth: UnderstandingDepth::Recall,
                confidence: 0.85,
                evidence: None,
            }],
        };
        assert!(apply_comprehension(
            &mut learner,
            &result,
            &ComprehensionSettings::default()
        ));
        assert_eq!(learner.concepts[0].depth, UnderstandingDepth::Recall);
        assert_eq!(learner.concepts[0].box_level, 1);
    }

    #[test]
    fn apply_comprehension_advances_box_on_strong_reconfirmation_at_same_depth() {
        // SR-critical: same depth + high confidence still advances the box.
        // Without this, expanding intervals never expand for stable concepts.
        use primer_comprehension::ComprehensionSettings;
        use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};
        use primer_core::learner::{ConceptState, UnderstandingDepth};

        let mut learner = test_learner();
        learner.concepts.push(ConceptState {
            concept_id: "x".into(),
            depth: UnderstandingDepth::Comprehension,
            confidence: 0.8,
            encounter_count: 3,
            last_encountered: Some(Utc::now()),
            notes: vec![],
            box_level: 1,
        });
        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "x".into(),
                depth: UnderstandingDepth::Comprehension,
                confidence: 0.9,
                evidence: None,
            }],
        };
        assert!(apply_comprehension(
            &mut learner,
            &result,
            &ComprehensionSettings::default()
        ));
        assert_eq!(learner.concepts[0].depth, UnderstandingDepth::Comprehension);
        assert_eq!(learner.concepts[0].box_level, 2);
    }

    #[test]
    fn apply_comprehension_resets_box_on_aware_assessment() {
        use primer_comprehension::ComprehensionSettings;
        use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};
        use primer_core::learner::{ConceptState, UnderstandingDepth};

        let mut learner = test_learner();
        learner.concepts.push(ConceptState {
            concept_id: "x".into(),
            depth: UnderstandingDepth::Comprehension,
            confidence: 0.85,
            encounter_count: 5,
            last_encountered: Some(Utc::now()),
            notes: vec![],
            box_level: 3,
        });
        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "x".into(),
                depth: UnderstandingDepth::Aware,
                confidence: 0.85,
                evidence: None,
            }],
        };
        assert!(apply_comprehension(
            &mut learner,
            &result,
            &ComprehensionSettings::default()
        ));
        // Depth is monotonic-max: stays at Comprehension despite Aware reading.
        assert_eq!(learner.concepts[0].depth, UnderstandingDepth::Comprehension);
        // Box dropped to 0 reflecting "needs re-review soon".
        assert_eq!(learner.concepts[0].box_level, 0);
    }

    #[test]
    fn apply_comprehension_leaves_box_unchanged_on_subthreshold_assessment() {
        use primer_comprehension::ComprehensionSettings;
        use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};
        use primer_core::learner::{ConceptState, UnderstandingDepth};

        let mut learner = test_learner();
        learner.concepts.push(ConceptState {
            concept_id: "x".into(),
            depth: UnderstandingDepth::Comprehension,
            confidence: 0.8,
            encounter_count: 3,
            last_encountered: Some(Utc::now()),
            notes: vec![],
            box_level: 2,
        });
        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "x".into(),
                depth: UnderstandingDepth::Application,
                confidence: 0.4, // below default 0.6 threshold
                evidence: None,
            }],
        };
        let changed =
            apply_comprehension(&mut learner, &result, &ComprehensionSettings::default());
        // Sub-threshold → outer guard skips entirely. depth, confidence,
        // and box stay as they were; returns false.
        assert!(!changed);
        assert_eq!(learner.concepts[0].depth, UnderstandingDepth::Comprehension);
        assert_eq!(learner.concepts[0].box_level, 2);
    }
}
