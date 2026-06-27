use super::*;
use primer_classifier::ClassifierSettings;
use primer_core::learner::{EngagementState, LearnerModel, LearnerProfile, LearningPreferences};
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
    let changed = apply_comprehension(&mut learner, &result, &ComprehensionSettings::default());
    // Sub-threshold → outer guard skips entirely. depth, confidence,
    // and box stay as they were; returns false.
    assert!(!changed);
    assert_eq!(learner.concepts[0].depth, UnderstandingDepth::Comprehension);
    assert_eq!(learner.concepts[0].box_level, 2);
}
