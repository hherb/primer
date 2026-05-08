//! Cross-axis tests for `LearnerStore` round-trips.
//!
//! Originally `#[cfg(test)] mod learner_store_tests` at the bottom of
//! `lib.rs`; now declared as `mod learner_tests` from `super::tests` in
//! `store/mod.rs` so it sees private items via `super::super::*`.

use super::super::*;
use chrono::Utc;
use primer_core::learner::{
    ConceptState, EngagementState, LearnerModel, LearnerProfile, LearningPreferences,
    UnderstandingDepth,
};
use primer_core::storage::LearnerStore;
use std::time::Duration;
use uuid::Uuid;

fn open_in_mem() -> SqliteSessionStore {
    SqliteSessionStore::open_for_locale(
        std::path::Path::new(":memory:"),
        primer_core::i18n::Locale::default(),
    )
    .unwrap()
}

fn sample_learner() -> LearnerModel {
    LearnerModel {
        profile: LearnerProfile {
            id: Uuid::new_v4(),
            name: "Binti".into(),
            age: 8,
            languages: vec!["en".into(), "fr".into()],
            locale: primer_core::i18n::Locale::English,
            created_at: Utc::now(),
            last_active: Utc::now(),
        },
        concepts: vec![
            ConceptState {
                concept_id: "physics:gravity".into(),
                depth: UnderstandingDepth::Comprehension,
                confidence: 0.7,
                encounter_count: 3,
                last_encountered: Some(Utc::now()),
                notes: vec!["mass vs weight confusion".into()],
                box_level: 0,
            },
            ConceptState {
                concept_id: "biology:photosynthesis".into(),
                depth: UnderstandingDepth::Aware,
                confidence: 0.4,
                encounter_count: 1,
                last_encountered: None,
                notes: vec![],
                box_level: 0,
            },
        ],
        preferences: LearningPreferences {
            narrative: 0.8,
            socratic: 0.7,
            visual: 0.5,
            kinesthetic: 0.3,
            typical_session_minutes: 25.0,
            high_engagement_topics: vec!["dinosaurs".into(), "space".into()],
            early_disengagement_threshold: Duration::from_secs(420),
        },
        current_engagement: EngagementState::Engaged,
        recent_assessments: vec![],
    }
}

#[tokio::test]
async fn save_learner_writes_one_row_to_learners_table() {
    let store = open_in_mem();
    store.save_learner(&sample_learner()).await.unwrap();

    let conn = store.conn.lock().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM learners", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

// ─── concept_language_tag (PR 5: i18n cross-locale tagging) ──────

#[tokio::test]
async fn save_learner_tags_concepts_with_store_locale() {
    // German-locale store; first-time concept inserts should land
    // with concept_language_tag = 'de'.
    let store = SqliteSessionStore::open_for_locale(
        std::path::Path::new(":memory:"),
        primer_core::i18n::Locale::German,
    )
    .unwrap();
    store.save_learner(&sample_learner()).await.unwrap();
    let conn = store.conn.lock().unwrap();
    let tags: Vec<String> = conn
        .prepare("SELECT concept_language_tag FROM concepts ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert!(!tags.is_empty(), "expected concepts inserted");
    for t in &tags {
        assert_eq!(t, "de", "every concept should be tagged 'de': {tags:?}");
    }
}

#[tokio::test]
async fn first_locale_to_introduce_concept_owns_the_tag() {
    // INSERT OR IGNORE semantics: once a concept name exists, the
    // tag stays put even if a different-locale store later "inserts"
    // the same name. This is the documented behaviour — first
    // introduction wins. Cross-locale concept linkage is a follow-up
    // PR; for now we just verify the documented behaviour.
    let path = std::env::temp_dir().join(format!(
        "primer-storage-locale-precedence-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    // Open once as English, save a learner whose concepts include
    // "physics:gravity".
    {
        let en_store =
            SqliteSessionStore::open_for_locale(&path, primer_core::i18n::Locale::English).unwrap();
        en_store.save_learner(&sample_learner()).await.unwrap();
    }
    // Reopen the same DB as German and save the SAME learner again
    // (same concept names). The concept rows already exist; their
    // tag should stay 'en' because of INSERT OR IGNORE.
    {
        let de_store =
            SqliteSessionStore::open_for_locale(&path, primer_core::i18n::Locale::German).unwrap();
        de_store.save_learner(&sample_learner()).await.unwrap();
        let conn = de_store.conn.lock().unwrap();
        let tags: Vec<String> = conn
            .prepare("SELECT concept_language_tag FROM concepts ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        for t in &tags {
            assert_eq!(t, "en", "first-introducer wins: {tags:?}");
        }
    }
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn save_learner_writes_one_learner_concepts_row_per_concept() {
    let store = open_in_mem();
    store.save_learner(&sample_learner()).await.unwrap();

    let conn = store.conn.lock().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM learner_concepts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn save_learner_is_idempotent_on_repeat_calls() {
    let store = open_in_mem();
    let l = sample_learner();
    store.save_learner(&l).await.unwrap();
    store.save_learner(&l).await.unwrap();

    let conn = store.conn.lock().unwrap();
    let learners: i64 = conn
        .query_row("SELECT COUNT(*) FROM learners", [], |r| r.get(0))
        .unwrap();
    let learner_concepts: i64 = conn
        .query_row("SELECT COUNT(*) FROM learner_concepts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(learners, 1);
    assert_eq!(learner_concepts, 2);
}

#[tokio::test]
async fn save_learner_updates_concept_in_place() {
    let store = open_in_mem();
    let mut l = sample_learner();
    store.save_learner(&l).await.unwrap();

    // Mutate the first concept's encounter_count and depth.
    l.concepts[0].encounter_count = 7;
    l.concepts[0].depth = UnderstandingDepth::Application;
    store.save_learner(&l).await.unwrap();

    let conn = store.conn.lock().unwrap();
    let (count, depth_id): (i64, i64) = conn
        .query_row(
            "SELECT lc.encounter_count, lc.depth_id
                 FROM learner_concepts lc
                 JOIN concepts c ON c.id = lc.concept_id
                 WHERE c.name = 'physics:gravity'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(count, 7);
    assert_eq!(
        depth_id,
        crate::catalog::understanding_depth_id(UnderstandingDepth::Application)
    );

    // Still only two rows total — no duplicate.
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM learner_concepts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 2);
}

#[tokio::test]
async fn save_learner_is_monotonic_on_concepts() {
    let store = open_in_mem();
    let mut l = sample_learner();
    store.save_learner(&l).await.unwrap();

    // Drop one concept from the in-memory Vec.
    l.concepts.truncate(1);
    store.save_learner(&l).await.unwrap();

    let conn = store.conn.lock().unwrap();
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM learner_concepts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 2, "dropped concept must remain on disk");
}

#[tokio::test]
async fn save_learner_persists_every_understanding_depth_variant() {
    let store = open_in_mem();
    let mut l = sample_learner();
    l.concepts.clear();
    for d in UnderstandingDepth::ALL {
        l.concepts.push(ConceptState {
            concept_id: format!("test:{}", d.name()),
            depth: *d,
            confidence: 0.5,
            encounter_count: 1,
            last_encountered: None,
            notes: vec![],
            box_level: 0,
        });
    }
    store.save_learner(&l).await.unwrap();

    let conn = store.conn.lock().unwrap();
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM learner_concepts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total as usize, UnderstandingDepth::ALL.len());
}

#[tokio::test]
async fn load_learner_returns_none_for_empty_db() {
    let store = open_in_mem();
    let result = store.load_learner().await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn save_then_load_round_trips_every_field() {
    let store = open_in_mem();
    let original = sample_learner();
    store.save_learner(&original).await.unwrap();

    let loaded = store.load_learner().await.unwrap().expect("learner row");

    // Profile.
    assert_eq!(loaded.profile.id, original.profile.id);
    assert_eq!(loaded.profile.name, original.profile.name);
    assert_eq!(loaded.profile.age, original.profile.age);
    assert_eq!(loaded.profile.languages, original.profile.languages);
    // Timestamps round-trip via RFC 3339; allow sub-second equality.
    assert_eq!(
        loaded.profile.created_at.timestamp(),
        original.profile.created_at.timestamp()
    );
    assert_eq!(
        loaded.profile.last_active.timestamp(),
        original.profile.last_active.timestamp()
    );

    // Preferences.
    assert!((loaded.preferences.narrative - original.preferences.narrative).abs() < 1e-6);
    assert!((loaded.preferences.socratic - original.preferences.socratic).abs() < 1e-6);
    assert!((loaded.preferences.visual - original.preferences.visual).abs() < 1e-6);
    assert!((loaded.preferences.kinesthetic - original.preferences.kinesthetic).abs() < 1e-6);
    assert_eq!(
        loaded.preferences.high_engagement_topics,
        original.preferences.high_engagement_topics
    );
    assert_eq!(
        loaded.preferences.early_disengagement_threshold,
        original.preferences.early_disengagement_threshold
    );

    // Engagement snapshot.
    assert_eq!(loaded.current_engagement, original.current_engagement);

    // Concepts — match by concept_id (order is not guaranteed by SELECT).
    assert_eq!(loaded.concepts.len(), original.concepts.len());
    for original_c in &original.concepts {
        let loaded_c = loaded
            .concepts
            .iter()
            .find(|c| c.concept_id == original_c.concept_id)
            .expect("concept present");
        assert_eq!(loaded_c.depth, original_c.depth);
        assert!((loaded_c.confidence - original_c.confidence).abs() < 1e-6);
        assert_eq!(loaded_c.encounter_count, original_c.encounter_count);
        assert_eq!(loaded_c.notes, original_c.notes);
        assert_eq!(
            loaded_c.last_encountered.map(|t| t.timestamp()),
            original_c.last_encountered.map(|t| t.timestamp()),
        );
        assert_eq!(loaded_c.box_level, original_c.box_level);
    }

    // recent_assessments is rehydrated separately from
    // turn_classifications and is not part of the round-trip.
    assert!(loaded.recent_assessments.is_empty());
}

#[tokio::test]
async fn save_learner_persists_box_level_per_concept() {
    let store = open_in_mem();
    let mut learner = sample_learner();
    learner.concepts = vec![
        ConceptState {
            concept_id: "physics:gravity".into(),
            depth: UnderstandingDepth::Comprehension,
            confidence: 0.85,
            encounter_count: 3,
            last_encountered: Some(Utc::now()),
            notes: vec![],
            box_level: 2,
        },
        ConceptState {
            concept_id: "biology:photosynthesis".into(),
            depth: UnderstandingDepth::Recall,
            confidence: 0.7,
            encounter_count: 1,
            last_encountered: Some(Utc::now()),
            notes: vec![],
            box_level: 1,
        },
    ];
    store.save_learner(&learner).await.unwrap();

    // Read raw rows to verify box_level was written.
    let conn = store.conn.lock().unwrap();
    let mut rows: Vec<(String, i64)> = conn
        .prepare(
            "SELECT c.name, lc.box_level FROM learner_concepts lc \
                 JOIN concepts c ON c.id = lc.concept_id ORDER BY c.name",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    rows.sort();
    assert_eq!(
        rows,
        vec![
            ("biology:photosynthesis".to_string(), 1),
            ("physics:gravity".to_string(), 2),
        ]
    );
}

#[tokio::test]
async fn save_then_load_learner_round_trips_box_level() {
    let store = open_in_mem();
    let mut learner = sample_learner();
    learner.concepts = vec![ConceptState {
        concept_id: "physics:gravity".into(),
        depth: UnderstandingDepth::Application,
        confidence: 0.9,
        encounter_count: 5,
        last_encountered: Some(Utc::now()),
        notes: vec!["struggled at first".into()],
        box_level: 3,
    }];
    store.save_learner(&learner).await.unwrap();

    let loaded = store
        .load_learner()
        .await
        .unwrap()
        .expect("learner present");
    let concept = loaded
        .concepts
        .iter()
        .find(|c| c.concept_id == "physics:gravity")
        .unwrap();
    assert_eq!(concept.box_level, 3);
    assert_eq!(concept.depth, UnderstandingDepth::Application);
    assert_eq!(concept.encounter_count, 5);
}

/// Inject a row with an out-of-range `age` to prove that
/// `load_learner` does not silently truncate `i64 → u8`. A corrupt
/// or hostile DB with `age = 300` must error rather than return a
/// learner with `age = 44` (300 mod 256).
#[tokio::test]
async fn load_learner_rejects_age_outside_u8_range() {
    let store = open_in_mem();
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO learners (
                     id, name, age, languages, created_at, last_active,
                     pref_narrative, pref_socratic, pref_visual, pref_kinesthetic,
                     typical_session_minutes, high_engagement_topics,
                     early_disengagement_secs, current_engagement_state_id
                 ) VALUES (?1, 'Test', 300, '[]', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                           0.5, 0.5, 0.5, 0.5, 20.0, '[]', 300, 1)",
            rusqlite::params!["00000000-0000-0000-0000-000000000001"],
        )
        .unwrap();
    }
    let err = store
        .load_learner()
        .await
        .expect_err("expected out-of-range error, not silent truncation");
    let msg = format!("{err}");
    assert!(
        msg.contains("age"),
        "error must name the failing field: got {msg:?}"
    );
    assert!(
        msg.contains("300"),
        "error must include the offending value: got {msg:?}"
    );
}

#[tokio::test]
async fn load_learner_rejects_negative_age() {
    let store = open_in_mem();
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO learners (
                     id, name, age, languages, created_at, last_active,
                     pref_narrative, pref_socratic, pref_visual, pref_kinesthetic,
                     typical_session_minutes, high_engagement_topics,
                     early_disengagement_secs, current_engagement_state_id
                 ) VALUES (?1, 'Test', -1, '[]', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                           0.5, 0.5, 0.5, 0.5, 20.0, '[]', 300, 1)",
            rusqlite::params!["00000000-0000-0000-0000-000000000002"],
        )
        .unwrap();
    }
    let err = store
        .load_learner()
        .await
        .expect_err("expected negative-age error");
    assert!(format!("{err}").contains("age"));
}

/// `load_learner` defensively falls back to `Locale::default()` when
/// the on-disk `learners.locale` value isn't a known pack id (the
/// expected source: a third-party tool, a hand-edited DB, or a
/// bit-flip). The intent is that a corrupted locale never locks a
/// child out of their session — the load succeeds and the warn
/// surfaces in `tracing` for observability.
#[tokio::test]
async fn load_learner_with_unknown_locale_falls_back_to_default() {
    let store = open_in_mem();
    {
        let conn = store.conn.lock().unwrap();
        // Insert a learner row with an unknown locale pack id. The
        // schema column is TEXT NOT NULL DEFAULT 'en' but accepts
        // any string — soft-fail at read time is the contract.
        conn.execute(
            "INSERT INTO learners (
                     id, name, age, languages, created_at, last_active,
                     pref_narrative, pref_socratic, pref_visual, pref_kinesthetic,
                     typical_session_minutes, high_engagement_topics,
                     early_disengagement_secs, current_engagement_state_id, locale
                 ) VALUES (?1, 'Test', 8, '[]', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                           0.5, 0.5, 0.5, 0.5, 20.0, '[]', 300, 1, 'zz')",
            rusqlite::params!["00000000-0000-0000-0000-00000000007a"],
        )
        .unwrap();
    }
    let learner = store
        .load_learner()
        .await
        .expect("load_learner must not error on unknown locale")
        .expect("row should be returned");
    assert_eq!(
        learner.profile.locale,
        primer_core::i18n::Locale::default(),
        "unknown locale must fall back to Locale::default()"
    );
}

#[tokio::test]
async fn load_learner_rejects_negative_encounter_count() {
    let store = open_in_mem();
    // Pre-seed a learners row + a concepts row, then inject a bad
    // learner_concepts row with encounter_count = -1.
    let learner_id = "00000000-0000-0000-0000-000000000003";
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO learners (
                     id, name, age, languages, created_at, last_active,
                     pref_narrative, pref_socratic, pref_visual, pref_kinesthetic,
                     typical_session_minutes, high_engagement_topics,
                     early_disengagement_secs, current_engagement_state_id
                 ) VALUES (?1, 'Test', 8, '[]', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                           0.5, 0.5, 0.5, 0.5, 20.0, '[]', 300, 1)",
            rusqlite::params![learner_id],
        )
        .unwrap();
        conn.execute("INSERT INTO concepts (name) VALUES ('test:bad')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO learner_concepts (
                     learner_id, concept_id, depth_id, confidence,
                     encounter_count, last_encountered, notes
                 ) VALUES (?1, (SELECT id FROM concepts WHERE name = 'test:bad'),
                           1, 0.5, -1, NULL, '[]')",
            rusqlite::params![learner_id],
        )
        .unwrap();
    }
    let err = store
        .load_learner()
        .await
        .expect_err("expected negative encounter_count to error");
    let msg = format!("{err}");
    assert!(
        msg.contains("encounter_count"),
        "error must name the failing field: got {msg:?}"
    );
}

#[tokio::test]
async fn load_learner_rejects_encounter_count_above_u32_max() {
    let store = open_in_mem();
    let learner_id = "00000000-0000-0000-0000-000000000004";
    let too_big: i64 = (u32::MAX as i64) + 1;
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO learners (
                     id, name, age, languages, created_at, last_active,
                     pref_narrative, pref_socratic, pref_visual, pref_kinesthetic,
                     typical_session_minutes, high_engagement_topics,
                     early_disengagement_secs, current_engagement_state_id
                 ) VALUES (?1, 'Test', 8, '[]', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                           0.5, 0.5, 0.5, 0.5, 20.0, '[]', 300, 1)",
            rusqlite::params![learner_id],
        )
        .unwrap();
        conn.execute("INSERT INTO concepts (name) VALUES ('test:huge')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO learner_concepts (
                     learner_id, concept_id, depth_id, confidence,
                     encounter_count, last_encountered, notes
                 ) VALUES (?1, (SELECT id FROM concepts WHERE name = 'test:huge'),
                           1, 0.5, ?2, NULL, '[]')",
            rusqlite::params![learner_id, too_big],
        )
        .unwrap();
    }
    let err = store
        .load_learner()
        .await
        .expect_err("expected encounter_count > u32::MAX to error");
    assert!(format!("{err}").contains("encounter_count"));
}

#[tokio::test]
async fn load_learner_rejects_negative_early_disengagement_secs() {
    let store = open_in_mem();
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO learners (
                     id, name, age, languages, created_at, last_active,
                     pref_narrative, pref_socratic, pref_visual, pref_kinesthetic,
                     typical_session_minutes, high_engagement_topics,
                     early_disengagement_secs, current_engagement_state_id
                 ) VALUES (?1, 'Test', 8, '[]', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                           0.5, 0.5, 0.5, 0.5, 20.0, '[]', -5, 1)",
            rusqlite::params!["00000000-0000-0000-0000-000000000005"],
        )
        .unwrap();
    }
    let err = store
        .load_learner()
        .await
        .expect_err("expected negative early_disengagement_secs to error");
    assert!(format!("{err}").contains("early_disengagement_secs"));
}
