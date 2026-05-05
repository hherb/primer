//! Lifecycle tests for the dialogue manager: open_session, resume_session,
//! close_session, and the resume-flow summary refresh logic.

use std::sync::Arc;

use chrono::Utc;
use primer_core::config::PedagogyConfig;
use primer_core::conversation::{Speaker, Turn};
use primer_core::inference::InferenceBackend;
use primer_core::knowledge::KnowledgeBase;
use primer_core::learner::EngagementState;
use primer_extractor::ExtractorSettings;
use uuid::Uuid;

use super::super::test_support::*;
use super::super::*;

#[tokio::test]
async fn close_session_fires_engine_save_with_ended_at() {
    let backend = ScriptedBackend::new(vec![Ok(chunk("Hi", false)), Ok(chunk("", true))]);
    let knowledge = EmptyKnowledge;
    let store = Arc::new(CountingStore::new());
    let mut dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores {
            session: Some(Arc::clone(&store) as Arc<dyn SessionStore>),
            learner: None,
        },
        default_subsystems(),
        PedagogyConfig::default(),
    );

    let _ = dm.respond_to_streaming("hello", |_| {}).await.unwrap();
    // First save fired during respond_to_streaming.
    let saves_after_response = store.save_count();

    dm.close_session().await;

    // close_session also fires a save, this time with ended_at populated.
    assert_eq!(store.save_count(), saves_after_response + 1);
    assert!(dm.session.ended_at.is_some());
}

#[tokio::test]
async fn open_session_fires_engine_save() {
    let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
    let knowledge = EmptyKnowledge;
    let store = Arc::new(CountingStore::new());
    let mut dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores {
            session: Some(Arc::clone(&store) as Arc<dyn SessionStore>),
            learner: None,
        },
        default_subsystems(),
        PedagogyConfig::default(),
    );

    let _ = dm.open_session().await.unwrap();

    // The greeting turn was recorded and persisted.
    assert_eq!(store.save_count(), 1);
    assert_eq!(store.last_turn_count(), 1);
}

// ─── resume_session and summary refresh ──────────────────────────

#[tokio::test]
async fn resume_session_loads_turns_without_greeting() {
    // Resume picks up the loaded turns verbatim. No greeting is
    // prepended; the turn count after resume_session matches the
    // loaded session exactly.
    let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
    let knowledge = EmptyKnowledge;
    let mut dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );
    let learner_id = dm.learner.profile.id;
    let loaded = make_test_session_with_turns(5, learner_id);
    let loaded_id = loaded.id;
    dm.resume_session(loaded).await.unwrap();
    assert_eq!(dm.session.turns.len(), 5);
    assert_eq!(dm.session.id, loaded_id);
    // The Primer never said "Hello, ..." — turn 0 is from our test fixture.
    assert_eq!(dm.session.turns[0].text, "turn 0");
}

#[tokio::test]
async fn resume_session_clears_ended_at_and_persists() {
    let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
    let knowledge = EmptyKnowledge;
    let store = Arc::new(CountingStore::new());
    let mut dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores {
            session: Some(Arc::clone(&store) as Arc<dyn SessionStore>),
            learner: None,
        },
        default_subsystems(),
        PedagogyConfig::default(),
    );
    let mut loaded = make_test_session_with_turns(3, dm.learner.profile.id);
    loaded.ended_at = Some(Utc::now());
    dm.resume_session(loaded).await.unwrap();
    assert!(dm.session.ended_at.is_none(), "ended_at should be cleared");
    assert_eq!(store.save_count(), 1, "resume should fire one save");
}

#[tokio::test]
async fn resume_session_preserves_loaded_learner_id() {
    // The in-memory LearnerModel comes from CLI flags; the loaded
    // Session might belong to a different learner_id. Resume must
    // keep the loaded learner_id (no silent override).
    let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
    let knowledge = EmptyKnowledge;
    let mut dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );
    let dm_learner_id = dm.learner.profile.id;
    let other_learner = Uuid::new_v4();
    assert_ne!(dm_learner_id, other_learner);
    let loaded = make_test_session_with_turns(2, other_learner);
    dm.resume_session(loaded).await.unwrap();
    assert_eq!(
        dm.session.learner_id, other_learner,
        "session learner_id should not be overwritten by the manager's learner"
    );
}

#[tokio::test]
async fn resume_session_triggers_summary_refresh_when_above_window() {
    // A loaded session with > context_window_turns should get its
    // summary refreshed unconditionally on resume so the Primer has
    // long-term memory of pre-window turns from turn one.
    let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
    let knowledge = EmptyKnowledge;
    let mut dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );
    // Window is 20; 25 turns gives 5 pre-window turns.
    let loaded = make_test_session_with_turns(25, dm.learner.profile.id);
    dm.resume_session(loaded).await.unwrap();
    assert_eq!(
        backend.summary_call_count(),
        1,
        "summary should refresh on resume"
    );
    assert!(
        !dm.session.summary.is_empty(),
        "summary should be populated after refresh"
    );
    assert_eq!(
        dm.session.summary_through_turn_index, 5,
        "summary boundary should land at total - window"
    );
}

#[tokio::test]
async fn resume_session_skips_summary_when_inside_first_window() {
    // Sessions that fit inside the active window have nothing to
    // summarize; resume must not waste an inference call.
    let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
    let knowledge = EmptyKnowledge;
    let mut dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );
    // Window is 20; 5 turns is well inside.
    let loaded = make_test_session_with_turns(5, dm.learner.profile.id);
    dm.resume_session(loaded).await.unwrap();
    assert_eq!(backend.summary_call_count(), 0);
    assert_eq!(dm.session.summary, "");
}

#[tokio::test]
async fn resume_session_skips_refresh_when_summary_already_current() {
    // Loaded session has 25 turns and a summary that already covers
    // turns[..5] — exactly the pre-window range. There is no new
    // pre-window content for the summary to absorb, so resume must
    // not burn an LLM call regenerating identical work.
    let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
    let knowledge = EmptyKnowledge;
    let mut dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );
    let mut loaded = make_test_session_with_turns(25, dm.learner.profile.id);
    loaded.summary = "Pre-existing summary covering turns 0..5.".to_string();
    loaded.summary_through_turn_index = 5;
    dm.resume_session(loaded).await.unwrap();
    assert_eq!(
        backend.summary_call_count(),
        0,
        "summary already covers the pre-window range; resume must not regenerate"
    );
    assert_eq!(
        dm.session.summary, "Pre-existing summary covering turns 0..5.",
        "existing summary must be preserved verbatim"
    );
}

#[tokio::test]
async fn resume_session_refreshes_when_existing_summary_is_stale() {
    // Loaded session has 30 turns and a summary that only covers
    // turns[..3]. The current pre-window range is turns[..10], so
    // there are 7 pre-window turns the summary doesn't yet know
    // about. Resume must refresh.
    let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
    let knowledge = EmptyKnowledge;
    let mut dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );
    let mut loaded = make_test_session_with_turns(30, dm.learner.profile.id);
    loaded.summary = "Stale summary covering only turns 0..3.".to_string();
    loaded.summary_through_turn_index = 3;
    dm.resume_session(loaded).await.unwrap();
    assert_eq!(backend.summary_call_count(), 1);
    assert_eq!(dm.session.summary_through_turn_index, 10);
}
#[tokio::test]
async fn resume_session_rehydrates_recent_assessments() {
    use primer_classifier::{EngagementClassifier, StubEngagementClassifier};
    use primer_core::classifier::EngagementAssessment;
    use primer_core::storage::SessionStore;
    use primer_storage::SqliteSessionStore;

    let storage: Arc<dyn SessionStore> = Arc::new(
        SqliteSessionStore::open_for_locale(
            std::path::Path::new(":memory:"),
            primer_core::i18n::Locale::default(),
        )
        .unwrap(),
    );
    let classifier: Arc<dyn EngagementClassifier> = Arc::new(StubEngagementClassifier::new());

    // Pre-seed: save a session with one child turn and one classification.
    let learner = test_learner();
    let mut session = Session::new(learner.profile.id);
    session.add_turn(Turn {
        speaker: Speaker::Child,
        text: "x".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    storage.save_session(&session).await.unwrap();
    storage
        .save_classification(
            session.id,
            0,
            &EngagementAssessment {
                state: EngagementState::FrustratedTrying,
                confidence: 0.9,
                reasoning: Some("test".into()),
            },
            "stub",
        )
        .await
        .unwrap();

    // Create a DialogueManager and resume the persisted session.
    let backend = ScriptedBackend::new(vec![Ok(chunk("", true))]);
    let knowledge = EmptyKnowledge;
    let settings = ClassifierSettings::default();
    let mut dm = DialogueManager::new(
        learner,
        &backend,
        &knowledge,
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
        },
        PedagogyConfig::default(),
    );

    let loaded = storage
        .load_session(session.id)
        .await
        .unwrap()
        .expect("must load");
    dm.resume_session(loaded).await.unwrap();

    // Verify rehydration.
    assert_eq!(
        dm.learner.recent_assessments.len(),
        1,
        "recent_assessments must be populated from the persisted classification"
    );
    assert_eq!(
        dm.learner.recent_assessments[0].state,
        EngagementState::FrustratedTrying,
        "rehydrated state must match what was saved"
    );
    assert_eq!(
        dm.learner.current_engagement,
        EngagementState::FrustratedTrying,
        "current_engagement must reflect the most recent rehydrated assessment"
    );
}

#[tokio::test]
async fn close_session_always_saves_learner_regardless_of_dirty() {
    // Lifecycle events flush unconditionally — they're explicit
    // checkpoints, not "save when dirty" sites.
    let (learner, store) = dirty_flag_test_setup(EngagementState::Engaged);
    let backend = RepeatingBackend;
    let knowledge = EmptyKnowledge;

    let mut dm = DialogueManager::new(
        learner,
        &backend,
        &knowledge,
        DialogueManagerStores {
            session: None,
            learner: Some(Arc::clone(&store) as Arc<dyn LearnerStore>),
        },
        default_subsystems(),
        PedagogyConfig::default(),
    );

    let _ = dm.open_session().await.unwrap();
    let after_open = store.save_count();
    dm.close_session().await;
    assert!(
        store.save_count() > after_open,
        "close_session must save unconditionally"
    );
}

/// Session-store spy that records `update_turn_concepts` calls so
/// tests can assert the extractor's persistence side effect. Also
/// records `save_comprehensions` calls so chain tests can assert
#[tokio::test]
async fn close_session_drains_extractor_task() {
    let backend = ScriptedBackend::new(vec![Ok(chunk("Hi", true))]);
    let extractor = Arc::new(primer_extractor::StubConceptExtractor::with_response(
        ConceptExtraction {
            child_concepts: vec!["x".into()],
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
        &backend as &dyn InferenceBackend,
        &EmptyKnowledge as &dyn KnowledgeBase,
        stores,
        subsystems_with_extractor(extractor as Arc<dyn ConceptExtractor>),
        PedagogyConfig::default(),
    );

    dm.respond_to("hi").await.unwrap();
    // close_session must drain so the extractor's update_turn_concepts
    // call has landed by the time close returns.
    dm.close_session().await;

    let captures = store.captured();
    assert!(
        !captures.is_empty(),
        "expected extraction to land before close returns"
    );
}

// ─── Chained post-response (extraction → comprehension) ──────────
