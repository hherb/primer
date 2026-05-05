//! Per-turn hot-path tests for the dialogue manager: respond_to_streaming
//! callback / accumulation / save / error semantics, plus the per-turn
//! learner save dirty-flag tests.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use primer_core::config::PedagogyConfig;
use primer_core::conversation::Speaker;
use primer_core::error::{PrimerError, Result};
use primer_core::learner::{EngagementState, LearnerModel};

use super::super::test_support::*;
use super::super::*;

#[tokio::test]
async fn respond_to_streaming_invokes_callback_per_chunk() {
    let backend = ScriptedBackend::new(vec![
        Ok(chunk("Hel", false)),
        Ok(chunk("lo", false)),
        Ok(chunk(" there", false)),
        Ok(chunk("", true)),
    ]);
    let knowledge = EmptyKnowledge;
    let mut dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );

    let received: Mutex<Vec<String>> = Mutex::new(vec![]);
    let _ = dm
        .respond_to_streaming("why is the sky blue", |c| {
            received.lock().unwrap().push(c.to_string());
        })
        .await
        .unwrap();

    let joined: String = received.lock().unwrap().join("");
    assert_eq!(joined, "Hello there");
}

#[tokio::test]
async fn respond_to_streaming_returns_full_accumulated_text() {
    let backend = ScriptedBackend::new(vec![
        Ok(chunk("Hel", false)),
        Ok(chunk("lo", false)),
        Ok(chunk(" there", false)),
        Ok(chunk("", true)),
    ]);
    let knowledge = EmptyKnowledge;
    let mut dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );

    let result = dm.respond_to_streaming("hi", |_| {}).await.unwrap();
    assert_eq!(result, "Hello there");
}

#[tokio::test]
async fn respond_to_streaming_records_full_primer_turn() {
    let backend = ScriptedBackend::new(vec![
        Ok(chunk("part one ", false)),
        Ok(chunk("part two", false)),
        Ok(chunk("", true)),
    ]);
    let knowledge = EmptyKnowledge;
    let mut dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );

    let _ = dm.respond_to_streaming("question", |_| {}).await.unwrap();
    let last = dm.session.turns.last().unwrap();
    assert_eq!(last.speaker, Speaker::Primer);
    assert_eq!(last.text, "part one part two");
}

#[tokio::test]
async fn respond_to_streaming_does_not_record_primer_turn_on_stream_error() {
    let backend = ScriptedBackend::new(vec![
        Ok(chunk("partial", false)),
        Err(PrimerError::Inference("simulated network drop".into())),
    ]);
    let knowledge = EmptyKnowledge;
    let mut dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );

    let result = dm.respond_to_streaming("question", |_| {}).await;
    assert!(result.is_err(), "expected Err on mid-stream failure");

    // Child turn should be recorded; Primer turn should NOT be.
    assert_eq!(dm.session.turns.len(), 1);
    assert_eq!(dm.session.turns[0].speaker, Speaker::Child);
}

#[tokio::test]
async fn respond_to_streaming_preserves_typed_inference_error_variant() {
    // Regression test for the dialogue_manager.rs:534 fix (commit c1578251).
    // Before the fix, a .map_err wrap re-wrapped typed InferenceError
    // variants from the backend back into InferenceError::Other via
    // format!(...).into(). That destroyed the typed dispatch the i18n
    // render layer relies on — a 401 from Anthropic landed as
    // Other("Generation failed: ...") and the user saw "Something
    // unexpected went wrong" instead of the friendly Auth message.
    //
    // This test asserts that a typed Auth variant from the backend
    // survives the dialogue_manager round-trip with its variant intact.
    let backend = ScriptedBackend::new(vec![Err(PrimerError::Inference(
        primer_core::error::InferenceError::Auth,
    ))]);
    let knowledge = EmptyKnowledge;
    let mut dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );
    let result = dm.respond_to_streaming("question", |_| {}).await;
    assert!(
        matches!(
            result,
            Err(PrimerError::Inference(
                primer_core::error::InferenceError::Auth
            ))
        ),
        "expected typed Auth variant to survive round-trip, got: {result:?}"
    );
}

#[tokio::test]
async fn respond_to_streaming_returns_empty_string_when_stream_yields_no_text() {
    // Backend completes cleanly with only an empty done-chunk. The call
    // should succeed with an empty accumulated string and still record
    // the (empty) Primer turn — the consumer is responsible for noticing
    // and surfacing this as a user-facing problem if they care.
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

    let result = dm.respond_to_streaming("hi", |_| {}).await.unwrap();
    assert_eq!(result, "");
    let last = dm.session.turns.last().unwrap();
    assert_eq!(last.speaker, Speaker::Primer);
    assert_eq!(last.text, "");
}

#[tokio::test]
async fn respond_to_thin_wrapper_still_works() {
    let backend = ScriptedBackend::new(vec![
        Ok(chunk("alpha ", false)),
        Ok(chunk("beta", false)),
        Ok(chunk("", true)),
    ]);
    let knowledge = EmptyKnowledge;
    let mut dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );

    let result = dm.respond_to("hi").await.unwrap();
    assert_eq!(result, "alpha beta");
}

#[tokio::test]
async fn respond_to_streaming_fires_engine_save_on_success() {
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

    // Engine fired exactly one save. Persisted session has both the
    // child input and the Primer response.
    assert_eq!(store.save_count(), 1);
    assert_eq!(store.last_turn_count(), 2);
}

#[tokio::test]
async fn respond_to_streaming_fires_engine_save_on_stream_error() {
    let backend = ScriptedBackend::new(vec![
        Ok(chunk("partial", false)),
        Err(PrimerError::Inference("simulated drop".into())),
    ]);
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

    let result = dm.respond_to_streaming("question", |_| {}).await;
    assert!(result.is_err());

    // Engine fired the save even though the stream errored. Persisted
    // session has only the child turn (Primer turn was dropped).
    assert_eq!(store.save_count(), 1);
    assert_eq!(store.last_turn_count(), 1);
    assert_eq!(dm.session.turns.len(), 1);
    assert_eq!(dm.session.turns[0].speaker, Speaker::Child);
}
#[tokio::test]
async fn summary_does_not_refresh_when_below_threshold_during_active_session() {
    // First respond_to_streaming fires only when there are turns to
    // process. With turn count below window+window, no refresh.
    let backend = ScriptedBackend::new(vec![Ok(chunk("ok", false)), Ok(chunk("", true))]);
    let knowledge = EmptyKnowledge;
    let mut dm = DialogueManager::new(
        test_learner(),
        &backend,
        &knowledge,
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );
    // Pre-load with 21 turns (1 turn pre-window). Far below the
    // 2*window threshold.
    dm.session.turns = make_test_session_with_turns(21, dm.learner.profile.id).turns;
    let _ = dm.respond_to_streaming("hi", |_| {}).await.unwrap();
    assert_eq!(backend.summary_call_count(), 0);
}

// ─── Per-turn save (learner_dirty flag) ──────────────────────────

#[tokio::test]
async fn per_turn_save_skipped_when_no_persisted_field_changes() {
    // learner starts at Reflecting; the input "ok yes" is < 3 words so
    // update_learner_model takes the "match other => other" branch
    // and leaves current_engagement at Reflecting. The classifier is
    // a stub returning no assessments. The only save_learner call is
    // the one open_session emits.
    let (learner, store) = dirty_flag_test_setup(EngagementState::Reflecting);
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
    assert_eq!(
        store.save_count(),
        1,
        "open_session must save once (lifecycle event)"
    );

    let _ = dm.respond_to("ok yes").await.unwrap();
    assert_eq!(
        store.save_count(),
        1,
        "per-turn save must be SKIPPED when no persisted field changed (still 1 from open)"
    );
}

#[tokio::test]
async fn per_turn_save_fires_when_engagement_changes() {
    // learner starts at Reflecting; a long input (>=3 words) maps to
    // Engaged in update_learner_model, which IS a change to a
    // persisted field. dirty=true → per-turn save fires.
    let (learner, store) = dirty_flag_test_setup(EngagementState::Reflecting);
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
    let count_after_open = store.save_count();

    let _ = dm
        .respond_to("this is a longer answer with many words")
        .await
        .unwrap();
    assert_eq!(
        store.save_count(),
        count_after_open + 1,
        "per-turn save must fire exactly once when current_engagement changes"
    );
}

#[tokio::test]
async fn dirty_cleared_after_save_so_subsequent_idle_turn_skips_save() {
    // Sequence: open → dirty turn → idle turn.
    // After the dirty turn, the flag should be cleared; the idle
    // turn must not produce a second per-turn save.
    let (learner, store) = dirty_flag_test_setup(EngagementState::Reflecting);
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

    // Dirty turn — updates engagement Reflecting → Engaged.
    let _ = dm
        .respond_to("this is a longer answer with many words")
        .await
        .unwrap();
    let after_dirty = store.save_count();
    assert_eq!(after_dirty, after_open + 1, "dirty turn must save");

    // Idle turn — current_engagement is now Engaged, input "ok yes"
    // (word_count<3) maps Engaged → Reflecting via the "Engaged =>
    // Reflecting" arm, so the value DOES change. We need an input
    // that keeps Engaged as Engaged: a long input. But that would
    // also keep dirty stable (Engaged → Engaged is no change).
    let _ = dm
        .respond_to("yes that is exactly what I think")
        .await
        .unwrap();
    assert_eq!(
        store.save_count(),
        after_dirty,
        "idle turn (Engaged → Engaged) must NOT save again"
    );
}

/// Always-failing learner store: every `save_learner` returns Err.
/// Used to prove that save failures are logged-and-swallowed rather
/// than propagated up through the dialogue-manager API.
struct FailingLearnerStore {
    attempts: Mutex<u32>,
}
impl FailingLearnerStore {
    fn new() -> Self {
        Self {
            attempts: Mutex::new(0),
        }
    }
    fn attempt_count(&self) -> u32 {
        *self.attempts.lock().unwrap()
    }
}
#[async_trait]
impl primer_core::storage::LearnerStore for FailingLearnerStore {
    async fn save_learner(&self, _learner: &LearnerModel) -> Result<()> {
        *self.attempts.lock().unwrap() += 1;
        Err(PrimerError::Storage(
            "simulated save_learner failure".into(),
        ))
    }
    async fn load_learner(&self) -> Result<Option<LearnerModel>> {
        Ok(None)
    }
}

#[tokio::test]
async fn save_learner_failure_does_not_propagate_through_respond_to() {
    // A failing LearnerStore must be visible only as a tracing::warn —
    // the conversation must continue. Otherwise a flaky disk would
    // shut down the child's session, which is the wrong failure mode
    // for a children's product.
    let mut learner = test_learner();
    learner.current_engagement = EngagementState::Reflecting;
    let failing = Arc::new(FailingLearnerStore::new());
    let backend = RepeatingBackend;
    let knowledge = EmptyKnowledge;

    let mut dm = DialogueManager::new(
        learner,
        &backend,
        &knowledge,
        DialogueManagerStores {
            session: None,
            learner: Some(Arc::clone(&failing) as Arc<dyn LearnerStore>),
        },
        default_subsystems(),
        PedagogyConfig::default(),
    );

    // open_session must succeed despite the underlying save failing.
    let _ = dm
        .open_session()
        .await
        .expect("open_session must not propagate save_learner errors");
    let after_open = failing.attempt_count();
    assert!(after_open >= 1, "open_session must attempt to save");

    // A dirty turn must succeed despite the underlying save failing.
    let reply = dm
        .respond_to("this is a longer answer with many words")
        .await
        .expect("respond_to must not propagate save_learner errors");
    assert!(!reply.is_empty(), "Primer reply must still come through");
    assert!(
        failing.attempt_count() > after_open,
        "per-turn dirty save must be attempted, even though it errors"
    );

    // close_session must also swallow the error (no return value, no panic).
    dm.close_session().await;

    // Because every save errors, the dirty flag should still be set
    // — the save site only clears dirty on success. This is the
    // correct invariant: a failed save did NOT actually flush, so
    // marking clean would be a lie.
    assert!(
        dm.learner_dirty,
        "dirty must remain set when save_learner errors so a future save still runs"
    );
}
