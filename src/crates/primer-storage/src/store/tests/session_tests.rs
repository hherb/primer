//! Cross-axis tests for `SqliteSessionStore` (the SessionStore side).
//!
//! Originally a single `#[cfg(test)] mod tests` block at the bottom of
//! `lib.rs`; now declared as `mod session_tests` from `super::tests` in
//! `store/mod.rs` so it sees private items via `super::super::*`.
//! Lifted as-is during the storage-split refactor; further per-axis
//! grouping (schema / migrations / save / retrieve / hybrid) is a
//! follow-up.

use super::super::*;
use std::path::PathBuf;

use chrono::Utc;
use primer_core::conversation::{Session, Turn};
use primer_core::storage::SessionStore;
use uuid::Uuid;

fn open_memory() -> SqliteSessionStore {
    SqliteSessionStore::open_for_locale(
        &PathBuf::from(":memory:"),
        primer_core::i18n::Locale::default(),
    )
    .expect("open :memory:")
}

fn make_turn(
    speaker: primer_core::conversation::Speaker,
    text: &str,
    intent: Option<primer_core::conversation::PedagogicalIntent>,
    concepts: Vec<String>,
) -> Turn {
    Turn {
        speaker,
        text: text.to_string(),
        timestamp: Utc::now(),
        intent,
        concepts,
    }
}

#[test]
fn open_fresh_creates_all_tables_and_sets_user_version() {
    let store = open_memory();
    let conn = store.conn.lock().unwrap();

    // user_version is the current schema version.
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, crate::schema::USER_VERSION);

    // foreign_keys is on.
    let fk: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .unwrap();
    assert_eq!(fk, 1);

    // All base tables exist, plus the v2 FTS index, v3 tables, v4
    // tables, and v5 tables.
    for table in &[
        "speakers",
        "pedagogical_intents",
        "concepts",
        "sessions",
        "turns",
        "turn_concepts",
        "turn_text_fts",
        "engagement_states",
        "classifiers",
        "turn_classifications",
        "understanding_depths",
        "learners",
        "learner_concepts",
        "comprehension_classifiers",
        "turn_comprehensions",
        "embedding_models",
        "embeddings_turns",
    ] {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "table {table} should exist");
    }
}

#[test]
fn open_rejects_incompatible_user_version() {
    // Write a future version into a fresh on-disk DB, then try to open via the store.
    let tmp = tempfile_path();
    {
        let conn = Connection::open(&tmp).unwrap();
        conn.execute_batch("PRAGMA user_version = 99;").unwrap();
    }
    let err = SqliteSessionStore::open_for_locale(&tmp, primer_core::i18n::Locale::default())
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("99"),
        "error should mention the bad version: {msg}"
    );
    assert!(
        msg.contains("Storage"),
        "error should be a Storage variant: {msg}"
    );
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn open_existing_valid_db_is_a_no_op() {
    // First open creates the schema and stamps the current user_version.
    let tmp = tempfile_path();
    {
        let _store =
            SqliteSessionStore::open_for_locale(&tmp, primer_core::i18n::Locale::default())
                .unwrap();
    }
    // Second open should succeed cleanly. user_version stays put.
    let store =
        SqliteSessionStore::open_for_locale(&tmp, primer_core::i18n::Locale::default()).unwrap();
    let conn = store.conn.lock().unwrap();
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, crate::schema::USER_VERSION);
    drop(conn);
    drop(store);
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn open_seeds_lookup_tables_on_fresh_db() {
    let store = open_memory();
    let conn = store.conn.lock().unwrap();

    let speaker_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM speakers", [], |r| r.get(0))
        .unwrap();
    assert_eq!(speaker_count, 2);

    let intent_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM pedagogical_intents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(intent_count, 9);

    // Spot-check a specific row.
    let name: String = conn
        .query_row(
            "SELECT name FROM pedagogical_intents WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(name, "SocraticQuestion");
}

#[test]
fn open_seeds_engagement_states_on_fresh_db() {
    let store = open_memory();
    let conn = store.conn.lock().unwrap();

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM engagement_states", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        count,
        primer_core::learner::EngagementState::ALL.len() as i64,
        "engagement_states row count must equal EngagementState::ALL.len()"
    );

    // Verify every expected (id, name) pair is present.
    for (id, name) in crate::catalog::expected_engagement_states() {
        let actual_name: String = conn
            .query_row(
                "SELECT name FROM engagement_states WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| panic!("no engagement_states row with id={id}"));
        assert_eq!(actual_name, name, "engagement_states id={id} name mismatch");
    }
}

#[test]
fn open_validates_engagement_states_rejects_name_mismatch() {
    // Hand-roll a DB with a corrupted engagement_states row, then open
    // it via SqliteSessionStore::open — it must return an error.
    let tmp = tempfile_path();
    {
        let conn = Connection::open(&tmp).unwrap();
        // Create the table manually with one wrong name so the validator
        // fires before any migration seeds the correct data.
        conn.execute_batch(
            "
                PRAGMA foreign_keys = ON;
                CREATE TABLE engagement_states (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
                INSERT INTO engagement_states (id, name) VALUES (1, 'WRONG_NAME');
                PRAGMA user_version = 1;
                ",
        )
        .unwrap();
    }
    let err = SqliteSessionStore::open_for_locale(&tmp, primer_core::i18n::Locale::default())
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("WRONG_NAME") || msg.contains("Engaged"),
        "error should mention the conflict: {msg}"
    );
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn reopen_is_a_no_op_on_seeded_tables() {
    let tmp = tempfile_path();
    {
        let _store =
            SqliteSessionStore::open_for_locale(&tmp, primer_core::i18n::Locale::default())
                .unwrap();
    }
    let store =
        SqliteSessionStore::open_for_locale(&tmp, primer_core::i18n::Locale::default()).unwrap();
    let conn = store.conn.lock().unwrap();
    let speaker_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM speakers", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        speaker_count, 2,
        "second open should not duplicate seed rows"
    );
    drop(conn);
    drop(store);
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn validate_rejects_name_mismatch() {
    let tmp = tempfile_path();
    {
        let conn = Connection::open(&tmp).unwrap();
        conn.execute_batch(
                "
                PRAGMA foreign_keys = ON;
                CREATE TABLE pedagogical_intents (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
                INSERT INTO pedagogical_intents (id, name) VALUES (1, 'WrongName');
                PRAGMA user_version = 1;
                ",
            )
            .unwrap();
    }
    let err = SqliteSessionStore::open_for_locale(&tmp, primer_core::i18n::Locale::default())
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("WrongName") || msg.contains("SocraticQuestion"),
        "error should mention the conflict: {msg}"
    );
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn validate_rejects_unknown_id() {
    let tmp = tempfile_path();
    {
        let conn = Connection::open(&tmp).unwrap();
        conn.execute_batch(
                "
                PRAGMA foreign_keys = ON;
                CREATE TABLE pedagogical_intents (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
                INSERT INTO pedagogical_intents (id, name) VALUES (99, 'FromTheFuture');
                PRAGMA user_version = 1;
                ",
            )
            .unwrap();
    }
    let err = SqliteSessionStore::open_for_locale(&tmp, primer_core::i18n::Locale::default())
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("99") || msg.contains("unknown"),
        "error should mention the unknown id: {msg}"
    );
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn validate_seeds_missing_rows() {
    // Pre-populate one valid row; the validator should fill the others in.
    let tmp = tempfile_path();
    {
        let conn = Connection::open(&tmp).unwrap();
        conn.execute_batch(
                "
                PRAGMA foreign_keys = ON;
                CREATE TABLE pedagogical_intents (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
                INSERT INTO pedagogical_intents (id, name) VALUES (1, 'SocraticQuestion');
                PRAGMA user_version = 1;
                ",
            )
            .unwrap();
    }
    let store =
        SqliteSessionStore::open_for_locale(&tmp, primer_core::i18n::Locale::default()).unwrap();
    let conn = store.conn.lock().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM pedagogical_intents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 9, "missing rows should have been seeded");
    drop(conn);
    drop(store);
    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
async fn save_empty_session_persists_metadata_only() {
    let store = open_memory();
    let learner_id = Uuid::new_v4();
    let session = Session::new(learner_id);

    store.save_session(&session).await.unwrap();

    let conn = store.conn.lock().unwrap();
    let row: (String, String, String, Option<String>) = conn
        .query_row(
            "SELECT id, learner_id, started_at, ended_at FROM sessions",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(row.0, session.id.to_string());
    assert_eq!(row.1, learner_id.to_string());
    assert!(!row.2.is_empty());
    assert!(row.3.is_none());

    let turn_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0))
        .unwrap();
    assert_eq!(turn_count, 0);
}

#[tokio::test]
async fn save_session_persists_turns_in_order() {
    use primer_core::conversation::{PedagogicalIntent, Speaker};

    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(
        Speaker::Child,
        "why is the sky blue",
        None,
        vec![],
    ));
    session.add_turn(make_turn(
        Speaker::Primer,
        "What do you notice about the sky during the day?",
        Some(PedagogicalIntent::SocraticQuestion),
        vec![],
    ));

    store.save_session(&session).await.unwrap();

    let conn = store.conn.lock().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT turn_index, speaker_id, text, intent_id FROM turns
                     WHERE session_id = ?1 ORDER BY turn_index",
        )
        .unwrap();
    let rows: Vec<(i64, i64, String, Option<i64>)> = stmt
        .query_map(rusqlite::params![session.id.to_string()], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, 0); // turn_index
    assert_eq!(rows[0].1, 1); // Child = 1
    assert_eq!(rows[0].2, "why is the sky blue");
    assert_eq!(rows[0].3, None);

    assert_eq!(rows[1].0, 1);
    assert_eq!(rows[1].1, 2); // Primer = 2
    assert_eq!(rows[1].3, Some(1)); // SocraticQuestion = 1
}

#[tokio::test]
async fn save_session_persists_turn_concepts_with_lazy_creation() {
    use primer_core::conversation::Speaker;

    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(
        Speaker::Child,
        "why does gravity pull things down",
        None,
        vec!["gravity".to_string(), "mass".to_string()],
    ));

    store.save_session(&session).await.unwrap();

    let conn = store.conn.lock().unwrap();
    let concept_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM concepts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(concept_count, 2);

    let link_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM turn_concepts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(link_count, 2);

    // The concept names round-trip via the lookup.
    let mut stmt = conn
        .prepare(
            "SELECT c.name FROM turn_concepts tc
                 JOIN concepts c ON c.id = tc.concept_id
                 ORDER BY c.name",
        )
        .unwrap();
    let names: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(names, vec!["gravity".to_string(), "mass".to_string()]);
}

#[tokio::test]
async fn save_session_dedups_concepts_across_turns() {
    use primer_core::conversation::Speaker;

    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    // "gravity" appears in two turns — should be one concepts row, two turn_concepts rows.
    session.add_turn(make_turn(
        Speaker::Child,
        "what is gravity",
        None,
        vec!["gravity".to_string()],
    ));
    session.add_turn(make_turn(
        Speaker::Primer,
        "good question",
        None,
        vec!["gravity".to_string()],
    ));

    store.save_session(&session).await.unwrap();

    let conn = store.conn.lock().unwrap();
    let concept_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM concepts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(concept_count, 1, "gravity should be one concept row");

    let link_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM turn_concepts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(link_count, 2, "gravity should be linked to both turns");
}

#[tokio::test]
async fn idempotent_re_save_does_not_duplicate_rows() {
    use primer_core::conversation::Speaker;

    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(
        Speaker::Child,
        "hi",
        None,
        vec!["greeting".to_string()],
    ));

    store.save_session(&session).await.unwrap();
    store.save_session(&session).await.unwrap();

    let conn = store.conn.lock().unwrap();
    let session_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    let turn_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0))
        .unwrap();
    let concept_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM concepts", [], |r| r.get(0))
        .unwrap();
    let link_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM turn_concepts", [], |r| r.get(0))
        .unwrap();

    assert_eq!(session_count, 1);
    assert_eq!(turn_count, 1);
    assert_eq!(concept_count, 1);
    assert_eq!(link_count, 1);
}

#[tokio::test]
async fn append_a_turn_grows_the_persisted_session() {
    use primer_core::conversation::Speaker;

    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(Speaker::Child, "first", None, vec![]));
    store.save_session(&session).await.unwrap();
    session.add_turn(make_turn(Speaker::Primer, "second", None, vec![]));
    store.save_session(&session).await.unwrap();

    let conn = store.conn.lock().unwrap();
    let turn_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0))
        .unwrap();
    assert_eq!(turn_count, 2);

    let last_text: String = conn
        .query_row("SELECT text FROM turns WHERE turn_index = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(last_text, "second");
}

#[tokio::test]
async fn every_intent_variant_round_trips() {
    use primer_core::conversation::{PedagogicalIntent, Speaker};

    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    // One turn per intent variant.
    for &variant in PedagogicalIntent::ALL {
        session.add_turn(make_turn(Speaker::Primer, "_", Some(variant), vec![]));
    }
    store.save_session(&session).await.unwrap();

    let conn = store.conn.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT intent_id FROM turns ORDER BY turn_index")
        .unwrap();
    let ids: Vec<i64> = stmt
        .query_map([], |r| r.get::<_, Option<i64>>(0))
        .unwrap()
        .map(|r| r.unwrap().unwrap())
        .collect();
    assert_eq!(ids.len(), PedagogicalIntent::ALL.len());

    // Every persisted id must reverse-map to the original variant.
    let mut variants_seen = Vec::new();
    for id in ids {
        let v = crate::catalog::intent_from_id(id).expect("known id");
        variants_seen.push(v);
    }
    let expected: Vec<PedagogicalIntent> = PedagogicalIntent::ALL.to_vec();
    assert_eq!(variants_seen, expected);
}

#[tokio::test]
async fn round_trip_turn_with_intent_suggest_break() {
    // Pinned single-variant test for the SuggestBreak intent.
    // Catches a future regression where the catalog id mapping
    // drifts away from the variant order.
    use primer_core::conversation::{PedagogicalIntent, Speaker};

    let path = tempfile_path();
    let store =
        SqliteSessionStore::open_for_locale(&path, primer_core::i18n::Locale::default()).unwrap();

    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(Turn {
        speaker: Speaker::Primer,
        text: "Want to take a break?".into(),
        timestamp: Utc::now(),
        intent: Some(PedagogicalIntent::SuggestBreak),
        concepts: vec![],
    });

    store.save_session(&session).await.unwrap();
    let loaded = store.load_session(session.id).await.unwrap().unwrap();

    assert_eq!(loaded.turns.len(), 1);
    assert_eq!(
        loaded.turns[0].intent,
        Some(PedagogicalIntent::SuggestBreak),
    );
}

#[tokio::test]
async fn deleting_session_cascades_turns_and_links_but_keeps_concepts() {
    use primer_core::conversation::Speaker;

    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(
        Speaker::Child,
        "what is gravity",
        None,
        vec!["gravity".to_string()],
    ));
    store.save_session(&session).await.unwrap();

    let conn = store.conn.lock().unwrap();
    // Pre-conditions: rows exist in all three tables we expect to touch.
    let pre_turns: i64 = conn
        .query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0))
        .unwrap();
    let pre_links: i64 = conn
        .query_row("SELECT COUNT(*) FROM turn_concepts", [], |r| r.get(0))
        .unwrap();
    let pre_concepts: i64 = conn
        .query_row("SELECT COUNT(*) FROM concepts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(pre_turns, 1);
    assert_eq!(pre_links, 1);
    assert_eq!(pre_concepts, 1);

    // Delete the session row directly. The schema's ON DELETE CASCADE
    // should propagate through turns → turn_concepts. The concepts row
    // is intentionally session-agnostic and should remain.
    conn.execute(
        "DELETE FROM sessions WHERE id = ?1",
        rusqlite::params![session.id.to_string()],
    )
    .unwrap();

    let post_turns: i64 = conn
        .query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0))
        .unwrap();
    let post_links: i64 = conn
        .query_row("SELECT COUNT(*) FROM turn_concepts", [], |r| r.get(0))
        .unwrap();
    let post_concepts: i64 = conn
        .query_row("SELECT COUNT(*) FROM concepts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(post_turns, 0, "turns should cascade-delete");
    assert_eq!(post_links, 0, "turn_concepts should cascade-delete");
    assert_eq!(
        post_concepts, 1,
        "concept rows are not session-scoped and should remain"
    );
}

#[test]
fn foreign_key_enforcement_rejects_unknown_speaker_id() {
    // Proves that PRAGMA foreign_keys = ON is honoured at write time,
    // not just queryable as a flag. Inserting a turn with an unknown
    // speaker_id must fail.
    let store = open_memory();
    let session_id = Uuid::new_v4().to_string();
    let conn = store.conn.lock().unwrap();
    // Insert a session row first so the turn's session_id FK is satisfied.
    conn.execute(
        "INSERT INTO sessions (id, learner_id, started_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![
            &session_id,
            Uuid::new_v4().to_string(),
            "2026-04-30T00:00:00+00:00"
        ],
    )
    .unwrap();

    let result = conn.execute(
        "INSERT INTO turns (session_id, turn_index, speaker_id, text, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            &session_id,
            0_i64,
            9999_i64,
            "hi",
            "2026-04-30T00:00:00+00:00"
        ],
    );
    assert!(
        result.is_err(),
        "FK enforcement should reject speaker_id=9999"
    );
}

#[tokio::test]
async fn turn_ids_are_stable_across_appending_saves() {
    // Append-only writes mean a turn's auto-incremented `id` should
    // stay the same when a later save appends new turns. The previous
    // DELETE+INSERT scheme failed this — every save gave every turn a
    // fresh id. Future tables that FK into `turns.id` will rely on
    // this stability.
    use primer_core::conversation::Speaker;

    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(Speaker::Child, "first", None, vec![]));
    store.save_session(&session).await.unwrap();

    let id_before: i64 = {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT id FROM turns WHERE session_id = ?1 AND turn_index = 0",
            rusqlite::params![session.id.to_string()],
            |r| r.get(0),
        )
        .unwrap()
    };

    session.add_turn(make_turn(Speaker::Primer, "second", None, vec![]));
    store.save_session(&session).await.unwrap();

    let id_after: i64 = {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT id FROM turns WHERE session_id = ?1 AND turn_index = 0",
            rusqlite::params![session.id.to_string()],
            |r| r.get(0),
        )
        .unwrap()
    };

    assert_eq!(
        id_before, id_after,
        "turn 0's row id must not change when turn 1 is appended"
    );
}

/// Returns a unique tempfile path using a UUID to avoid collisions
/// between parallel test threads.
fn tempfile_path() -> PathBuf {
    std::env::temp_dir().join(format!("primer-storage-test-{}.db", uuid::Uuid::new_v4()))
}

// ─── v2 migration ────────────────────────────────────────────────

#[test]
fn apply_v2_migrations_rolls_back_on_failure() {
    // Inject a known failure mode: invoke the migration on a connection
    // where `sessions` exists (so the ALTERs succeed) but `turns` does
    // NOT exist (so the FTS backfill INSERT fails). With the migration
    // wrapped in a transaction, the column adds and the FTS table
    // creation must roll back, leaving the DB exactly as we found it.
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions (
                 id TEXT PRIMARY KEY,
                 learner_id TEXT NOT NULL,
                 started_at TEXT NOT NULL,
                 ended_at TEXT
             );",
    )
    .unwrap();
    // No `turns` table — backfill will fail.

    let result = crate::schema::apply_v2_migrations(&conn);
    assert!(result.is_err(), "expected backfill to fail without turns");

    // Pre-fix behaviour: each statement auto-commits, so `sessions.summary`
    // would already exist on disk despite the backfill failure. Post-fix:
    // the transaction rolls back, leaving sessions in its original shape.
    let summary_col_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'summary'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        summary_col_count, 0,
        "sessions.summary should have rolled back when backfill failed"
    );
    let fts_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='turn_text_fts'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        fts_count, 0,
        "turn_text_fts should have rolled back when backfill failed"
    );
}

#[test]
fn fresh_db_at_v2_has_summary_columns_and_fts_table() {
    let store = open_memory();
    let conn = store.conn.lock().unwrap();
    assert_eq!(
        conn.query_row::<i64, _, _>("PRAGMA user_version", [], |r| r.get(0))
            .unwrap(),
        crate::schema::USER_VERSION
    );
    // Summary columns are present.
    for col in &["summary", "summary_through_turn_index"] {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = ?1",
                [col],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "sessions.{col} should exist");
    }
    // FTS virtual table is present.
    let fts: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='turn_text_fts'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fts, 1);
}

#[test]
fn migrate_v1_db_with_turns_adds_columns_and_backfills_fts() {
    // Hand-roll a v1 DB on disk with a session and two turns. Then
    // open it via the store (which runs the v2 migration in place)
    // and verify the new columns exist with default values, the FTS
    // table is populated, and the original turn rows are intact.
    let tmp = tempfile_path();
    let session_id = Uuid::new_v4().to_string();
    let learner_id = Uuid::new_v4().to_string();
    {
        let conn = Connection::open(&tmp).unwrap();
        conn.execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE speakers (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
                 CREATE TABLE pedagogical_intents (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
                 CREATE TABLE concepts (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE);
                 CREATE TABLE sessions (id TEXT PRIMARY KEY, learner_id TEXT NOT NULL,
                     started_at TEXT NOT NULL, ended_at TEXT);
                 CREATE TABLE turns (id INTEGER PRIMARY KEY AUTOINCREMENT,
                     session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                     turn_index INTEGER NOT NULL, speaker_id INTEGER NOT NULL REFERENCES speakers(id),
                     text TEXT NOT NULL, timestamp TEXT NOT NULL,
                     intent_id INTEGER REFERENCES pedagogical_intents(id),
                     UNIQUE(session_id, turn_index));
                 CREATE TABLE turn_concepts (turn_id INTEGER NOT NULL REFERENCES turns(id) ON DELETE CASCADE,
                     concept_id INTEGER NOT NULL REFERENCES concepts(id),
                     PRIMARY KEY(turn_id, concept_id));
                 INSERT INTO speakers (id, name) VALUES (1, 'Child'), (2, 'Primer');
                 INSERT INTO pedagogical_intents (id, name) VALUES
                     (1,'SocraticQuestion'),(2,'ComprehensionCheck'),(3,'Scaffolding'),
                     (4,'Encouragement'),(5,'Extension'),(6,'DirectAnswer'),
                     (7,'AnswerThenPivot'),(8,'SessionClose');
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, learner_id, started_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![&session_id, &learner_id, "2026-04-30T00:00:00+00:00"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO turns (session_id, turn_index, speaker_id, text, timestamp)
                 VALUES (?1, 0, 1, 'why is the sky blue', '2026-04-30T00:00:00+00:00'),
                        (?1, 1, 2, 'what colour is the sky?', '2026-04-30T00:00:01+00:00')",
            rusqlite::params![&session_id],
        )
        .unwrap();
    }

    // Now open via the store. v2 migration runs in place.
    let store =
        SqliteSessionStore::open_for_locale(&tmp, primer_core::i18n::Locale::default()).unwrap();
    let conn = store.conn.lock().unwrap();

    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, crate::schema::USER_VERSION);

    // Summary columns exist with default values.
    let (summary, through): (String, i64) = conn
        .query_row(
            "SELECT summary, summary_through_turn_index FROM sessions WHERE id = ?1",
            rusqlite::params![&session_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(summary, "");
    assert_eq!(through, 0);

    // FTS table is populated from existing turns.
    let fts_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM turn_text_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(fts_count, 2, "FTS index should be backfilled from turns");

    // Original turn rows are untouched.
    let turn_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0))
        .unwrap();
    assert_eq!(turn_count, 2);

    drop(conn);
    drop(store);
    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
async fn inserting_a_turn_updates_fts_index() {
    use primer_core::conversation::Speaker;
    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(
        Speaker::Child,
        "supercalifragilistic",
        None,
        vec![],
    ));
    store.save_session(&session).await.unwrap();
    let conn = store.conn.lock().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM turn_text_fts WHERE text MATCH ?1",
            ["\"supercalifragilistic\""],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "trigger should have inserted into FTS");
}

#[tokio::test]
async fn deleting_a_turn_removes_it_from_fts() {
    use primer_core::conversation::Speaker;
    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(Speaker::Child, "uniqueterm", None, vec![]));
    store.save_session(&session).await.unwrap();
    let conn = store.conn.lock().unwrap();
    // Cascade-delete via the session row (mimics what an admin would do).
    conn.execute(
        "DELETE FROM sessions WHERE id = ?1",
        rusqlite::params![session.id.to_string()],
    )
    .unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM turn_text_fts WHERE text MATCH ?1",
            ["\"uniqueterm\""],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "trigger should have removed the row from FTS");
}

// ─── load_session ────────────────────────────────────────────────

#[tokio::test]
async fn load_unknown_id_returns_none() {
    let store = open_memory();
    let result = store.load_session(Uuid::new_v4()).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn save_then_load_round_trips_empty_session_with_default_summary() {
    let store = open_memory();
    let session = Session::new(Uuid::new_v4());
    store.save_session(&session).await.unwrap();
    let loaded = store.load_session(session.id).await.unwrap().unwrap();
    assert_eq!(loaded.id, session.id);
    assert_eq!(loaded.learner_id, session.learner_id);
    assert!(loaded.ended_at.is_none());
    assert_eq!(loaded.turns.len(), 0);
    assert_eq!(loaded.summary, "");
    assert_eq!(loaded.summary_through_turn_index, 0);
}

#[tokio::test]
async fn save_then_load_round_trips_with_turns() {
    use primer_core::conversation::{PedagogicalIntent, Speaker};
    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(
        Speaker::Child,
        "why is the sky blue",
        None,
        vec![],
    ));
    session.add_turn(make_turn(
        Speaker::Primer,
        "What do you notice about the sky during the day?",
        Some(PedagogicalIntent::SocraticQuestion),
        vec![],
    ));
    store.save_session(&session).await.unwrap();
    let loaded = store.load_session(session.id).await.unwrap().unwrap();
    assert_eq!(loaded.turns.len(), 2);
    assert_eq!(loaded.turns[0].speaker, Speaker::Child);
    assert_eq!(loaded.turns[0].text, "why is the sky blue");
    assert!(loaded.turns[0].intent.is_none());
    assert_eq!(loaded.turns[1].speaker, Speaker::Primer);
    assert_eq!(
        loaded.turns[1].intent,
        Some(PedagogicalIntent::SocraticQuestion)
    );
}

#[tokio::test]
async fn load_preserves_turn_order_under_appending_saves() {
    use primer_core::conversation::Speaker;
    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(Speaker::Child, "first", None, vec![]));
    store.save_session(&session).await.unwrap();
    session.add_turn(make_turn(Speaker::Primer, "second", None, vec![]));
    store.save_session(&session).await.unwrap();
    session.add_turn(make_turn(Speaker::Child, "third", None, vec![]));
    store.save_session(&session).await.unwrap();
    let loaded = store.load_session(session.id).await.unwrap().unwrap();
    let texts: Vec<&str> = loaded.turns.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(texts, vec!["first", "second", "third"]);
}

#[tokio::test]
async fn load_preserves_concepts_per_turn() {
    use primer_core::conversation::Speaker;
    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(
        Speaker::Child,
        "tell me about gravity and mass",
        None,
        vec!["gravity".to_string(), "mass".to_string()],
    ));
    store.save_session(&session).await.unwrap();
    let loaded = store.load_session(session.id).await.unwrap().unwrap();
    let mut concepts = loaded.turns[0].concepts.clone();
    concepts.sort();
    assert_eq!(concepts, vec!["gravity".to_string(), "mass".to_string()]);
}

#[tokio::test]
async fn load_with_concept_shared_across_turns() {
    use primer_core::conversation::Speaker;
    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(
        Speaker::Child,
        "what is gravity",
        None,
        vec!["gravity".to_string()],
    ));
    session.add_turn(make_turn(
        Speaker::Primer,
        "What does gravity do?",
        None,
        vec!["gravity".to_string()],
    ));
    store.save_session(&session).await.unwrap();
    let loaded = store.load_session(session.id).await.unwrap().unwrap();
    assert_eq!(loaded.turns[0].concepts, vec!["gravity".to_string()]);
    assert_eq!(loaded.turns[1].concepts, vec!["gravity".to_string()]);
}

#[tokio::test]
async fn load_session_with_ended_at_round_trips() {
    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.ended_at = Some(Utc::now());
    store.save_session(&session).await.unwrap();
    let loaded = store.load_session(session.id).await.unwrap().unwrap();
    assert!(loaded.ended_at.is_some());
}

#[tokio::test]
async fn load_session_round_trips_summary_and_through_turn_index() {
    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.summary = "We have been talking about why the sky is blue.".to_string();
    session.summary_through_turn_index = 42;
    store.save_session(&session).await.unwrap();
    let loaded = store.load_session(session.id).await.unwrap().unwrap();
    assert_eq!(
        loaded.summary,
        "We have been talking about why the sky is blue."
    );
    assert_eq!(loaded.summary_through_turn_index, 42);
}

// ─── retrieve_session_turns ──────────────────────────────────────

#[tokio::test]
async fn retrieve_session_turns_returns_matching_turns() {
    use primer_core::conversation::Speaker;
    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(Speaker::Child, "I love kittens", None, vec![]));
    session.add_turn(make_turn(
        Speaker::Primer,
        "Tell me about gravity",
        None,
        vec![],
    ));
    session.add_turn(make_turn(
        Speaker::Child,
        "what causes lightning",
        None,
        vec![],
    ));
    store.save_session(&session).await.unwrap();
    let hits = store
        .retrieve_session_turns(session.id, "gravity", 10, 1000)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].text.contains("gravity"));
}

#[tokio::test]
async fn retrieve_session_turns_excludes_recent_window() {
    // The dialogue manager passes `exclude_indices_at_or_after` to
    // skip turns the model already sees in the active window.
    use primer_core::conversation::Speaker;
    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    // Three turns, all mention "lightning".
    session.add_turn(make_turn(Speaker::Child, "lightning early", None, vec![]));
    session.add_turn(make_turn(Speaker::Primer, "lightning middle", None, vec![]));
    session.add_turn(make_turn(Speaker::Child, "lightning late", None, vec![]));
    store.save_session(&session).await.unwrap();
    // Exclude index >= 1: only the first turn ("early") qualifies.
    let hits = store
        .retrieve_session_turns(session.id, "lightning", 10, 1)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].text, "lightning early");
}

#[tokio::test]
async fn retrieve_session_turns_returns_empty_when_no_match() {
    use primer_core::conversation::Speaker;
    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(Speaker::Child, "kittens are nice", None, vec![]));
    store.save_session(&session).await.unwrap();
    let hits = store
        .retrieve_session_turns(session.id, "supernova", 10, 1000)
        .await
        .unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn retrieve_session_turns_handles_quotes_and_special_chars() {
    // FTS5-special characters and reserved keywords in the input
    // must not be interpreted as operators. Hostile chars get
    // stripped, reserved tokens (`OR`, `NEAR`, ...) are dropped,
    // surviving content tokens are quoted and ANDed — meaningful
    // words still match the indexed turn.
    use primer_core::conversation::Speaker;
    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(Speaker::Child, "what is plasma", None, vec![]));
    store.save_session(&session).await.unwrap();
    let hostile = "plasma what \" * OR ^col: NEAR/2";
    let hits = store
        .retrieve_session_turns(session.id, hostile, 10, 1000)
        .await
        .unwrap();
    assert!(
        !hits.is_empty(),
        "tokens 'plasma' and 'what' survive sanitization and should match"
    );
}

#[tokio::test]
async fn retrieve_session_turns_drops_only_reserved_tokens() {
    // A query that is *nothing but* FTS5 keywords + special chars
    // must not produce a query that matches everything; it must
    // produce an empty result via the empty-phrase short-circuit.
    let store = open_memory();
    let session = Session::new(Uuid::new_v4());
    store.save_session(&session).await.unwrap();
    let hits = store
        .retrieve_session_turns(session.id, "AND OR NOT NEAR \" * ^", 10, 1000)
        .await
        .unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn retrieve_session_turns_empty_query_returns_empty() {
    let store = open_memory();
    let session = Session::new(Uuid::new_v4());
    store.save_session(&session).await.unwrap();
    let hits = store
        .retrieve_session_turns(session.id, "   ", 10, 1000)
        .await
        .unwrap();
    assert!(hits.is_empty());
}

// ─── v3 migration ────────────────────────────────────────────────

#[test]
fn apply_v3_migrations_creates_all_three_tables() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    // Set up v2 baseline: existing schema must be valid before v3 runs.
    conn.execute_batch(crate::schema::SCHEMA_SQL).unwrap();
    crate::schema::apply_v2_migrations(&conn).unwrap();

    crate::schema::apply_v3_migrations(&conn).unwrap();

    for table in ["engagement_states", "classifiers", "turn_classifications"] {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "{table} not created by v3 migration");
    }
}

#[test]
fn apply_v3_migrations_is_idempotent() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(crate::schema::SCHEMA_SQL).unwrap();
    crate::schema::apply_v2_migrations(&conn).unwrap();
    crate::schema::apply_v3_migrations(&conn).unwrap();
    crate::schema::apply_v3_migrations(&conn).unwrap(); // second call must succeed
}

#[test]
fn apply_v3_migrations_creates_index_on_turn_classifications() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(crate::schema::SCHEMA_SQL).unwrap();
    crate::schema::apply_v2_migrations(&conn).unwrap();
    crate::schema::apply_v3_migrations(&conn).unwrap();

    let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_turn_classifications_turn_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn user_version_is_eight() {
    assert_eq!(crate::schema::USER_VERSION, 8);
}

// ─── save_classification / load_recent_assessments ───────────────

#[tokio::test]
async fn save_classification_persists_to_table() {
    use primer_core::classifier::EngagementAssessment;
    use primer_core::conversation::Speaker;
    use primer_core::learner::EngagementState;
    use primer_core::storage::SessionStore;

    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(Turn {
        speaker: Speaker::Child,
        text: "what is gravity?".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    store.save_session(&session).await.unwrap();

    let assessment = EngagementAssessment {
        state: EngagementState::Engaged,
        confidence: 0.92,
        reasoning: Some("child curious".into()),
    };
    store
        .save_classification(session.id, 0, &assessment, "stub")
        .await
        .unwrap();

    let loaded = store
        .load_recent_assessments(session.id, "stub", 10)
        .await
        .unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].state, EngagementState::Engaged);
    assert!((loaded[0].confidence - 0.92).abs() < 1e-6);
    assert_eq!(loaded[0].reasoning.as_deref(), Some("child curious"));
}

#[tokio::test]
async fn save_classification_handles_null_reasoning() {
    use primer_core::classifier::EngagementAssessment;
    use primer_core::conversation::Speaker;
    use primer_core::learner::EngagementState;
    use primer_core::storage::SessionStore;

    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(Turn {
        speaker: Speaker::Child,
        text: "ok".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    store.save_session(&session).await.unwrap();

    let assessment = EngagementAssessment {
        state: EngagementState::Reflecting,
        confidence: 0.5,
        reasoning: None,
    };
    store
        .save_classification(session.id, 0, &assessment, "stub")
        .await
        .unwrap();

    let loaded = store
        .load_recent_assessments(session.id, "stub", 10)
        .await
        .unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].reasoning, None);
}

#[tokio::test]
async fn save_classification_unique_constraint_fires_on_duplicate() {
    use primer_core::classifier::EngagementAssessment;
    use primer_core::conversation::Speaker;
    use primer_core::learner::EngagementState;
    use primer_core::storage::SessionStore;

    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(Turn {
        speaker: Speaker::Child,
        text: "x".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    store.save_session(&session).await.unwrap();

    let a = EngagementAssessment {
        state: EngagementState::Engaged,
        confidence: 0.5,
        reasoning: None,
    };
    store
        .save_classification(session.id, 0, &a, "stub")
        .await
        .unwrap();
    // Same classifier on same turn — must error (logic bug to surface).
    let err = store.save_classification(session.id, 0, &a, "stub").await;
    assert!(err.is_err());
}

#[tokio::test]
async fn load_recent_assessments_filters_by_classifier_identifier() {
    use primer_core::classifier::EngagementAssessment;
    use primer_core::conversation::Speaker;
    use primer_core::learner::EngagementState;
    use primer_core::storage::SessionStore;

    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(Turn {
        speaker: Speaker::Child,
        text: "x".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    store.save_session(&session).await.unwrap();

    let a = EngagementAssessment {
        state: EngagementState::Engaged,
        confidence: 0.5,
        reasoning: None,
    };
    store
        .save_classification(session.id, 0, &a, "stub")
        .await
        .unwrap();
    store
        .save_classification(session.id, 0, &a, "llm:haiku")
        .await
        .unwrap();

    let stub_only = store
        .load_recent_assessments(session.id, "stub", 10)
        .await
        .unwrap();
    assert_eq!(stub_only.len(), 1);

    let llm_only = store
        .load_recent_assessments(session.id, "llm:haiku", 10)
        .await
        .unwrap();
    assert_eq!(llm_only.len(), 1);
}

#[tokio::test]
async fn load_recent_assessments_respects_k_limit() {
    use primer_core::classifier::EngagementAssessment;
    use primer_core::conversation::Speaker;
    use primer_core::learner::EngagementState;
    use primer_core::storage::SessionStore;

    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    for i in 0..5 {
        session.add_turn(Turn {
            speaker: Speaker::Child,
            text: format!("t{i}"),
            timestamp: Utc::now(),
            intent: None,
            concepts: vec![],
        });
    }
    store.save_session(&session).await.unwrap();

    for i in 0..5usize {
        let confidence = 0.1 + (i as f32) * 0.1;
        let a = EngagementAssessment {
            state: EngagementState::Engaged,
            confidence,
            reasoning: None,
        };
        store
            .save_classification(session.id, i, &a, "stub")
            .await
            .unwrap();
        // Tiny sleep so classified_at is monotonic.
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }

    let loaded = store
        .load_recent_assessments(session.id, "stub", 2)
        .await
        .unwrap();
    assert_eq!(loaded.len(), 2);
    // Must be the most-recent two, ordered oldest-first within the result.
    assert!(
        (loaded[0].confidence - 0.4).abs() < 1e-6,
        "expected 0.4, got {}",
        loaded[0].confidence
    );
    assert!(
        (loaded[1].confidence - 0.5).abs() < 1e-6,
        "expected 0.5, got {}",
        loaded[1].confidence
    );
}

#[tokio::test]
async fn load_recent_assessments_returns_empty_when_no_classifications() {
    use primer_core::storage::SessionStore;

    let store = open_memory();
    let session = Session::new(Uuid::new_v4());
    store.save_session(&session).await.unwrap();
    let loaded = store
        .load_recent_assessments(session.id, "stub", 10)
        .await
        .unwrap();
    assert!(loaded.is_empty());
}

#[tokio::test]
async fn open_fresh_db_creates_v4_schema_with_understanding_depths_seeded() {
    let path = tempfile_path();
    let _store =
        SqliteSessionStore::open_for_locale(&path, primer_core::i18n::Locale::default()).unwrap();

    // Inspect the file directly via a raw rusqlite connection.
    let conn = Connection::open(&path).unwrap();
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, crate::schema::USER_VERSION);

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM understanding_depths", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        count, 6,
        "expected all 6 UnderstandingDepth variants seeded"
    );

    let learners: i64 = conn
        .query_row("SELECT COUNT(*) FROM learners", [], |r| r.get(0))
        .unwrap();
    assert_eq!(learners, 0, "open() must not insert any learners row");

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn fk_enforcement_rejects_unknown_depth_id_in_learner_concepts() {
    // Proves PRAGMA foreign_keys = ON covers the v4 tables too.
    // We cannot exercise this through save_learner (which only ever uses
    // valid depth_ids from the catalog), so reach for the underlying
    // connection and try to insert a row with depth_id = 99.
    let store = SqliteSessionStore::open_for_locale(
        std::path::Path::new(":memory:"),
        primer_core::i18n::Locale::default(),
    )
    .unwrap();

    // Pre-seed: a learners row + a concepts row, so the only FK that can
    // fail is depth_id.
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
            rusqlite::params!["00000000-0000-0000-0000-000000000001"],
        )
        .unwrap();
        conn.execute("INSERT INTO concepts (name) VALUES ('test:concept')", [])
            .unwrap();

        let result = conn.execute(
            "INSERT INTO learner_concepts (
                     learner_id, concept_id, depth_id, confidence,
                     encounter_count, last_encountered, notes
                 ) VALUES ('00000000-0000-0000-0000-000000000001',
                           (SELECT id FROM concepts WHERE name = 'test:concept'),
                           99, 0.5, 1, NULL, '[]')",
            [],
        );
        assert!(
            result.is_err(),
            "expected FK constraint failure on depth_id = 99"
        );
    }
}

#[tokio::test]
async fn most_recent_session_learner_id_returns_none_on_empty_db() {
    let store = SqliteSessionStore::open_for_locale(
        std::path::Path::new(":memory:"),
        primer_core::i18n::Locale::default(),
    )
    .unwrap();
    let result = store.most_recent_session_learner_id().await.unwrap();
    assert_eq!(result, None);
}

#[tokio::test]
async fn most_recent_session_learner_id_returns_single_existing_learner_id() {
    use primer_core::conversation::Session;

    let store = SqliteSessionStore::open_for_locale(
        std::path::Path::new(":memory:"),
        primer_core::i18n::Locale::default(),
    )
    .unwrap();
    let learner_id = uuid::Uuid::new_v4();
    let session = Session::new(learner_id);
    store.save_session(&session).await.unwrap();

    let result = store.most_recent_session_learner_id().await.unwrap();
    assert_eq!(result, Some(learner_id));
}

#[tokio::test]
async fn most_recent_session_learner_id_picks_most_recent_when_multiple_distinct() {
    use chrono::{Duration, Utc};
    use primer_core::conversation::Session;

    let store = SqliteSessionStore::open_for_locale(
        std::path::Path::new(":memory:"),
        primer_core::i18n::Locale::default(),
    )
    .unwrap();
    let older_learner = uuid::Uuid::new_v4();
    let newer_learner = uuid::Uuid::new_v4();

    let mut older = Session::new(older_learner);
    older.started_at = Utc::now() - Duration::hours(2);
    let mut newer = Session::new(newer_learner);
    newer.started_at = Utc::now();

    store.save_session(&older).await.unwrap();
    store.save_session(&newer).await.unwrap();

    let result = store.most_recent_session_learner_id().await.unwrap();
    assert_eq!(result, Some(newer_learner));
}

// ─── list_sessions ────────────────────────────────────────────────

#[tokio::test]
async fn list_sessions_returns_empty_on_empty_db() {
    let store = SqliteSessionStore::open_for_locale(
        std::path::Path::new(":memory:"),
        primer_core::i18n::Locale::default(),
    )
    .unwrap();
    let result = store.list_sessions().await.unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn list_sessions_includes_turn_count_and_summary() {
    use primer_core::conversation::{Session, Speaker, Turn};

    let store = open_memory();
    let learner_id = Uuid::new_v4();
    let mut session = Session::new(learner_id);
    session.summary = "discussed planets and gravity".into();
    session.summary_through_turn_index = 2;
    session.add_turn(Turn {
        speaker: Speaker::Child,
        text: "why does the moon orbit?".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    session.add_turn(Turn {
        speaker: Speaker::Primer,
        text: "what's pulling on it?".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    store.save_session(&session).await.unwrap();

    let listings = store.list_sessions().await.unwrap();
    assert_eq!(listings.len(), 1);
    assert_eq!(listings[0].id, session.id);
    assert_eq!(listings[0].learner_id, learner_id);
    assert_eq!(listings[0].turn_count, 2);
    assert_eq!(listings[0].summary, "discussed planets and gravity");
    assert!(listings[0].ended_at.is_none());
}

#[tokio::test]
async fn list_sessions_orders_by_last_activity_desc() {
    use chrono::{Duration, Utc};
    use primer_core::conversation::{Session, Speaker, Turn};

    let store = open_memory();
    let learner_id = Uuid::new_v4();

    let mut older = Session::new(learner_id);
    older.started_at = Utc::now() - Duration::hours(5);
    older.add_turn(Turn {
        speaker: Speaker::Child,
        text: "older question".into(),
        timestamp: Utc::now() - Duration::hours(4),
        intent: None,
        concepts: vec![],
    });
    store.save_session(&older).await.unwrap();

    let mut newer = Session::new(learner_id);
    newer.started_at = Utc::now() - Duration::hours(3);
    newer.add_turn(Turn {
        speaker: Speaker::Child,
        text: "newer question".into(),
        timestamp: Utc::now() - Duration::minutes(2),
        intent: None,
        concepts: vec![],
    });
    store.save_session(&newer).await.unwrap();

    let listings = store.list_sessions().await.unwrap();
    assert_eq!(listings.len(), 2);
    assert_eq!(
        listings[0].id, newer.id,
        "most-recent activity wins regardless of started_at"
    );
    assert_eq!(listings[1].id, older.id);
}

#[tokio::test]
async fn list_sessions_uses_started_at_when_no_turns() {
    // A session that has been opened but never had a turn must still
    // appear in the picker — sort key falls back to started_at.
    use primer_core::conversation::Session;

    let store = open_memory();
    let learner_id = Uuid::new_v4();
    let session = Session::new(learner_id);
    store.save_session(&session).await.unwrap();

    let listings = store.list_sessions().await.unwrap();
    assert_eq!(listings.len(), 1);
    assert_eq!(listings[0].turn_count, 0);
    assert_eq!(listings[0].last_activity, listings[0].started_at);
}

// ─── update_turn_concepts ─────────────────────────────────────────

#[tokio::test]
async fn update_turn_concepts_appends_to_persisted_turn() {
    let store = open_memory();
    let learner_id = Uuid::new_v4();
    let mut session = primer_core::conversation::Session::new(learner_id);
    session.add_turn(primer_core::conversation::Turn {
        speaker: primer_core::conversation::Speaker::Child,
        text: "what is photosynthesis?".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    store.save_session(&session).await.unwrap();

    store
        .update_turn_concepts(session.id, 0, &["photosynthesis".into(), "biology".into()])
        .await
        .unwrap();

    let loaded = store.load_session(session.id).await.unwrap().unwrap();
    let mut concepts = loaded.turns[0].concepts.clone();
    concepts.sort();
    assert_eq!(
        concepts,
        vec!["biology".to_string(), "photosynthesis".into()]
    );
}

#[tokio::test]
async fn update_turn_concepts_is_idempotent() {
    let store = open_memory();
    let learner_id = Uuid::new_v4();
    let mut session = primer_core::conversation::Session::new(learner_id);
    session.add_turn(primer_core::conversation::Turn {
        speaker: primer_core::conversation::Speaker::Child,
        text: "what is photosynthesis?".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    store.save_session(&session).await.unwrap();

    // Run twice with overlapping concepts — should not duplicate rows.
    store
        .update_turn_concepts(session.id, 0, &["photosynthesis".into()])
        .await
        .unwrap();
    store
        .update_turn_concepts(session.id, 0, &["photosynthesis".into(), "biology".into()])
        .await
        .unwrap();

    let loaded = store.load_session(session.id).await.unwrap().unwrap();
    assert_eq!(loaded.turns[0].concepts.len(), 2);
}

#[tokio::test]
async fn update_turn_concepts_empty_slice_is_noop() {
    let store = open_memory();
    let learner_id = Uuid::new_v4();
    let mut session = primer_core::conversation::Session::new(learner_id);
    session.add_turn(primer_core::conversation::Turn {
        speaker: primer_core::conversation::Speaker::Child,
        text: "hi".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    store.save_session(&session).await.unwrap();

    store
        .update_turn_concepts(session.id, 0, &[])
        .await
        .unwrap();

    let loaded = store.load_session(session.id).await.unwrap().unwrap();
    assert!(loaded.turns[0].concepts.is_empty());
}

#[tokio::test]
async fn update_turn_concepts_errors_on_missing_turn() {
    let store = open_memory();
    let unknown_session = Uuid::new_v4();
    let res = store
        .update_turn_concepts(unknown_session, 0, &["x".into()])
        .await;
    assert!(
        res.is_err(),
        "expected Err for unknown (session, turn_index)"
    );
}

// ─── update_exchange_concepts ─────────────────────────────────────

#[tokio::test]
async fn update_exchange_concepts_persists_both_turns_atomically() {
    let store = open_memory();
    let learner_id = Uuid::new_v4();
    let mut session = primer_core::conversation::Session::new(learner_id);
    session.add_turn(primer_core::conversation::Turn {
        speaker: primer_core::conversation::Speaker::Child,
        text: "what is photosynthesis?".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    session.add_turn(primer_core::conversation::Turn {
        speaker: primer_core::conversation::Speaker::Primer,
        text: "great question!".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    store.save_session(&session).await.unwrap();

    store
        .update_exchange_concepts(
            session.id,
            0,
            &["photosynthesis".into()],
            1,
            &["chlorophyll".into(), "biology".into()],
        )
        .await
        .unwrap();

    let loaded = store.load_session(session.id).await.unwrap().unwrap();
    assert_eq!(loaded.turns[0].concepts, vec!["photosynthesis".to_string()]);
    let mut primer = loaded.turns[1].concepts.clone();
    primer.sort();
    assert_eq!(primer, vec!["biology".to_string(), "chlorophyll".into()]);
}

#[tokio::test]
async fn update_exchange_concepts_rolls_back_when_one_turn_missing() {
    let store = open_memory();
    let learner_id = Uuid::new_v4();
    let mut session = primer_core::conversation::Session::new(learner_id);
    session.add_turn(primer_core::conversation::Turn {
        speaker: primer_core::conversation::Speaker::Child,
        text: "hi".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    // Only ONE turn persisted: turn_index 1 (primer) doesn't exist.
    store.save_session(&session).await.unwrap();

    let res = store
        .update_exchange_concepts(
            session.id,
            0,
            &["should-not-persist".into()],
            1,
            &["also-no".into()],
        )
        .await;
    assert!(res.is_err(), "expected Err when primer turn is missing");

    // The child write must have been rolled back — no concepts on
    // turn 0 even though the call had a valid child_turn_index.
    let loaded = store.load_session(session.id).await.unwrap().unwrap();
    assert!(
        loaded.turns[0].concepts.is_empty(),
        "child write must roll back when primer write fails"
    );
}

#[tokio::test]
async fn update_exchange_concepts_skips_empty_slices() {
    let store = open_memory();
    let learner_id = Uuid::new_v4();
    let mut session = primer_core::conversation::Session::new(learner_id);
    session.add_turn(primer_core::conversation::Turn {
        speaker: primer_core::conversation::Speaker::Child,
        text: "x".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    session.add_turn(primer_core::conversation::Turn {
        speaker: primer_core::conversation::Speaker::Primer,
        text: "y".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    store.save_session(&session).await.unwrap();

    // Both empty → no-op.
    store
        .update_exchange_concepts(session.id, 0, &[], 1, &[])
        .await
        .unwrap();

    // One empty, other populated — populated side persists.
    store
        .update_exchange_concepts(session.id, 0, &["only-child".into()], 1, &[])
        .await
        .unwrap();

    let loaded = store.load_session(session.id).await.unwrap().unwrap();
    assert_eq!(loaded.turns[0].concepts, vec!["only-child".to_string()]);
    assert!(loaded.turns[1].concepts.is_empty());
}

#[tokio::test]
async fn update_exchange_concepts_dedupes_concept_lookup_across_turns() {
    // Same concept appears in BOTH child and primer concepts. The
    // per-call cache should resolve it once; the DB should still
    // link both turns. Asserting no duplicate `concepts` row exists
    // is the canary for the cache working correctly.
    let store = open_memory();
    let learner_id = Uuid::new_v4();
    let mut session = primer_core::conversation::Session::new(learner_id);
    session.add_turn(primer_core::conversation::Turn {
        speaker: primer_core::conversation::Speaker::Child,
        text: "x".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    session.add_turn(primer_core::conversation::Turn {
        speaker: primer_core::conversation::Speaker::Primer,
        text: "y".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    store.save_session(&session).await.unwrap();

    store
        .update_exchange_concepts(session.id, 0, &["gravity".into()], 1, &["gravity".into()])
        .await
        .unwrap();

    let loaded = store.load_session(session.id).await.unwrap().unwrap();
    assert_eq!(loaded.turns[0].concepts, vec!["gravity".to_string()]);
    assert_eq!(loaded.turns[1].concepts, vec!["gravity".to_string()]);
}

// ─── save_comprehensions ────────────────────────────────────────────

/// Helper used by the `save_comprehensions_*` tests: persists a
/// session with one Child turn at index 0 and one Primer turn at
/// index 1. Mirrors the inline pattern used by the
/// `update_exchange_concepts_*` tests.
fn make_two_turn_session() -> primer_core::conversation::Session {
    let mut session = primer_core::conversation::Session::new(Uuid::new_v4());
    session.add_turn(primer_core::conversation::Turn {
        speaker: primer_core::conversation::Speaker::Child,
        text: "what is photosynthesis?".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    session.add_turn(primer_core::conversation::Turn {
        speaker: primer_core::conversation::Speaker::Primer,
        text: "great question!".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    session
}

#[tokio::test]
async fn save_comprehensions_persists_one_row_per_concept() {
    use primer_core::comprehension::ComprehensionAssessment;
    use primer_core::learner::UnderstandingDepth;

    let store = open_memory();
    let session = make_two_turn_session();
    store.save_session(&session).await.unwrap();

    let assessments = vec![
        ComprehensionAssessment {
            concept: "photosynthesis".into(),
            depth: UnderstandingDepth::Comprehension,
            confidence: 0.85,
            evidence: Some("explained in own words".into()),
        },
        ComprehensionAssessment {
            concept: "chlorophyll".into(),
            depth: UnderstandingDepth::Aware,
            confidence: 0.6,
            evidence: None,
        },
    ];
    store
        .save_comprehensions(session.id, 1, &assessments, "llm:test:model")
        .await
        .unwrap();

    let count: i64 = store
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM turn_comprehensions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn save_comprehensions_unique_constraint_makes_resave_a_noop() {
    use primer_core::comprehension::ComprehensionAssessment;
    use primer_core::learner::UnderstandingDepth;

    let store = open_memory();
    let session = make_two_turn_session();
    store.save_session(&session).await.unwrap();

    let a = vec![ComprehensionAssessment {
        concept: "gravity".into(),
        depth: UnderstandingDepth::Recall,
        confidence: 0.7,
        evidence: None,
    }];
    store
        .save_comprehensions(session.id, 1, &a, "llm:test:model")
        .await
        .unwrap();
    // Same classifier + same turn + same concept → INSERT OR IGNORE
    // makes this a no-op.
    store
        .save_comprehensions(session.id, 1, &a, "llm:test:model")
        .await
        .unwrap();

    let count: i64 = store
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM turn_comprehensions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn save_comprehensions_empty_slice_is_noop() {
    let store = open_memory();
    let session = make_two_turn_session();
    store.save_session(&session).await.unwrap();

    store
        .save_comprehensions(session.id, 1, &[], "llm:test:model")
        .await
        .unwrap();

    let count: i64 = store
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM turn_comprehensions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn save_comprehensions_missing_turn_returns_err() {
    use primer_core::comprehension::ComprehensionAssessment;
    use primer_core::learner::UnderstandingDepth;

    let store = open_memory();
    let session = make_two_turn_session();
    store.save_session(&session).await.unwrap();

    let a = vec![ComprehensionAssessment {
        concept: "x".into(),
        depth: UnderstandingDepth::Aware,
        confidence: 0.5,
        evidence: None,
    }];
    let result = store
        .save_comprehensions(session.id, 99, &a, "llm:test:model")
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn save_comprehensions_classifier_lookup_populated_lazily() {
    use primer_core::comprehension::ComprehensionAssessment;
    use primer_core::learner::UnderstandingDepth;

    let store = open_memory();
    let session = make_two_turn_session();
    store.save_session(&session).await.unwrap();

    let a = vec![ComprehensionAssessment {
        concept: "x".into(),
        depth: UnderstandingDepth::Aware,
        confidence: 0.5,
        evidence: None,
    }];
    store
        .save_comprehensions(session.id, 1, &a, "llm:newmodel")
        .await
        .unwrap();

    let count: i64 = store
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM comprehension_classifiers WHERE identifier = 'llm:newmodel'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

// ─── i18n: concept_language_tag across INSERT sites (PR 5) ──────────
//
// Companion tests for the per-site coverage gap noted in the PR review.
// `save_learner` is already covered in `learner_store_tests`. The four
// tests below pin the same behaviour for the other INSERT paths so a
// regression introducing a sixth INSERT site without the tag would be
// caught locally rather than only via the cross-store precedence test.

fn open_memory_for_locale(locale: primer_core::i18n::Locale) -> SqliteSessionStore {
    SqliteSessionStore::open_for_locale(std::path::Path::new(":memory:"), locale)
        .expect("open :memory: for locale")
}

fn collect_concept_tags(store: &SqliteSessionStore) -> Vec<String> {
    let conn = store.conn.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT concept_language_tag FROM concepts ORDER BY id")
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

#[tokio::test]
async fn save_session_tags_concepts_with_store_locale() {
    use primer_core::conversation::Speaker;

    let store = open_memory_for_locale(primer_core::i18n::Locale::German);
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(
        Speaker::Child,
        "warum ist der Himmel blau",
        None,
        vec!["Himmel".into(), "Licht".into()],
    ));
    store.save_session(&session).await.unwrap();

    let tags = collect_concept_tags(&store);
    assert_eq!(tags.len(), 2, "expected two concept rows: {tags:?}");
    for t in &tags {
        assert_eq!(t, "de", "save_session must tag with store locale: {tags:?}");
    }
}

#[tokio::test]
async fn update_turn_concepts_tags_concepts_with_store_locale() {
    let store = open_memory_for_locale(primer_core::i18n::Locale::German);
    let mut session = primer_core::conversation::Session::new(Uuid::new_v4());
    session.add_turn(primer_core::conversation::Turn {
        speaker: primer_core::conversation::Speaker::Child,
        text: "was ist Photosynthese?".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    store.save_session(&session).await.unwrap();

    store
        .update_turn_concepts(session.id, 0, &["Photosynthese".into(), "Biologie".into()])
        .await
        .unwrap();

    let tags = collect_concept_tags(&store);
    assert_eq!(tags.len(), 2, "expected two concept rows: {tags:?}");
    for t in &tags {
        assert_eq!(
            t, "de",
            "update_turn_concepts must tag with store locale: {tags:?}"
        );
    }
}

#[tokio::test]
async fn update_exchange_concepts_tags_concepts_with_store_locale() {
    let store = open_memory_for_locale(primer_core::i18n::Locale::German);
    let session = make_two_turn_session();
    store.save_session(&session).await.unwrap();

    store
        .update_exchange_concepts(session.id, 0, &["Schwerkraft".into()], 1, &["Masse".into()])
        .await
        .unwrap();

    let tags = collect_concept_tags(&store);
    assert_eq!(tags.len(), 2, "expected two concept rows: {tags:?}");
    for t in &tags {
        assert_eq!(
            t, "de",
            "update_exchange_concepts must tag with store locale: {tags:?}"
        );
    }
}

#[tokio::test]
async fn save_comprehensions_tags_concepts_with_store_locale() {
    use primer_core::comprehension::ComprehensionAssessment;
    use primer_core::learner::UnderstandingDepth;

    let store = open_memory_for_locale(primer_core::i18n::Locale::German);
    let session = make_two_turn_session();
    store.save_session(&session).await.unwrap();

    let assessments = vec![
        ComprehensionAssessment {
            concept: "Photosynthese".into(),
            depth: UnderstandingDepth::Comprehension,
            confidence: 0.85,
            evidence: None,
        },
        ComprehensionAssessment {
            concept: "Chlorophyll".into(),
            depth: UnderstandingDepth::Aware,
            confidence: 0.6,
            evidence: None,
        },
    ];
    store
        .save_comprehensions(session.id, 1, &assessments, "llm:test:model")
        .await
        .unwrap();

    let tags = collect_concept_tags(&store);
    assert_eq!(tags.len(), 2, "expected two concept rows: {tags:?}");
    for t in &tags {
        assert_eq!(
            t, "de",
            "save_comprehensions must tag with store locale: {tags:?}"
        );
    }
}

// ─── embedding storage + hybrid retrieval (schema v8) ────────────

#[tokio::test]
async fn save_turn_embedding_round_trip() {
    use primer_core::conversation::Speaker;
    use primer_core::embedder::Embedder;
    use primer_embedding::StubEmbedder;
    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(Speaker::Child, "hello world", None, vec![]));
    store.save_session(&session).await.unwrap();

    let stub = StubEmbedder::with_dim(16);
    let v = stub.embed(&["hello world"]).await.unwrap().remove(0);
    store
        .save_turn_embedding(session.id, 0, stub.model_id(), stub.dim(), &v)
        .await
        .unwrap();

    let conn = store.conn.lock().unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM embeddings_turns", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn save_turn_embedding_overwrites_on_second_call() {
    use primer_core::conversation::Speaker;
    use primer_core::embedder::Embedder;
    use primer_embedding::StubEmbedder;
    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(Speaker::Child, "hello", None, vec![]));
    store.save_session(&session).await.unwrap();

    let stub = StubEmbedder::with_dim(16);
    let v1 = stub.embed(&["hello"]).await.unwrap().remove(0);
    let v2 = stub.embed(&["different text"]).await.unwrap().remove(0);
    store
        .save_turn_embedding(session.id, 0, stub.model_id(), stub.dim(), &v1)
        .await
        .unwrap();
    store
        .save_turn_embedding(session.id, 0, stub.model_id(), stub.dim(), &v2)
        .await
        .unwrap();

    let conn = store.conn.lock().unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM embeddings_turns", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "second save must upsert, not duplicate");
}

#[tokio::test]
async fn save_turn_embedding_rejects_dim_mismatch() {
    use primer_core::conversation::Speaker;
    use primer_core::embedder::Embedder;
    use primer_embedding::StubEmbedder;
    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(Speaker::Child, "hello", None, vec![]));
    store.save_session(&session).await.unwrap();

    let stub_a = StubEmbedder::with_dim(16);
    let v1 = stub_a.embed(&["hello"]).await.unwrap().remove(0);
    store
        .save_turn_embedding(session.id, 0, stub_a.model_id(), 16, &v1)
        .await
        .unwrap();

    // Same model_id (the stub uses a constant), different dim.
    let stub_b = StubEmbedder::with_dim(384);
    let v2 = stub_b.embed(&["hello"]).await.unwrap().remove(0);
    let result = store
        .save_turn_embedding(session.id, 0, stub_b.model_id(), 384, &v2)
        .await;
    assert!(
        matches!(result, Err(PrimerError::Storage(_))),
        "dim mismatch must hard-error, got {result:?}"
    );
}

#[tokio::test]
async fn retrieve_session_turns_hybrid_includes_bm25_hit() {
    use primer_core::conversation::Speaker;
    use primer_core::embedder::Embedder;
    use primer_core::knowledge::HybridParams;
    use primer_embedding::StubEmbedder;
    let store = open_memory();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(make_turn(Speaker::Child, "I love kittens", None, vec![]));
    session.add_turn(make_turn(
        Speaker::Primer,
        "tell me about gravity",
        None,
        vec![],
    ));
    session.add_turn(make_turn(
        Speaker::Child,
        "what causes lightning",
        None,
        vec![],
    ));
    store.save_session(&session).await.unwrap();

    let stub = StubEmbedder::with_dim(16);
    for (i, turn) in session.turns.iter().enumerate() {
        let v = stub.embed(&[turn.text.as_str()]).await.unwrap().remove(0);
        store
            .save_turn_embedding(session.id, i, stub.model_id(), stub.dim(), &v)
            .await
            .unwrap();
    }

    let hits = store
        .retrieve_session_turns_hybrid(
            session.id,
            "gravity",
            &stub,
            &HybridParams {
                bm25_top_k: 5,
                vector_top_k: 5,
                final_top_k: 3,
                rrf_k: 60.0,
                min_score: f64::NEG_INFINITY,
                source_filter: vec![],
            },
            1000,
        )
        .await
        .unwrap();
    assert!(!hits.is_empty(), "hybrid must return at least one hit");
    assert!(
        hits.iter().any(|t| t.text.contains("gravity")),
        "hybrid result must include the BM25-matching turn"
    );
}
