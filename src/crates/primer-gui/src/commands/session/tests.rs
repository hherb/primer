use super::*;
use crate::config::GuiConfig;
use crate::wiring::build_active_session;
use primer_core::conversation::PedagogicalIntent;
use primer_core::learner::EngagementState;
use tempfile::TempDir;

fn stub_config_with_persistence(home: &std::path::Path) -> GuiConfig {
    // Persist to a real on-disk session DB so the second turn's
    // resume_session path is exercised against actual storage.
    let mut cfg = GuiConfig::default();
    cfg.persistence.session_db = Some(home.join("test_session.db"));
    cfg
}

#[tokio::test]
async fn first_turn_creates_session_and_returns_payload() {
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();
    let dm_arc = Arc::clone(&active.dialogue_manager);

    let mut chunks = Vec::<(usize, String)>::new();
    let payload = run_turn(&dm_arc, "hello", |i, c| chunks.push((i, c.to_string())))
        .await
        .unwrap();

    assert_eq!(payload.child_turn_index, 0, "child lands at index 0");
    assert_eq!(payload.primer_turn_index, 1, "primer lands at index 1");
    assert!(!chunks.is_empty(), "stub backend emits at least one chunk");
    for (idx, _) in &chunks {
        assert_eq!(*idx, payload.primer_turn_index);
    }
}

#[tokio::test]
async fn second_turn_continues_same_session() {
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();
    let dm_arc = Arc::clone(&active.dialogue_manager);

    let first = run_turn(&dm_arc, "hello", |_, _| {}).await.unwrap();
    let second = run_turn(&dm_arc, "tell me more", |_, _| {}).await.unwrap();

    assert_eq!(
        first.session_id, second.session_id,
        "long-lived DM holds the same Session across turns"
    );
    assert_eq!(
        second.child_turn_index, 2,
        "child #2 lands after first exchange"
    );
    assert_eq!(second.primer_turn_index, 3, "primer #2 lands at index 3");
}

#[tokio::test]
async fn resume_path_swaps_dm_session_to_loaded_one() {
    // Models the resume_session command: build active, run a turn
    // to land a session row, drop, build a second active (which
    // mints a fresh session), then load + resume to swap DM's
    // session in place. End state: dm.session.id matches the
    // originally-persisted id.
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());

    // First active: run one turn so a session row exists on disk.
    let first_active = build_active_session(home.path(), &cfg).await.unwrap();
    let first_dm = Arc::clone(&first_active.dialogue_manager);
    let payload = run_turn(&first_dm, "hello", |_, _| {}).await.unwrap();
    let original_id = payload.session_id;
    // Drain background tasks before drop so the row is committed.
    first_dm.lock().await.close_session().await;
    drop(first_active);

    // Second active: brand-new DM, brand-new minted session id.
    let second_active = build_active_session(home.path(), &cfg).await.unwrap();
    let fresh_id_before_resume = second_active.dialogue_manager.lock().await.session.id;
    assert_ne!(
        fresh_id_before_resume, original_id,
        "fresh build mints a fresh session id"
    );

    // Resume: load the original session via the stored Arc, then
    // swap it in via DM::resume_session. After this the DM is
    // logically continuing the persisted conversation.
    let loaded = second_active
        .session_store
        .load_session(original_id)
        .await
        .unwrap()
        .expect("loaded session must exist on disk");
    second_active
        .dialogue_manager
        .lock()
        .await
        .resume_session(loaded)
        .await
        .unwrap();

    let after = second_active.dialogue_manager.lock().await.session.id;
    assert_eq!(
        after, original_id,
        "after resume_session, dm.session.id matches the loaded one"
    );

    // And the loaded session carries the persisted turn count.
    assert_eq!(
        second_active
            .dialogue_manager
            .lock()
            .await
            .session
            .turns
            .len(),
        2,
        "resumed session carries both turns of the original exchange"
    );
}

#[tokio::test]
async fn list_sessions_via_store_after_one_turn() {
    // Builds a session through wiring, runs a turn (the only way to
    // land a sessions row through DM), then uses the same store Arc
    // ActiveSession exposes to read the listing back. Validates the
    // wiring contract — list_sessions sees what send_message wrote
    // — without needing a Tauri state injection harness.

    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();
    let dm_arc = Arc::clone(&active.dialogue_manager);
    let store = Arc::clone(&active.session_store);

    let payload = run_turn(&dm_arc, "what is curiosity?", |_, _| {})
        .await
        .unwrap();
    dm_arc.lock().await.close_session().await;

    let listings = store.list_sessions().await.unwrap();
    assert_eq!(listings.len(), 1, "exactly one persisted session");
    assert_eq!(listings[0].id, payload.session_id);
    assert_eq!(
        listings[0].turn_count, 2,
        "child + primer turns both counted"
    );
}

#[test]
fn resume_rejects_invalid_uuid_inline() {
    // The first thing resume_session does is parse the session_id
    // string into a Uuid; an invalid id must produce a helpful
    // error string the picker can render rather than panicking.
    let err = Uuid::parse_str("not-a-uuid")
        .map_err(|e| format!("invalid session id {:?}: {e}", "not-a-uuid"))
        .unwrap_err();
    assert!(
        err.contains("invalid session id"),
        "user-facing prefix preserved: {err}"
    );
    assert!(
        err.contains("\"not-a-uuid\""),
        "echoes the bad input verbatim so the user can spot the typo: {err}"
    );
}

#[tokio::test]
async fn resume_returns_not_found_for_unknown_uuid() {
    // Mirrors the "no session found" branch in resume_session: a
    // syntactically-valid UUID that no session row backs must
    // produce an Ok(None) at the store layer, which the command
    // turns into a user-facing error string.
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();

    let random_id = Uuid::new_v4();
    let loaded = active.session_store.load_session(random_id).await.unwrap();
    assert!(
        loaded.is_none(),
        "load_session on a never-persisted id yields None"
    );

    // Emulate the command's `.ok_or_else(...)` mapping so the test
    // pins the actual user-facing error shape.
    let err: String = loaded
        .map(|_| String::new())
        .ok_or_else(|| format!("no session found with id {random_id}"))
        .unwrap_err();
    assert!(err.starts_with("no session found with id "));
    assert!(err.contains(&random_id.to_string()));
}

#[tokio::test]
async fn resume_helper_inherits_persisted_locale_on_mismatch() {
    // Resume-on-mismatch behaviour: instead of erroring like the
    // start_session path, the GUI's resume path silently inherits
    // the persisted learner's locale (issue #86 collapsed this from
    // a probe + build_active_session sequence into a single
    // build_active_session_for_resume call that opens the DB once).
    let home = TempDir::new().unwrap();

    // Step 1: build + save under English so the learner row lands
    // with locale=en.
    let cfg_en = stub_config_with_persistence(home.path());
    let active_en = build_active_session(home.path(), &cfg_en).await.unwrap();
    let dm_en = Arc::clone(&active_en.dialogue_manager);
    run_turn(&dm_en, "hello", |_, _| {}).await.unwrap();
    dm_en.lock().await.close_session().await;
    drop(active_en);

    // Step 2: resume with a cfg that asks for German. The helper
    // must inherit English (the stored locale), not German (cfg's
    // request) — without opening the DB twice.
    let mut cfg_de = stub_config_with_persistence(home.path());
    cfg_de.learner.locale = "de".to_string();
    let active_resumed = crate::wiring::build_active_session_for_resume(home.path(), &cfg_de)
        .await
        .unwrap();
    assert_eq!(
        active_resumed.locale,
        primer_core::i18n::Locale::English,
        "resume inherits persisted locale, not cfg's"
    );

    // Step 3: a resume on a fresh home with a fresh cfg (no session
    // DB yet) falls through to cfg's locale because there's no
    // inheritance source.
    let fresh = TempDir::new().unwrap();
    let mut cfg_fresh = stub_config_with_persistence(fresh.path());
    cfg_fresh.learner.locale = "de".to_string();
    let active_fresh = crate::wiring::build_active_session_for_resume(fresh.path(), &cfg_fresh)
        .await
        .unwrap();
    assert_eq!(
        active_fresh.locale,
        primer_core::i18n::Locale::German,
        "no persisted learner → cfg's locale wins"
    );
}

/// Regression guard for issue #87. The
/// `resume_helper_inherits_persisted_locale_on_mismatch` test pins
/// the `ActiveSession.locale` field; this one extends coverage to
/// the two downstream consequences the issue calls out:
///   - the resumed `DialogueManager`'s `learner.profile.locale` is
///     the persisted English value (not cfg's German request); and
///   - a concept inserted *after* resume lands tagged with that
///     persisted locale in the session DB.
///
/// The stub extractor doesn't actually emit concepts in the
/// default test wiring, so this drives `update_turn_concepts`
/// directly on the resumed `session_store` — the same surface the
/// real spawned extractor task writes through.
#[tokio::test]
async fn resume_inherits_persisted_locale_end_to_end() {
    let home = TempDir::new().unwrap();

    // Step 1: build under English, run a turn so a session row
    // lands on disk, then close.
    let cfg_en = stub_config_with_persistence(home.path());
    let active_en = build_active_session(home.path(), &cfg_en).await.unwrap();
    let dm_en = Arc::clone(&active_en.dialogue_manager);
    let payload_en = run_turn(&dm_en, "hello", |_, _| {}).await.unwrap();
    let original_id = payload_en.session_id;
    dm_en.lock().await.close_session().await;
    drop(active_en);

    // Step 2: build_active_session_for_resume with cfg.locale = de.
    // The helper inherits English from the persisted learner row.
    let mut cfg_de = stub_config_with_persistence(home.path());
    cfg_de.learner.locale = "de".to_string();
    let active_resumed = crate::wiring::build_active_session_for_resume(home.path(), &cfg_de)
        .await
        .unwrap();
    assert_eq!(
        active_resumed.locale,
        primer_core::i18n::Locale::English,
        "active session inherits English locale"
    );

    // Step 3: actually load + resume the persisted session into
    // the new DM. The resumed DM must report English in its
    // learner.profile.locale, not the cfg's German.
    let loaded = active_resumed
        .session_store
        .load_session(original_id)
        .await
        .unwrap()
        .expect("the just-persisted session must be loadable");
    active_resumed
        .dialogue_manager
        .lock()
        .await
        .resume_session(loaded)
        .await
        .unwrap();
    assert_eq!(
        active_resumed
            .dialogue_manager
            .lock()
            .await
            .learner
            .profile
            .locale,
        primer_core::i18n::Locale::English,
        "resumed DM's learner carries English, not cfg's German"
    );

    // Step 4: insert a concept against the resumed store. This
    // exercises the SAME `update_turn_concepts` path the spawned
    // extractor task uses post-turn, against the SAME store the
    // GUI handed back. If the in-place locale re-tag from issue
    // #86 silently broke, the row would land tagged 'de'.
    active_resumed
        .session_store
        .update_turn_concepts(original_id, 0, &["post_resume_concept".into()])
        .await
        .unwrap();

    // Deterministically drain any in-flight extractor / comprehension
    // tasks before reading the on-disk artefact from a second
    // connection. `close_session` calls `await_pending_background`
    // internally — this is a real join on the spawned tasks, not a
    // sleep, so the read below sees a settled file. Dropping the
    // active session afterwards releases the SQLite connection so
    // the read-only test seam can re-open.
    active_resumed
        .dialogue_manager
        .lock()
        .await
        .close_session()
        .await;
    drop(active_resumed);

    // Step 5: read the tag back through the primer-storage
    // cross-crate test seam. Verifies the on-disk artefact without
    // pulling rusqlite into primer-gui's dev-deps.
    let session_db = home.path().join("test_session.db");
    let tag = primer_storage::__concept_language_tag_for_tests(&session_db, "post_resume_concept")
        .expect("the post-resume concept must exist with a tag");
    assert_eq!(
        tag, "en",
        "concept inserted after resume carries the persisted locale, not cfg's request"
    );
}

#[tokio::test]
async fn turn_persists_to_session_store() {
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();
    let dm_arc = Arc::clone(&active.dialogue_manager);

    let payload = run_turn(&dm_arc, "what is curiosity?", |_, _| {})
        .await
        .unwrap();

    // Drain the DM's background tasks before re-opening the same
    // DB from a second connection so we don't race a still-running
    // extractor/comprehension/embedding write through the first.
    dm_arc.lock().await.close_session().await;

    // Re-open via the test config's session-db path so we validate
    // the actual on-disk artefact independently of any DM-internal
    // caching.
    let session_db = home.path().join("test_session.db");
    let store =
        primer_storage::SqliteSessionStore::open_for_locale(&session_db, active.locale).unwrap();
    let loaded = primer_core::storage::SessionStore::load_session(&store, payload.session_id)
        .await
        .unwrap()
        .expect("session must be loadable after first turn");
    assert!(
        loaded.turns.len() >= 2,
        "session must persist both the child and primer turns; got {} turns",
        loaded.turns.len()
    );
}

#[test]
fn truncate_short_text_passes_through() {
    let (preview, truncated) = truncate_to_preview("hello", 80);
    assert_eq!(preview, "hello");
    assert!(!truncated);
}

#[test]
fn truncate_long_text_adds_ellipsis() {
    let s = "a".repeat(200);
    let (preview, truncated) = truncate_to_preview(&s, 80);
    assert!(truncated);
    assert!(preview.ends_with('…'));
    // 80 a's + ellipsis = 81 chars
    assert_eq!(preview.chars().count(), 81);
}

#[test]
fn truncate_respects_codepoint_boundaries() {
    // A run of multibyte characters; max_chars is a *char* limit,
    // so we must not split a codepoint.
    let s = "🌟".repeat(10); // 10 chars, 40 bytes
    let (preview, truncated) = truncate_to_preview(&s, 5);
    assert!(truncated);
    // 5 stars + ellipsis = 6 chars
    assert_eq!(preview.chars().count(), 6);
    assert!(preview.starts_with("🌟🌟🌟🌟🌟"));
}

#[tokio::test]
async fn read_turn_list_empty_for_fresh_session() {
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();
    let dm = active.dialogue_manager.lock().await;
    let list = read_turn_list(&dm);
    assert!(list.is_empty(), "no turns before first send_message");
}

#[tokio::test]
async fn read_turn_list_after_one_exchange() {
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();
    let dm_arc = Arc::clone(&active.dialogue_manager);

    run_turn(&dm_arc, "hello", |_, _| {}).await.unwrap();

    let dm = dm_arc.lock().await;
    let list = read_turn_list(&dm);
    assert_eq!(list.len(), 2, "one exchange = child + primer turns");
    assert_eq!(list[0].index, 0);
    assert_eq!(list[0].speaker, "child");
    assert_eq!(list[0].text_preview, "hello");
    assert!(!list[0].truncated);
    assert!(list[0].intent.is_none(), "child turns have no intent");

    assert_eq!(list[1].index, 1);
    assert_eq!(list[1].speaker, "primer");
    assert!(
        list[1].intent.is_some(),
        "primer turn carries the decided intent"
    );
}

#[tokio::test]
async fn read_learner_fresh_session_shape() {
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();
    let dm = active.dialogue_manager.lock().await;
    let snap = read_learner(&dm);

    assert_eq!(snap.profile.name, cfg.learner.name);
    assert_eq!(snap.profile.age, cfg.learner.age);
    assert_eq!(snap.profile.locale, cfg.learner.locale);
    assert_eq!(snap.concept_count, 0);
    assert!(snap.vocab_due.is_empty());
    assert!(snap.recent_engagement.is_empty());
    // Distribution is always six entries — depths the learner
    // has never reached carry count=0. Canonical order matches
    // UnderstandingDepth::ALL.
    let names: Vec<&str> = snap
        .depth_distribution
        .iter()
        .map(|r| r.depth.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "Unknown",
            "Aware",
            "Recall",
            "Comprehension",
            "Application",
            "Analysis"
        ]
    );
    for row in &snap.depth_distribution {
        assert_eq!(
            row.count, 0,
            "fresh learner has no concepts at {}",
            row.depth
        );
    }
}

#[tokio::test]
async fn read_learner_counts_concepts_by_depth() {
    use primer_core::learner::{ConceptState, UnderstandingDepth};
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();
    {
        let mut dm = active.dialogue_manager.lock().await;
        // Inject concepts directly into the in-memory learner —
        // the extractor stub returns empty, so this is the only
        // way to exercise the populated counting path in a unit test.
        dm.learner.concepts.push(ConceptState {
            concept_id: "physics:gravity".into(),
            depth: UnderstandingDepth::Aware,
            confidence: 0.6,
            encounter_count: 1,
            last_encountered: Some(Utc::now()),
            notes: vec![],
            box_level: 0,
        });
        dm.learner.concepts.push(ConceptState {
            concept_id: "biology:photosynthesis".into(),
            depth: UnderstandingDepth::Recall,
            confidence: 0.8,
            encounter_count: 2,
            last_encountered: Some(Utc::now() - chrono::Duration::days(2)),
            notes: vec![],
            box_level: 0,
        });
        dm.learner.concepts.push(ConceptState {
            concept_id: "physics:mass".into(),
            depth: UnderstandingDepth::Aware,
            confidence: 0.5,
            encounter_count: 1,
            last_encountered: Some(Utc::now()),
            notes: vec![],
            box_level: 0,
        });
    }
    let dm = active.dialogue_manager.lock().await;
    let snap = read_learner(&dm);

    assert_eq!(snap.concept_count, 3);
    let by_depth: std::collections::HashMap<_, _> = snap
        .depth_distribution
        .iter()
        .map(|r| (r.depth.as_str(), r.count))
        .collect();
    assert_eq!(by_depth["Aware"], 2);
    assert_eq!(by_depth["Recall"], 1);
    assert_eq!(by_depth["Analysis"], 0);
    // Vocab due: photosynthesis is 2 days past its 1-day box-0
    // interval, so it lands in the due list. Mass and gravity
    // were "just encountered" so are not yet due.
    let due_ids: Vec<&str> = snap
        .vocab_due
        .iter()
        .map(|c| c.concept_id.as_str())
        .collect();
    assert_eq!(due_ids, vec!["biology:photosynthesis"]);
    assert!(snap.vocab_due[0].days_until_due <= 0, "must be overdue");
}

#[tokio::test]
async fn read_learner_recent_engagement_oldest_first_and_clamped() {
    use primer_core::classifier::EngagementAssessment;
    use primer_core::learner::EngagementState;
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();
    // Inject more than DEFAULT_HISTORY_DEPTH assessments in
    // chronological order; the snapshot must preserve order
    // (oldest first) and clamp to the display limit. Pushed
    // directly because the in-memory cap from apply_assessment is
    // exactly what we want to exercise from the snapshot side.
    let states = [
        EngagementState::Disengaging,
        EngagementState::Reflecting,
        EngagementState::Engaged,
        EngagementState::FrustratedTrying,
        EngagementState::Engaged,
    ];
    {
        let mut dm = active.dialogue_manager.lock().await;
        for s in states {
            dm.learner.recent_assessments.push(EngagementAssessment {
                state: s,
                confidence: 0.8,
                reasoning: None,
            });
        }
    }
    let dm = active.dialogue_manager.lock().await;
    let snap = read_learner(&dm);

    assert_eq!(
        snap.recent_engagement.len(),
        RECENT_ENGAGEMENT_DISPLAY_LIMIT,
        "clamped to the display limit when source exceeds it"
    );
    // Tail-slice preserves order — the displayed slice is the
    // most-recent N states in the same order they were appended.
    let tail_start = states.len() - RECENT_ENGAGEMENT_DISPLAY_LIMIT;
    let expected: Vec<String> = states[tail_start..]
        .iter()
        .map(|s| s.name().to_string())
        .collect();
    assert_eq!(snap.recent_engagement, expected);
}

#[tokio::test]
async fn read_signals_empty_before_any_turn() {
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();
    let dm = active.dialogue_manager.lock().await;
    let signals = read_signals(&dm);
    assert!(signals.intent.is_none(), "no intent before any turn");
    assert!(
        signals.engagement.is_none(),
        "no engagement before any turn"
    );
    assert!(signals.concepts.child.is_empty());
    assert!(signals.concepts.primer.is_empty());
    assert!(signals.comprehension.is_empty());
    // Identifiers are populated at construction (subsystems always exist).
    assert_eq!(signals.classifier_identifier, "stub");
    assert_eq!(signals.extractor_identifier, "stub");
    assert_eq!(signals.comprehension_identifier, "stub");
}

#[tokio::test]
async fn read_signals_after_first_turn_has_intent_only() {
    // After exactly one respond_to_streaming, intent is current
    // (decided in-turn) but the classifier / extractor /
    // comprehension tasks for that turn haven't been drained —
    // that drain happens at the TOP of turn 2's respond_to_streaming.
    // This is the lag boundary the UI banner promises; pin it.
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();
    let dm_arc = Arc::clone(&active.dialogue_manager);

    run_turn(&dm_arc, "hello", |_, _| {}).await.unwrap();

    let dm = dm_arc.lock().await;
    let signals = read_signals(&dm);
    assert!(
        signals.intent.is_some(),
        "intent is decided in-turn — populated after first respond_to_streaming"
    );
    assert!(
        signals.engagement.is_none(),
        "engagement task is still pending — drain happens at top of turn 2"
    );
    assert!(
        signals.concepts.child.is_empty() && signals.concepts.primer.is_empty(),
        "extractor task is still pending — drain happens at top of turn 2"
    );
    assert!(
        signals.comprehension.is_empty(),
        "comprehension task is still pending — drain happens at top of turn 2"
    );
}

#[tokio::test]
async fn read_signals_populates_after_second_turn() {
    // The DM drains the previous turn's background tasks at the
    // TOP of the next respond_to_streaming. So after turn 2,
    // last_* reflects turn 1's analysis — a populated path the
    // stub classifier/extractor/comprehension all exercise.
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();
    let dm_arc = Arc::clone(&active.dialogue_manager);

    run_turn(&dm_arc, "hello", |_, _| {}).await.unwrap();
    run_turn(&dm_arc, "tell me more", |_, _| {}).await.unwrap();

    let dm = dm_arc.lock().await;
    let signals = read_signals(&dm);
    // Intent is current (decided during turn 2); always populated
    // after at least one respond_to_streaming has run.
    let intent = signals
        .intent
        .as_deref()
        .expect("intent is current — set during turn 2");
    // Stable wire format from PedagogicalIntent::name() — must
    // match one of the canonical variant names. If this assertion
    // ever fires, either a variant was added/renamed in primer-core
    // (update the list below + the CSS) or somebody put the
    // `format!("{:?}", ...)` back. Both need to be caught.
    assert!(
        PedagogicalIntent::ALL.iter().any(|v| v.name() == intent),
        "intent {intent:?} must be a canonical PedagogicalIntent::name()"
    );
    // Stub classifier produces a deterministic Engaged-default
    // assessment — populated after the turn-1 task drain at top
    // of turn 2.
    let eng = signals
        .engagement
        .expect("engagement populated after second turn drains turn-1 classifier task");
    assert!(
        EngagementState::ALL.iter().any(|v| v.name() == eng.state),
        "engagement state {:?} must be a canonical EngagementState::name()",
        eng.state
    );
    assert!(
        (0.0..=1.0).contains(&eng.confidence),
        "confidence in valid range"
    );
}

/// Pre-turn `current_session_info` (via `info_from`) returns the
/// initial snapshot (no `session_id` yet) without ever touching
/// the DM mutex; after `send_message`-style snapshot refresh, the
/// session_id and concept count appear.
#[tokio::test]
async fn snapshot_decouples_info_from_dm_lock() {
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();

    let before = info_from(&active).await;
    assert!(
        before.session_id.is_none(),
        "pre-turn snapshot has no session_id"
    );
    assert_eq!(before.learner.name, cfg.learner.name);
    assert_eq!(before.learner.age, cfg.learner.age);

    // Hold the DM lock for the whole duration of the snapshot
    // refresh + reader call — if `info_from` were still touching
    // the DM mutex, the second `info_from` below would deadlock
    // here (current task holds DM lock, info_from would block
    // waiting for it). Reaching the `assert!` proves info_from
    // never blocks on the DM.
    let dm_arc = Arc::clone(&active.dialogue_manager);
    let _guard = dm_arc.lock().await;
    let during_stream = info_from(&active).await;
    assert_eq!(
        during_stream.learner.id, before.learner.id,
        "info_from returns while DM is locked elsewhere"
    );
    drop(_guard);

    let dm_arc = Arc::clone(&active.dialogue_manager);
    let _payload = run_turn(&dm_arc, "hello", |_, _| {}).await.unwrap();
    refresh_snapshot(&dm_arc, &active.snapshot).await;

    let after = info_from(&active).await;
    assert!(
        after.session_id.is_some(),
        "post-turn snapshot carries the session id"
    );
}

// ─── Cancel-mid-stream tests ──────────────────────────────────────

/// Validates the contract `cancel_response` relies on:
/// `JoinHandle::abort()` on a still-pending task results in a
/// `JoinError::is_cancelled() == true` join result. This is a
/// tokio invariant our cancel path is built on; the test exists
/// to fail loudly if a future tokio bump changes the semantics
/// (which would silently break our cancel path).
#[tokio::test]
async fn abort_handle_yields_cancelled_join_error() {
    let task = tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        "unreachable"
    });
    let handle = task.abort_handle();
    // Yield once so the task enters its sleep.
    tokio::task::yield_now().await;
    handle.abort();
    let err = task.await.unwrap_err();
    assert!(
        err.is_cancelled(),
        "abort() should produce a cancelled JoinError, got {err:?}"
    );
}

/// `current_turn_abort` starts None and stays None after a normal
/// turn — the send_message path's "clear-on-completion" step keeps
/// a stale handle from sitting around between turns.
#[tokio::test]
async fn current_turn_abort_slot_lifecycle() {
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();

    assert!(
        active.current_turn_abort.lock().await.is_none(),
        "starts empty"
    );

    // Mirror the spawn + store + await + clear sequence from
    // send_message.
    let dm_arc = Arc::clone(&active.dialogue_manager);
    let task = tokio::spawn(async move { run_turn(&dm_arc, "hello", |_, _| {}).await });
    *active.current_turn_abort.lock().await = Some(task.abort_handle());

    let result = task.await.expect("task completes without panic");
    assert!(result.is_ok(), "stub turn succeeds");

    *active.current_turn_abort.lock().await = None;
    assert!(
        active.current_turn_abort.lock().await.is_none(),
        "cleared after completion"
    );
}

/// Calling the cancel sequence on a session with no in-flight turn
/// is safe — the optional handle is None and the abort branch is
/// skipped without panic. Mirrors what `cancel_response` does when
/// the user clicks Cancel a moment after the response landed.
#[tokio::test]
async fn cancel_with_idle_session_is_noop() {
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();

    // Drive the production helper directly. With an empty slot
    // this must return cleanly without panicking.
    cancel_active_turn(&active).await;
    assert!(active.current_turn_abort.lock().await.is_none());
}

/// End-to-end smoke for the cancel path's effect on a live task:
/// spawn a pending task, stash its abort handle, drive
/// `cancel_active_turn`, and verify the join result reports
/// cancellation. Pins the abort *wiring* (not just the slot
/// mechanics) without needing a Tauri runtime in scope.
#[tokio::test]
async fn cancel_active_turn_aborts_pending_task() {
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();

    // A long sleep stands in for an in-flight respond_to_streaming.
    // What matters is that `.abort()` on the stashed handle drops
    // the future and yields a cancelled JoinError, which is the
    // exact contract `send_message`'s match arm relies on.
    let task = tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    });
    *active.current_turn_abort.lock().await = Some(task.abort_handle());
    tokio::task::yield_now().await;

    cancel_active_turn(&active).await;

    let join_err = task.await.expect_err("task should be cancelled");
    assert!(
        join_err.is_cancelled(),
        "expected cancelled JoinError, got {join_err:?}"
    );
}

/// The frontend (`ui/app.js`) matches `CANCEL_SENTINEL` against
/// this exact string to suppress the error banner on
/// user-initiated cancel. A one-sided rename here without an
/// equivalent change in `ui/app.js` would silently re-surface
/// cancel messages as errors — pin the value so CI catches the
/// drift the moment it lands.
#[test]
fn cancelled_message_is_stable_machine_token() {
    assert_eq!(CANCELLED_MESSAGE, "primer:turn_cancelled");
}

#[tokio::test]
async fn session_info_carries_voice_mode_available_flag() {
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();
    let info = info_from(&active).await;
    // The flag matches whatever feature the test binary was built with.
    assert_eq!(info.voice_mode_available, cfg!(feature = "speech"));
}

// ─── Issue #102: session switch tears down voice mode ────────────

/// `prepare_for_session_change` clears `state.session` even on
/// non-speech builds — the pre-existing `close_session_inner`
/// behaviour must survive the refactor through the new helper.
/// Compiled in every build so the no-speech path stays covered.
#[tokio::test]
async fn prepare_for_session_change_clears_text_session() {
    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());
    let active = build_active_session(home.path(), &cfg).await.unwrap();

    let state = AppState::new(home.path().to_path_buf(), cfg);
    *state.session.lock().await = Some(active);

    prepare_for_session_change(&state).await.unwrap();

    assert!(
        state.session.lock().await.is_none(),
        "text session must be torn down so the next start_session rebuilds it"
    );
}

/// Voice-build only: `prepare_for_session_change` tears down a
/// running voice loop AND preserves `speech.voice_mode_enabled`
/// so the frontend can auto-restart voice mode under the new
/// locale (closes #102 polished follow-up). Without this, every
/// session switch silently flipped voice mode off and required a
/// manual re-enable.
#[cfg(feature = "speech")]
#[tokio::test]
async fn prepare_for_session_change_stops_voice_loop() {
    use primer_speech::voice_loop::VoiceLoopError;

    let home = TempDir::new().unwrap();
    let cfg = stub_config_with_persistence(home.path());

    // Synthesize a voice-loop handle whose task exits cleanly the
    // moment stop_tx is signaled — mirrors the production
    // contract without spinning up cpal/whisper/piper.
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let (cancel_tx, _cancel_rx) = tokio::sync::mpsc::channel::<()>(8);
    let join: tokio::task::JoinHandle<Result<(), VoiceLoopError>> = tokio::spawn(async move {
        let _ = stop_rx.await;
        Ok(())
    });

    let active = build_active_session(home.path(), &cfg).await.unwrap();
    let info = info_from(&active).await;

    // Sticky toggle is on — mirrors the user having voice mode
    // active at the moment of the session switch.
    let mut cfg_with_voice_on = cfg.clone();
    cfg_with_voice_on.speech.voice_mode_enabled = true;
    let state = AppState::new(home.path().to_path_buf(), cfg_with_voice_on);
    *state.session.lock().await = Some(active);
    *state.voice.lock().await = Some(crate::state::ActiveVoiceLoop {
        join,
        stop_tx,
        cancel_response_tx: cancel_tx,
        info,
    });

    prepare_for_session_change(&state).await.unwrap();

    assert!(
        state.voice.lock().await.is_none(),
        "voice loop must be cleared so the next start_voice_mode rebuilds backends \
             under the new locale (issue #102)"
    );
    assert!(
        state.session.lock().await.is_none(),
        "active session must also be cleared — stop_voice_mode_inner drops it as part \
             of its teardown"
    );
    assert!(
        state.config.lock().await.speech.voice_mode_enabled,
        "sticky toggle must survive the session-change teardown — the frontend reads \
             this flag after start_session/resume_session returns and auto-invokes \
             start_voice_mode against the new locale (#102 polished follow-up)"
    );
}

/// Voice-build only: after a session switch from `de` → `en` via
/// `prepare_for_session_change` + rebuild, the new active session
/// is configured under the new locale. This is the
/// construction-time witness of the fix — the broken behaviour
/// from #102 was that the running voice loop kept its German
/// Whisper + Piper backends because `state.voice` was untouched.
/// With the loop now cleared, a subsequent `start_voice_mode`
/// (production path) rebuilds backends from the new cfg.
#[cfg(feature = "speech")]
#[tokio::test]
async fn session_switch_rebuilds_under_new_locale() {
    use primer_speech::voice_loop::VoiceLoopError;

    let home = TempDir::new().unwrap();

    // Step 1: start under German. Uses `no_persist` so neither
    // learner row touches disk — otherwise the locale-mismatch
    // hard-fail from PR #101 would fire on the en-side build (it
    // protects against silent retagging of an existing learner,
    // a separate bug class from the voice-loop teardown gap).
    let mut cfg_de = GuiConfig::default();
    cfg_de.persistence.no_persist = true;
    cfg_de.learner.locale = "de".to_string();
    cfg_de.learner.name = "Hans".to_string();
    let active_de = build_active_session(home.path(), &cfg_de).await.unwrap();
    let info_de = info_from(&active_de).await;
    assert_eq!(active_de.locale.pack_id(), "de");

    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let (cancel_tx, _cancel_rx) = tokio::sync::mpsc::channel::<()>(8);
    let join: tokio::task::JoinHandle<Result<(), VoiceLoopError>> = tokio::spawn(async move {
        let _ = stop_rx.await;
        Ok(())
    });

    let state = AppState::new(home.path().to_path_buf(), cfg_de);
    *state.session.lock().await = Some(active_de);
    *state.voice.lock().await = Some(crate::state::ActiveVoiceLoop {
        join,
        stop_tx,
        cancel_response_tx: cancel_tx,
        info: info_de,
    });

    // Step 2: user switches to English. Mirrors what start_session
    // does after the fix: tear down, then rebuild from current cfg.
    {
        let mut c = state.config.lock().await;
        c.learner.locale = "en".to_string();
        c.learner.name = "Alice".to_string();
    }
    prepare_for_session_change(&state).await.unwrap();

    // Voice loop is gone — this is the *necessary condition* for
    // the production `start_voice_mode` path to rebuild backends
    // (LoopBackends is built once from `cfg.learner.locale` at
    // voice.rs:118-131; pulling the loop out of `state.voice` is
    // what frees the next `start_voice_mode` to construct new
    // ones). This test pins that necessary condition; the actual
    // rebuild is exercised by `start_voice_mode`'s own happy-path
    // tests (cf. #102).
    assert!(state.voice.lock().await.is_none());

    let cfg_en = state.config.lock().await.clone();
    let active_en = build_active_session(&state.home, &cfg_en).await.unwrap();
    assert_eq!(
        active_en.locale.pack_id(),
        "en",
        "freshly-built session uses the new cfg's locale — the production \
             `start_voice_mode` would build LoopBackends against this same cfg"
    );
}
