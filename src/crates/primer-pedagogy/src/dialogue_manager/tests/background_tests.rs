//! Background-task tests for the dialogue manager: classifier spawn-and-
//! await, post-response (extractor → comprehension) chain, and the
//! end-to-end multi-turn classifier-routing tests.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use primer_core::config::PedagogyConfig;
use primer_core::error::{PrimerError, Result};
use primer_core::learner::EngagementState;
use primer_extractor::ExtractorSettings;

use super::super::test_support::*;
use super::super::*;

#[tokio::test]
async fn respond_to_streaming_spawns_classify_task_and_persists() {
    use primer_classifier::{EngagementClassifier, StubEngagementClassifier};
    use primer_core::classifier::EngagementAssessment;
    use primer_core::storage::SessionStore;
    use primer_storage::SqliteSessionStore;

    // A classifier that always returns FrustratedTrying with high confidence.
    let target_state = EngagementState::FrustratedTrying;
    let classifier: Arc<dyn EngagementClassifier> = Arc::new(
        StubEngagementClassifier::with_response(EngagementAssessment {
            state: target_state,
            confidence: 0.95,
            reasoning: Some("integration test".into()),
        }),
    );

    let storage: Arc<dyn SessionStore> = Arc::new(
        SqliteSessionStore::open_for_locale(
            std::path::Path::new(":memory:"),
            primer_core::i18n::Locale::default(),
        )
        .unwrap(),
    );

    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![
        Ok(chunk("Great question!", false)),
        Ok(chunk("", true)),
    ]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    let settings = ClassifierSettings::default();

    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores {
            session: Some(Arc::clone(&storage) as Arc<dyn SessionStore>),
            learner: None,
        },
        DialogueManagerSubsystems {
            classifier: Arc::clone(&classifier),
            classifier_settings: settings,
            extractor: stub_extractor(),
            extractor_settings: ExtractorSettings::default(),
            comprehension: stub_comprehension(),
            comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
            vocab_settings: crate::VocabSettings::default(),
            embedder: None,
        },
        PedagogyConfig::default(),
    );

    dm.open_session().await.unwrap();

    // Run one full turn. After this call a classify_task should be live.
    let response = dm
        .respond_to_streaming("Why is the sky blue?", |_| {})
        .await
        .unwrap();
    assert!(!response.is_empty(), "should have a non-empty response");

    // The classify_task is now running (or already done). Simulating the
    // start of the next turn by calling await_pending_classification
    // should apply the FrustratedTrying assessment.
    dm.await_pending_classification().await;

    // Assessment applied: current_engagement updated by the stub.
    assert_eq!(
        dm.learner.current_engagement, target_state,
        "await_pending_classification must apply the spawned assessment"
    );
    assert_eq!(
        dm.learner.recent_assessments.len(),
        1,
        "assessment must be pushed into recent_assessments"
    );
}

#[tokio::test]
async fn await_pending_classification_aborts_and_preserves_state_on_timeout() {
    use primer_classifier::EngagementClassifier;
    use primer_core::classifier::{EngagementAssessment, EngagementContext};
    use std::time::Duration;

    // Classifier that sleeps long enough to reliably exceed the test's
    // blocking_timeout. If the timeout path works, the sleep never
    // completes (task gets aborted) and current_engagement stays untouched.
    struct SlowClassifier;

    #[async_trait]
    impl EngagementClassifier for SlowClassifier {
        fn identifier(&self) -> &str {
            "slow"
        }
        async fn classify(&self, _ctx: EngagementContext<'_>) -> Result<EngagementAssessment> {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(EngagementAssessment {
                state: EngagementState::FrustratedTrying,
                confidence: 0.99,
                reasoning: None,
            })
        }
    }

    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![Ok(chunk("hi", false)), Ok(chunk("", true))]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    // Tight timeout so the await reliably trips it before the 5s sleep.
    let settings = ClassifierSettings {
        blocking_timeout: Duration::from_millis(50),
        ..ClassifierSettings::default()
    };

    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores::default(),
        DialogueManagerSubsystems {
            classifier: Arc::new(SlowClassifier),
            classifier_settings: settings,
            extractor: stub_extractor(),
            extractor_settings: ExtractorSettings::default(),
            comprehension: stub_comprehension(),
            comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
            vocab_settings: crate::VocabSettings::default(),
            embedder: None,
        },
        PedagogyConfig::default(),
    );

    // Run a turn so a classify_task is spawned. The task is still
    // sleeping when respond_to_streaming returns.
    let _ = dm.respond_to_streaming("hi", |_| {}).await.unwrap();
    assert!(
        dm.classify_task.is_some(),
        "a classify_task must be spawned after a successful turn"
    );
    // Capture the engagement state AFTER respond_to_streaming so the
    // placeholder word-count heuristic in `update_learner_model` (which
    // mutates `current_engagement` independently of the classifier) does
    // not contaminate this test. We're checking that the timeout path
    // does not apply the slow classifier's pending result, not that the
    // pre-existing heuristic is bypassed.
    let initial = dm.learner.current_engagement;

    // This call should hit the timeout path: abort the task, log
    // tracing::debug!, and return without applying any assessment.
    let started = std::time::Instant::now();
    dm.await_pending_classification().await;
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "await_pending_classification must give up within ~blocking_timeout, \
             not wait for the slow classifier; elapsed={elapsed:?}"
    );
    assert_eq!(
        dm.learner.current_engagement, initial,
        "timeout path must NOT update current_engagement"
    );
    assert!(
        dm.learner.recent_assessments.is_empty(),
        "timeout path must NOT push into recent_assessments"
    );
    assert!(
        dm.classify_task.is_none(),
        "the task handle must be consumed even on timeout"
    );
}

// ─── End-to-end: classifier routing across a multi-turn session ───

#[tokio::test]
async fn end_to_end_classifier_routing_across_multi_turn_session() {
    use primer_classifier::{EngagementClassifier, StubEngagementClassifier};
    use primer_core::classifier::EngagementAssessment;
    use primer_core::conversation::PedagogicalIntent;
    use primer_core::storage::SessionStore;
    use primer_storage::SqliteSessionStore;
    use std::time::Duration;

    // Scripted classifier:
    //   turn 1 -> Engaged, turn 2 -> FrustratedTrying, turn 3 -> Disengaging
    // Exhausted script falls back to Engaged for turn 4 — but by then
    // current_engagement is already Disengaging (applied before turn 4 starts).
    let classifier: Arc<dyn EngagementClassifier> =
        Arc::new(StubEngagementClassifier::with_script(vec![
            EngagementAssessment {
                state: EngagementState::Engaged,
                confidence: 0.9,
                reasoning: None,
            },
            EngagementAssessment {
                state: EngagementState::FrustratedTrying,
                confidence: 0.9,
                reasoning: None,
            },
            EngagementAssessment {
                state: EngagementState::Disengaging,
                confidence: 0.9,
                reasoning: None,
            },
        ]));

    let storage: Arc<dyn SessionStore> = Arc::new(
        SqliteSessionStore::open_for_locale(
            std::path::Path::new(":memory:"),
            primer_core::i18n::Locale::default(),
        )
        .unwrap(),
    );

    let backend = std::sync::Arc::new(RepeatingBackend);
    let knowledge = std::sync::Arc::new(EmptyKnowledge);

    // Generous blocking timeout for deterministic test behaviour.
    let settings = ClassifierSettings {
        blocking_timeout: Duration::from_secs(5),
        ..Default::default()
    };

    let mut learner = test_learner();
    // 60-second threshold: a backdated session (120 s elapsed) reliably
    // routes Disengaging → SessionClose.
    learner.preferences.early_disengagement_threshold = Duration::from_secs(60);

    let mut dm = DialogueManager::new(
        learner,
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores {
            session: Some(Arc::clone(&storage) as Arc<dyn SessionStore>),
            learner: None,
        },
        DialogueManagerSubsystems {
            classifier: Arc::clone(&classifier),
            classifier_settings: settings,
            extractor: stub_extractor(),
            extractor_settings: ExtractorSettings::default(),
            comprehension: stub_comprehension(),
            comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
            vocab_settings: crate::VocabSettings::default(),
            embedder: None,
        },
        PedagogyConfig::default(),
    );

    dm.open_session().await.unwrap();

    // Backdate started_at so elapsed (120 s) exceeds the 60-second threshold.
    // This makes Disengaging → SessionClose rather than Encouragement.
    dm.session.started_at = Utc::now() - chrono::Duration::seconds(120);

    let session_id = dm.session.id;

    // ── Turn 1 ──
    // classify task returns Engaged (first script entry).
    let _r1 = dm
        .respond_to_streaming("i'm curious about gravity", |_| {})
        .await
        .unwrap();
    // Drain the spawned task; apply Engaged.
    dm.await_pending_classification().await;
    assert_eq!(
        dm.learner.current_engagement,
        EngagementState::Engaged,
        "turn 1: engagement must be Engaged"
    );

    // ── Turn 2 ──
    // At the START of respond_to_streaming, await_pending_classification
    // is called internally — but we already drained it above, so there is
    // nothing to await.  After this call, a new task carrying FrustratedTrying
    // is spawned.
    let _r2 = dm
        .respond_to_streaming("I think it's hard to explain", |_| {})
        .await
        .unwrap();
    // Drain the spawned task; apply FrustratedTrying.
    dm.await_pending_classification().await;
    assert_eq!(
        dm.learner.current_engagement,
        EngagementState::FrustratedTrying,
        "turn 2: engagement must be FrustratedTrying after classifier"
    );

    // ── Turn 3 ──
    // Task for this turn returns Disengaging.
    let _r3 = dm
        .respond_to_streaming("I'm not sure but maybe...", |_| {})
        .await
        .unwrap();
    // Drain; apply Disengaging.
    dm.await_pending_classification().await;
    assert_eq!(
        dm.learner.current_engagement,
        EngagementState::Disengaging,
        "turn 3: engagement must be Disengaging after classifier"
    );

    // ── Turn 4 ──
    // At the START of respond_to_streaming, await_pending_classification
    // is called (nothing to drain — we already did it). Then decide_intent
    // sees Disengaging + elapsed (120 s) > threshold (60 s) → SessionClose.
    let _r4 = dm.respond_to_streaming("ok", |_| {}).await.unwrap();

    // last_intent reads the intent stored on the most recent Primer turn.
    let intent = dm.last_intent().expect("intent must be set after turn 4");
    assert_eq!(
        intent,
        PedagogicalIntent::SessionClose,
        "turn 4: Disengaging + elapsed > threshold must route to SessionClose"
    );

    // Drain the task spawned after turn 4 (not needed for intent assertion,
    // but ensures we don't leave background work running after the test).
    dm.await_pending_classification().await;

    // All four child-turn classifications must have been persisted.
    let recent = storage
        .load_recent_assessments(session_id, "stub", 10)
        .await
        .unwrap();
    assert_eq!(
        recent.len(),
        4,
        "all four turn classifications must be persisted; got {}",
        recent.len()
    );
}

#[tokio::test]
async fn end_to_end_save_learner_after_open_and_one_turn() {
    // Build a manager with both Some(SessionStore) and Some(LearnerStore)
    // backed by the same SqliteSessionStore (which implements both traits).
    // Run open_session + one turn and verify the learners row was upserted.
    use primer_core::storage::LearnerStore;
    use primer_storage::SqliteSessionStore;

    let store = Arc::new(
        SqliteSessionStore::open_for_locale(
            std::path::Path::new(":memory:"),
            primer_core::i18n::Locale::default(),
        )
        .unwrap(),
    );

    // Pre-save the learner so the DB has a row to UPDATE rather than INSERT.
    let learner = test_learner();
    store.save_learner(&learner).await.unwrap();

    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![Ok(chunk("Hello!", false)), Ok(chunk("", true))]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);

    let mut dm = DialogueManager::new(
        learner,
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores {
            session: Some(Arc::clone(&store) as Arc<dyn SessionStore>),
            learner: Some(Arc::clone(&store) as Arc<dyn LearnerStore>),
        },
        default_subsystems(),
        PedagogyConfig::default(),
    );

    let _greeting = dm.open_session().await.unwrap();
    let _reply = dm.respond_to("hello").await.unwrap();

    // load_learner should return the persisted row.
    let loaded = store
        .load_learner()
        .await
        .unwrap()
        .expect("learner row must exist");
    assert_eq!(
        loaded.profile.id, dm.learner.profile.id,
        "persisted learner id must match"
    );
}

#[tokio::test]
async fn divergence_bug_closed_via_cli_startup_flow() {
    // Fixture: a fresh DB seeded with a session under UUID U1, no
    // learners row yet (simulates the v3 → v4 upgrade-on-first-open).
    // Then run the CLI's first-run startup flow:
    //   load_learner() == None
    //   most_recent_session_learner_id() == Some(U1)
    //   mint LearnerModel with id=U1, save_learner(...)
    // Assert the resulting LearnerModel.profile.id == U1.
    use primer_core::conversation::Session as ConversationSession;
    use primer_core::storage::{LearnerStore, SessionStore};
    use primer_storage::SqliteSessionStore;

    let store = Arc::new(
        SqliteSessionStore::open_for_locale(
            std::path::Path::new(":memory:"),
            primer_core::i18n::Locale::default(),
        )
        .unwrap(),
    );
    let u1 = uuid::Uuid::new_v4();
    let s = ConversationSession::new(u1);
    store.save_session(&s).await.unwrap();

    // Simulate the CLI startup flow.
    let load_result = store.load_learner().await.unwrap();
    assert!(load_result.is_none(), "no learner row yet");

    let adopted = store
        .most_recent_session_learner_id()
        .await
        .unwrap()
        .expect("session exists");
    assert_eq!(adopted, u1);

    let mut adopted_learner = test_learner();
    adopted_learner.profile.id = adopted;
    store.save_learner(&adopted_learner).await.unwrap();

    // Construct a DialogueManager with the adopted learner.
    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![Ok(chunk("", true))]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    let mut dm = DialogueManager::new(
        adopted_learner,
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores {
            session: Some(Arc::clone(&store) as Arc<dyn SessionStore>),
            learner: Some(Arc::clone(&store) as Arc<dyn LearnerStore>),
        },
        default_subsystems(),
        PedagogyConfig::default(),
    );

    let _ = dm.open_session().await.unwrap();

    assert_eq!(
        dm.session.learner_id, dm.learner.profile.id,
        "session learner_id must match adopted learner id"
    );
    assert_eq!(
        dm.session.learner_id, u1,
        "adopted learner id must be the original session's learner_id"
    );
}

// ─── Extractor + comprehension chain ──────────────────────────────

#[tokio::test]
async fn extract_task_persists_concepts_for_both_turns_after_response() {
    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![Ok(chunk("Hi there!", true))]));
    let extractor = Arc::new(primer_extractor::StubConceptExtractor::with_response(
        ConceptExtraction {
            child_concepts: vec!["topic-a".into()],
            primer_concepts: vec!["topic-b".into()],
        },
    ));
    let store = Arc::new(ConceptCapturingStore::new());

    let stores = DialogueManagerStores {
        session: Some(store.clone() as Arc<dyn primer_core::storage::SessionStore>),
        learner: None,
    };

    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        std::sync::Arc::new(EmptyKnowledge) as std::sync::Arc<dyn primer_core::knowledge::KnowledgeBase>,
        stores,
        subsystems_with_extractor(extractor as Arc<dyn ConceptExtractor>),
        PedagogyConfig::default(),
    );

    dm.respond_to("Hello").await.unwrap();

    // Yield until the spawned extractor task lands its captures.
    for _ in 0..50 {
        if store.captured().len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let captures = store.captured();
    assert_eq!(captures.len(), 2, "expected child + primer captures");
    // Child turn is at index 0, primer at index 1.
    let child_capture = captures.iter().find(|(i, _)| *i == 0).unwrap();
    let primer_capture = captures.iter().find(|(i, _)| *i == 1).unwrap();
    assert_eq!(child_capture.1, vec!["topic-a".to_string()]);
    assert_eq!(primer_capture.1, vec!["topic-b".to_string()]);
}

#[tokio::test]
async fn extract_task_does_not_spawn_on_inference_error() {
    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![Err(PrimerError::Inference("boom".into()))]));
    let extractor = Arc::new(primer_extractor::StubConceptExtractor::with_response(
        ConceptExtraction {
            child_concepts: vec!["should-not-persist".into()],
            primer_concepts: vec![],
        },
    ));
    let store = Arc::new(ConceptCapturingStore::new());

    let stores = DialogueManagerStores {
        session: Some(store.clone() as Arc<dyn primer_core::storage::SessionStore>),
        learner: None,
    };

    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        std::sync::Arc::new(EmptyKnowledge) as std::sync::Arc<dyn primer_core::knowledge::KnowledgeBase>,
        stores,
        subsystems_with_extractor(extractor as Arc<dyn ConceptExtractor>),
        PedagogyConfig::default(),
    );

    let _ = dm.respond_to("Hello").await;

    // Give the runtime a chance to run any spuriously-spawned task.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        store.captured().is_empty(),
        "extractor must not run on inference error"
    );
}

#[tokio::test]
async fn pending_extraction_applied_to_learner_at_next_turn() {
    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![Ok(chunk("Hi turn 1!", true))]));
    // Two turns of extraction scripted: turn 1 surfaces "gravity" + "physics",
    // turn 2 surfaces "mass". Only the first one matters for this test —
    // we want to assert that after respond_to(turn 2), the learner has
    // gravity from turn 1's extraction.
    let extractor = Arc::new(primer_extractor::StubConceptExtractor::with_script(vec![
        ConceptExtraction {
            child_concepts: vec!["gravity".into()],
            primer_concepts: vec!["physics".into()],
        },
        ConceptExtraction {
            child_concepts: vec!["mass".into()],
            primer_concepts: vec![],
        },
    ]));

    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        std::sync::Arc::new(EmptyKnowledge) as std::sync::Arc<dyn primer_core::knowledge::KnowledgeBase>,
        DialogueManagerStores::default(),
        subsystems_with_extractor(extractor as Arc<dyn ConceptExtractor>),
        PedagogyConfig::default(),
    );

    dm.respond_to("turn 1").await.unwrap();

    // Refill the backend script for turn 2.
    backend.set_script(vec![Ok(chunk("Hi turn 2!", true))]);

    // Allow the previous-turn extractor task to complete before turn 2 starts.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    dm.respond_to("turn 2").await.unwrap();

    let names: std::collections::HashSet<&str> = dm
        .learner
        .concepts
        .iter()
        .map(|c| c.concept_id.as_str())
        .collect();
    assert!(
        names.contains("gravity"),
        "child concept 'gravity' should be applied to learner; got: {:?}",
        names
    );
    assert!(
        names.contains("physics"),
        "primer concept 'physics' should be applied to learner; got: {:?}",
        names
    );
}

#[tokio::test]
async fn post_response_chain_persists_extraction_and_comprehension() {
    use primer_comprehension::StubComprehensionClassifier;
    use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};
    use primer_core::extractor::ConceptExtraction;
    use primer_core::learner::UnderstandingDepth;

    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![Ok(chunk("Hi there!", true))]));
    let extractor = Arc::new(primer_extractor::StubConceptExtractor::with_response(
        ConceptExtraction {
            child_concepts: vec!["gravity".into()],
            primer_concepts: vec![],
        },
    ));
    let comprehension = Arc::new(StubComprehensionClassifier::with_response(
        ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "gravity".into(),
                depth: UnderstandingDepth::Recall,
                confidence: 0.8,
                evidence: Some("named the concept".into()),
            }],
        },
    )) as Arc<dyn primer_comprehension::ComprehensionClassifier>;
    let store = Arc::new(ConceptCapturingStore::new());

    let stores = DialogueManagerStores {
        session: Some(store.clone() as Arc<dyn primer_core::storage::SessionStore>),
        learner: None,
    };

    // Build subsystems directly (subsystems_with_extractor doesn't
    // accept a custom comprehension classifier).
    let subsystems = DialogueManagerSubsystems {
        classifier: stub_classifier(),
        classifier_settings: ClassifierSettings::default(),
        extractor: extractor as Arc<dyn ConceptExtractor>,
        extractor_settings: ExtractorSettings::default(),
        comprehension,
        comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
        vocab_settings: crate::VocabSettings::default(),
        embedder: None,
    };

    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        std::sync::Arc::new(EmptyKnowledge) as std::sync::Arc<dyn primer_core::knowledge::KnowledgeBase>,
        stores,
        subsystems,
        PedagogyConfig::default(),
    );

    dm.respond_to("Hello").await.unwrap();
    // close_session drains the post-response chain so both
    // update_exchange_concepts and save_comprehensions have landed
    // and in-memory state has been applied by the time it returns.
    dm.close_session().await;

    // Extraction persisted: child concept captured.
    let captures = store.captured();
    assert!(
        captures
            .iter()
            .any(|(_, names)| names.contains(&"gravity".to_string())),
        "expected child capture of 'gravity'; got {:?}",
        captures
    );

    // Comprehension persisted via save_comprehensions.
    let comp_captures = store.captured_comprehensions();
    assert_eq!(
        comp_captures.len(),
        1,
        "expected one save_comprehensions call; got {:?}",
        comp_captures
    );
    let (primer_idx, assessments, classifier_id) = &comp_captures[0];
    assert_eq!(*primer_idx, 1, "primer turn index should be 1");
    assert_eq!(assessments.len(), 1);
    assert_eq!(assessments[0].concept, "gravity");
    assert_eq!(assessments[0].depth, UnderstandingDepth::Recall);
    assert_eq!(classifier_id, "stub");

    // Last comprehension applied to learner via await_pending_background.
    let last_comp = dm
        .last_comprehension()
        .expect("last_comprehension must be set after the chain runs");
    assert_eq!(last_comp.assessments.len(), 1);
    assert_eq!(last_comp.assessments[0].concept, "gravity");

    // learner.concepts has gravity at Recall (extraction inserted at
    // Aware first; comprehension promoted to Recall via monotonic max).
    let gravity = dm
        .learner
        .concepts
        .iter()
        .find(|c| c.concept_id == "gravity")
        .expect("'gravity' must be in learner concepts");
    assert_eq!(gravity.depth, UnderstandingDepth::Recall);
}

#[tokio::test]
async fn post_response_chain_skips_comprehension_on_empty_extraction() {
    // Stub extractor returns empty → candidate_concepts is empty →
    // comprehension MUST NOT be invoked. The spy comprehension
    // classifier panics if classify() is called.
    struct PanicOnCall;
    #[async_trait]
    impl primer_comprehension::ComprehensionClassifier for PanicOnCall {
        fn identifier(&self) -> &str {
            "panic"
        }
        async fn classify(
            &self,
            _ctx: primer_core::comprehension::ComprehensionContext<'_>,
        ) -> Result<primer_core::comprehension::ComprehensionResult> {
            panic!("comprehension must not be called when extractor returned empty");
        }
    }

    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![Ok(chunk("Hi there!", true))]));
    let extractor =
        Arc::new(primer_extractor::StubConceptExtractor::new()) as Arc<dyn ConceptExtractor>;
    let comprehension =
        Arc::new(PanicOnCall) as Arc<dyn primer_comprehension::ComprehensionClassifier>;
    let store = Arc::new(ConceptCapturingStore::new());

    let stores = DialogueManagerStores {
        session: Some(store.clone() as Arc<dyn primer_core::storage::SessionStore>),
        learner: None,
    };

    let subsystems = DialogueManagerSubsystems {
        classifier: stub_classifier(),
        classifier_settings: ClassifierSettings::default(),
        extractor,
        extractor_settings: ExtractorSettings::default(),
        comprehension,
        comprehension_settings: primer_comprehension::ComprehensionSettings::default(),
        vocab_settings: crate::VocabSettings::default(),
        embedder: None,
    };

    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        std::sync::Arc::new(EmptyKnowledge) as std::sync::Arc<dyn primer_core::knowledge::KnowledgeBase>,
        stores,
        subsystems,
        PedagogyConfig::default(),
    );

    dm.respond_to("Hello").await.unwrap();
    // Drain. If PanicOnCall.classify() were called, the panicked
    // task would surface as an Err inside apply_post_response_outcome
    // (logged) — but the comprehension code path guards on
    // candidates.is_empty() before invoking classify, so classify
    // is never reached and no panic surfaces.
    dm.close_session().await;

    // No comprehension captures because classify was never called.
    let comp_captures = store.captured_comprehensions();
    assert!(
        comp_captures.is_empty(),
        "save_comprehensions must not be invoked when extraction is empty; got {:?}",
        comp_captures
    );
}
