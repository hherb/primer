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
    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![
        Ok(chunk("Hel", false)),
        Ok(chunk("lo", false)),
        Ok(chunk(" there", false)),
        Ok(chunk("", true)),
    ]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
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
async fn kb_retrieval_uses_small_context_top_k_for_qnn_backend() {
    // Step 1.2.5: under a small-context ("qnn:") backend, the BM25-only
    // KB retrieval must request KB_FINAL_TOP_K_SMALL_CONTEXT passages.
    use primer_core::consts::retrieval::KB_FINAL_TOP_K_SMALL_CONTEXT;
    let backend = Arc::new(
        ScriptedBackend::new(vec![Ok(chunk("ok", false)), Ok(chunk("", true))])
            .with_name("qnn:test-model"),
    );
    let knowledge = Arc::new(TopKRecordingKnowledge::new());
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );
    let _ = dm
        .respond_to_streaming("why is the sky blue", |_| {})
        .await
        .unwrap();
    assert_eq!(
        knowledge.last_top_k(),
        Some(KB_FINAL_TOP_K_SMALL_CONTEXT),
        "qnn backend should request the small-context KB top-K"
    );
}

#[tokio::test]
async fn kb_retrieval_uses_global_top_k_for_cloud_backend() {
    // A non-"qnn:" backend keeps the global KB_FINAL_TOP_K, proving the
    // override is gated on the backend-name detection.
    use primer_core::consts::retrieval::KB_FINAL_TOP_K;
    let backend = Arc::new(ScriptedBackend::new(vec![
        Ok(chunk("ok", false)),
        Ok(chunk("", true)),
    ]));
    let knowledge = Arc::new(TopKRecordingKnowledge::new());
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );
    let _ = dm
        .respond_to_streaming("why is the sky blue", |_| {})
        .await
        .unwrap();
    assert_eq!(
        knowledge.last_top_k(),
        Some(KB_FINAL_TOP_K),
        "non-qnn backend should keep the global KB top-K"
    );
}

#[tokio::test]
async fn respond_to_streaming_returns_full_accumulated_text() {
    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![
        Ok(chunk("Hel", false)),
        Ok(chunk("lo", false)),
        Ok(chunk(" there", false)),
        Ok(chunk("", true)),
    ]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );

    let result = dm.respond_to_streaming("hi", |_| {}).await.unwrap();
    assert_eq!(result, "Hello there");
}

#[tokio::test]
async fn respond_to_streaming_records_full_primer_turn() {
    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![
        Ok(chunk("part one ", false)),
        Ok(chunk("part two", false)),
        Ok(chunk("", true)),
    ]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
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
    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![
        Ok(chunk("partial", false)),
        Err(PrimerError::Inference("simulated network drop".into())),
    ]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
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
    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![Err(PrimerError::Inference(
        primer_core::error::InferenceError::Auth,
    ))]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
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
    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![Ok(chunk("", true))]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
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
    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![
        Ok(chunk("alpha ", false)),
        Ok(chunk("beta", false)),
        Ok(chunk("", true)),
    ]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );

    let result = dm.respond_to("hi").await.unwrap();
    assert_eq!(result, "alpha beta");
}

#[tokio::test]
async fn respond_to_streaming_fires_engine_save_on_success() {
    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![
        Ok(chunk("Hi", false)),
        Ok(chunk("", true)),
    ]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    let store = Arc::new(CountingStore::new());
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
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
    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![
        Ok(chunk("partial", false)),
        Err(PrimerError::Inference("simulated drop".into())),
    ]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    let store = Arc::new(CountingStore::new());
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
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
    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![
        Ok(chunk("ok", false)),
        Ok(chunk("", true)),
    ]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
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
    let backend = std::sync::Arc::new(RepeatingBackend);
    let knowledge = std::sync::Arc::new(EmptyKnowledge);

    let mut dm = DialogueManager::new(
        learner,
        backend.clone(),
        knowledge.clone(),
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
    let backend = std::sync::Arc::new(RepeatingBackend);
    let knowledge = std::sync::Arc::new(EmptyKnowledge);

    let mut dm = DialogueManager::new(
        learner,
        backend.clone(),
        knowledge.clone(),
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
    let backend = std::sync::Arc::new(RepeatingBackend);
    let knowledge = std::sync::Arc::new(EmptyKnowledge);

    let mut dm = DialogueManager::new(
        learner,
        backend.clone(),
        knowledge.clone(),
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
    let backend = std::sync::Arc::new(RepeatingBackend);
    let knowledge = std::sync::Arc::new(EmptyKnowledge);

    let mut dm = DialogueManager::new(
        learner,
        backend.clone(),
        knowledge.clone(),
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

// ─── Vocab spaced-repetition end-to-end ─────────────────────────────────

#[tokio::test]
async fn build_turn_prompt_includes_vocab_section_when_concept_is_overdue() {
    use chrono::{Duration, Utc};
    use primer_core::conversation::PedagogicalIntent;
    use primer_core::learner::{ConceptState, UnderstandingDepth};

    // Scripted backend isn't even called by build_turn_prompt — we drive
    // the prompt-build path directly. EmptyKnowledge keeps the knowledge
    // section empty so the assertion lands cleanly.
    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![Ok(chunk("", true))]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );

    // Inject a concept with a back-dated last_encountered: 10 days ago,
    // box_level=1 (interval = 3 days). Overdue by 7 days.
    dm.learner.concepts.push(ConceptState {
        concept_id: "physics:gravity".into(),
        depth: UnderstandingDepth::Comprehension,
        confidence: 0.85,
        encounter_count: 2,
        last_encountered: Some(Utc::now() - Duration::days(10)),
        notes: vec![],
        box_level: 1,
    });

    let (prompt, _passage_count) = dm
        .build_turn_prompt(
            "tell me a story",
            PedagogicalIntent::SocraticQuestion,
            crate::dialogue_manager::PromptBudgetTier::Full,
        )
        .await;

    assert!(
        prompt.system.contains("topically relevant"),
        "vocab intro should appear in prompt: {}",
        prompt.system
    );
    assert!(
        prompt.system.contains("physics:gravity"),
        "physics:gravity should appear in vocab list: {}",
        prompt.system
    );
    assert!(
        prompt.system.contains("10 days ago"),
        "expected '10 days ago' phrasing in vocab list: {}",
        prompt.system
    );
}

#[tokio::test]
async fn build_turn_prompt_omits_vocab_section_when_no_concept_is_due() {
    use chrono::{Duration, Utc};
    use primer_core::conversation::PedagogicalIntent;
    use primer_core::learner::{ConceptState, UnderstandingDepth};

    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![Ok(chunk("", true))]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );

    // 12h ago, box 0 = 1-day interval → not due.
    dm.learner.concepts.push(ConceptState {
        concept_id: "physics:gravity".into(),
        depth: UnderstandingDepth::Aware,
        confidence: 0.7,
        encounter_count: 1,
        last_encountered: Some(Utc::now() - Duration::hours(12)),
        notes: vec![],
        box_level: 0,
    });

    let (prompt, _passage_count) = dm
        .build_turn_prompt(
            "what is gravity",
            PedagogicalIntent::SocraticQuestion,
            crate::dialogue_manager::PromptBudgetTier::Full,
        )
        .await;

    assert!(
        !prompt.system.contains("topically relevant"),
        "vocab intro should NOT appear when no concept is due: {}",
        prompt.system
    );
}

#[tokio::test]
async fn build_turn_prompt_budgets_system_prompt_for_small_context_backend() {
    use primer_core::consts::prompt_budget::SYSTEM_PROMPT_BUDGET_TOKENS_SMALL_CONTEXT;
    use primer_core::conversation::PedagogicalIntent;
    use primer_core::prompt_budget::estimate_tokens;

    // A small-context ("qnn:") backend with a knowledge base that would
    // otherwise flood the prompt with 5 huge passages (200 marker words
    // each ≈ 750 tokens apiece). The budgeted assembly must keep the
    // system prompt under the small-context ceiling.
    let backend = std::sync::Arc::new(
        ScriptedBackend::new(vec![Ok(chunk("", true))]).with_name("qnn:test-model"),
    );
    let knowledge = std::sync::Arc::new(BigPassageKnowledge::new(5, 200));
    let dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );

    let (prompt, _passage_count) = dm
        .build_turn_prompt(
            "why is the sky blue",
            PedagogicalIntent::SocraticQuestion,
            crate::dialogue_manager::PromptBudgetTier::Full,
        )
        .await;

    assert!(
        estimate_tokens(&prompt.system) <= SYSTEM_PROMPT_BUDGET_TOKENS_SMALL_CONTEXT,
        "qnn system prompt must stay within the small-context budget ({SYSTEM_PROMPT_BUDGET_TOKENS_SMALL_CONTEXT}), got {}",
        estimate_tokens(&prompt.system)
    );
    // The Socratic core always survives.
    assert!(
        prompt.system.contains("Socratic"),
        "pedagogical core must be present: {}",
        prompt.system
    );
}

#[tokio::test]
async fn build_turn_prompt_leaves_large_context_backend_unbudgeted() {
    use primer_core::consts::prompt_budget::SYSTEM_PROMPT_BUDGET_TOKENS_SMALL_CONTEXT;
    use primer_core::conversation::PedagogicalIntent;
    use primer_core::prompt_budget::estimate_tokens;

    // The same flood of passages on a NON-small-context (default-named)
    // backend keeps the unbounded path: the system prompt is free to blow
    // past the small-context ceiling and the full passage bodies appear.
    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![Ok(chunk("", true))]));
    let knowledge = std::sync::Arc::new(BigPassageKnowledge::new(5, 200));
    let dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );

    let (prompt, _passage_count) = dm
        .build_turn_prompt(
            "why is the sky blue",
            PedagogicalIntent::SocraticQuestion,
            crate::dialogue_manager::PromptBudgetTier::Full,
        )
        .await;

    assert!(
        estimate_tokens(&prompt.system) > SYSTEM_PROMPT_BUDGET_TOKENS_SMALL_CONTEXT,
        "large-context prompt should be unbounded (the flood is included), got {}",
        estimate_tokens(&prompt.system)
    );
}

// ─── Break-gate end-to-end tests ────────────────────────────────────────

#[tokio::test]
async fn respond_after_threshold_yields_suggest_break_intent() {
    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![
        Ok(chunk("Hello", false)),
        Ok(chunk("", true)),
    ]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores::default(),
        default_subsystems(),
        primer_core::config::PedagogyConfig::default(),
    );

    // Fast-forward 31 minutes from session start (default
    // break_suggest_after_minutes is 30).
    let advanced = dm.session.started_at + chrono::Duration::minutes(31);
    dm.set_clock_for_test(advanced);

    let _response = dm.respond_to("hello").await.unwrap();

    assert_eq!(
        dm.last_intent(),
        Some(primer_core::conversation::PedagogicalIntent::SuggestBreak),
        "intent should be SuggestBreak after 31 minutes"
    );
    assert!(
        dm.last_break_suggested_at_for_test().is_some(),
        "last_break_suggested_at should be recorded"
    );
}

#[tokio::test]
async fn respond_to_streaming_threads_routing_signals() {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use futures::stream;
    use primer_core::error::Result as PResult;
    use primer_core::inference::{
        GenerationParams, InferenceBackend, Prompt, TokenChunk, TokenStream,
    };

    struct CapturingBackend {
        last_routing: Arc<Mutex<Option<primer_core::router::RoutingSignals>>>,
    }

    #[async_trait]
    impl InferenceBackend for CapturingBackend {
        fn name(&self) -> &str {
            "capture"
        }
        async fn is_available(&self) -> bool {
            true
        }
        async fn generate_stream(
            &self,
            _p: &Prompt,
            params: &GenerationParams,
        ) -> PResult<TokenStream> {
            *self.last_routing.lock().unwrap() = params.routing.clone();
            Ok(Box::pin(stream::once(async {
                Ok(TokenChunk {
                    text: "ok".into(),
                    done: true,
                    ..Default::default()
                })
            })))
        }
    }

    let captured = Arc::new(Mutex::new(None));
    let backend = Arc::new(CapturingBackend {
        last_routing: captured.clone(),
    });
    let knowledge = Arc::new(EmptyKnowledge);
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores::default(),
        default_subsystems(),
        primer_core::config::PedagogyConfig::default(),
    );

    dm.respond_to("why is the sky blue?").await.unwrap();

    let routing = captured.lock().unwrap().clone();
    assert!(
        routing.is_some(),
        "DM must populate GenerationParams.routing"
    );
    assert_eq!(
        routing.unwrap().retrieved_passages,
        0,
        "empty KB ⇒ 0 passages"
    );
}

#[tokio::test]
async fn cadence_resets_after_suggest_break_fires() {
    // Use RepeatingBackend so the same backend can handle both turns
    // (ScriptedBackend can only emit its script once).
    let backend = std::sync::Arc::new(RepeatingBackend);
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores::default(),
        default_subsystems(),
        primer_core::config::PedagogyConfig::default(),
    );

    let advanced = dm.session.started_at + chrono::Duration::minutes(31);
    dm.set_clock_for_test(advanced);
    dm.respond_to("hello").await.unwrap();

    // Second turn at the same wallclock — cadence should still be
    // active (last_suggested_at == now), so should NOT re-fire SuggestBreak.
    dm.respond_to("ok").await.unwrap();

    assert_ne!(
        dm.last_intent(),
        Some(primer_core::conversation::PedagogicalIntent::SuggestBreak),
        "second turn at the same wallclock should fall through to natural intent"
    );
}

// ─── Context-limit recovery loop ────────────────────────────────────────

#[tokio::test]
async fn truncated_turn_retries_then_records_only_final_answer() {
    use primer_core::inference::FinishReason;
    let backend = std::sync::Arc::new(SequenceBackend::new(vec![
        vec![Ok(chunk("A1", false)), Ok(finished(FinishReason::Length))],
        vec![Ok(chunk("A2", false)), Ok(finished(FinishReason::Length))],
        vec![Ok(chunk("A3", false)), Ok(finished(FinishReason::Stop))],
    ]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );
    let pack = crate::prompt_pack::load_cached(primer_core::i18n::Locale::English)
        .expect("English pack must load");
    let apology = pack.memory_limit_retry().to_string();

    let seen = Mutex::new(String::new());
    let final_text = dm
        .respond_to_streaming("teach me about volcanoes", |c| {
            seen.lock().unwrap().push_str(c)
        })
        .await
        .unwrap();

    let seen = seen.into_inner().unwrap();
    // Two apologies streamed (one per retry), all three partials visible.
    assert_eq!(
        seen.matches(&apology).count(),
        2,
        "expected 2 apologies in streamed output, got: {seen:?}"
    );
    assert!(seen.contains("A1") && seen.contains("A2") && seen.contains("A3"));
    // Only the final clean answer is returned AND recorded.
    assert_eq!(final_text, "A3");
    let last = dm.session.turns.last().unwrap();
    assert_eq!(last.speaker, Speaker::Primer);
    assert_eq!(last.text, "A3");
    assert!(
        !last.text.contains(&apology),
        "recorded turn must not contain apology"
    );
}

#[tokio::test]
async fn exhausted_retries_soft_stop_and_record_partial() {
    use primer_core::inference::FinishReason;
    let backend = std::sync::Arc::new(SequenceBackend::new(vec![
        vec![Ok(chunk("A1", false)), Ok(finished(FinishReason::Length))],
        vec![Ok(chunk("A2", false)), Ok(finished(FinishReason::Length))],
        vec![Ok(chunk("A3", false)), Ok(finished(FinishReason::Length))],
    ]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );
    let pack = crate::prompt_pack::load_cached(primer_core::i18n::Locale::English)
        .expect("English pack must load");
    let soft_stop = pack.memory_limit_soft_stop().to_string();

    let seen = Mutex::new(String::new());
    let final_text = dm
        .respond_to_streaming("teach me about volcanoes", |c| {
            seen.lock().unwrap().push_str(c)
        })
        .await
        .unwrap();
    let seen = seen.into_inner().unwrap();

    assert!(
        seen.contains(&soft_stop),
        "soft-stop cue must appear in streamed output; got: {seen:?}"
    );
    assert_eq!(final_text, "A3", "last partial must be returned");
    assert_eq!(
        dm.session.turns.last().unwrap().text,
        "A3",
        "last partial must be recorded"
    );
}

#[tokio::test]
async fn clean_first_try_has_no_retries_or_apologies() {
    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![
        Ok(chunk("hello", false)),
        Ok(chunk("", true)),
    ]));
    let knowledge = std::sync::Arc::new(EmptyKnowledge);
    let mut dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores::default(),
        default_subsystems(),
        PedagogyConfig::default(),
    );
    let pack = crate::prompt_pack::load_cached(primer_core::i18n::Locale::English)
        .expect("English pack must load");
    let apology = pack.memory_limit_retry().to_string();

    let seen = Mutex::new(String::new());
    dm.respond_to_streaming("hi", |c| seen.lock().unwrap().push_str(c))
        .await
        .unwrap();
    let seen = seen.into_inner().unwrap();
    assert!(
        !seen.contains(&apology),
        "clean first try must not stream any apology; got: {seen:?}"
    );
}

#[tokio::test]
async fn build_turn_prompt_no_knowledge_tier_omits_kb_section() {
    use primer_core::conversation::PedagogicalIntent;

    // Use BigPassageKnowledge: every passage body is a repetition of
    // KNOWLEDGEMARKER, which is a unique string that only appears in the
    // system prompt via the KB section. A standard (non-qnn) backend keeps
    // the unbounded path so Full definitely includes KB content.
    let backend = std::sync::Arc::new(ScriptedBackend::new(vec![Ok(chunk("", true))]));
    let knowledge = std::sync::Arc::new(BigPassageKnowledge::new(3, 10));
    let dm = DialogueManager::new(
        test_learner(),
        backend.clone(),
        knowledge.clone(),
        DialogueManagerStores::default(),
        default_subsystems(),
        primer_core::config::PedagogyConfig::default(),
    );

    let (prompt_full, _) = dm
        .build_turn_prompt(
            "why is the sky blue",
            PedagogicalIntent::SocraticQuestion,
            crate::dialogue_manager::PromptBudgetTier::Full,
        )
        .await;
    let (prompt_nokb, _) = dm
        .build_turn_prompt(
            "why is the sky blue",
            PedagogicalIntent::SocraticQuestion,
            crate::dialogue_manager::PromptBudgetTier::NoKnowledge,
        )
        .await;

    // KNOWLEDGEMARKER appears in passage bodies via BigPassageKnowledge::MARKER.
    // Full tier retrieves passages → marker present; NoKnowledge tier skips
    // retrieval → marker absent.
    assert!(
        prompt_full.system.contains(BigPassageKnowledge::MARKER),
        "Full tier must include KB passages (marker absent): {}",
        &prompt_full.system[..prompt_full.system.len().min(300)]
    );
    assert!(
        !prompt_nokb.system.contains(BigPassageKnowledge::MARKER),
        "NoKnowledge tier must NOT include KB passages (marker present): {}",
        &prompt_nokb.system[..prompt_nokb.system.len().min(300)]
    );
}
