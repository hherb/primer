use super::*;
use crate::catalog::expected_understanding_depths;
use rusqlite::Connection;

fn fresh_v3_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(SCHEMA_SQL).unwrap();
    apply_v2_migrations(&conn).unwrap();
    apply_v3_migrations(&conn).unwrap();
    conn
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            rusqlite::params![name],
            |r| r.get(0),
        )
        .unwrap();
    count > 0
}

#[test]
fn apply_v4_migrations_creates_three_tables() {
    let conn = fresh_v3_conn();
    apply_v4_migrations(&conn).unwrap();
    assert!(table_exists(&conn, "understanding_depths"));
    assert!(table_exists(&conn, "learners"));
    assert!(table_exists(&conn, "learner_concepts"));
}

#[test]
fn apply_v4_migrations_is_idempotent() {
    let conn = fresh_v3_conn();
    apply_v4_migrations(&conn).unwrap();
    apply_v4_migrations(&conn).unwrap(); // second call must not fail or duplicate
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='learners'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn apply_v4_migrations_does_not_insert_learners_row() {
    let conn = fresh_v3_conn();
    apply_v4_migrations(&conn).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM learners", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0, "v4 migration must not insert any learners rows");
}

#[test]
fn understanding_depths_is_seeded_after_v4_with_validate_and_seed() {
    let conn = fresh_v3_conn();
    apply_v4_migrations(&conn).unwrap();
    validate_and_seed_lookup(
        &conn,
        "understanding_depths",
        &expected_understanding_depths(),
    )
    .unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM understanding_depths", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 6);
}

#[test]
fn user_version_constant_is_eight() {
    assert_eq!(USER_VERSION, 8);
}

#[test]
fn apply_v4_migrations_rolls_back_on_failure() {
    // Fault-injection: pre-create a TABLE colliding with the v4 index
    // name so the CREATE INDEX step fails, forcing the migration's
    // transaction to roll back the preceding CREATE TABLE statements.
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(SCHEMA_SQL).unwrap();
    apply_v2_migrations(&conn).unwrap();
    apply_v3_migrations(&conn).unwrap();
    conn.execute_batch("CREATE TABLE idx_learner_concepts_learner (id INTEGER PRIMARY KEY);")
        .unwrap();

    let result = apply_v4_migrations(&conn);
    assert!(result.is_err(), "expected migration to fail");

    // The transaction must have rolled back: learners,
    // learner_concepts, and understanding_depths must NOT exist.
    assert!(!table_exists(&conn, "learners"));
    assert!(!table_exists(&conn, "learner_concepts"));
    assert!(!table_exists(&conn, "understanding_depths"));
}

#[test]
fn v5_migration_creates_tables_and_indices() {
    let conn = Connection::open_in_memory().unwrap();
    // v1
    conn.execute_batch(SCHEMA_SQL).unwrap();
    // v2..v5 in order so the FKs (e.g. understanding_depths from v4) exist
    apply_v2_migrations(&conn).unwrap();
    apply_v3_migrations(&conn).unwrap();
    apply_v4_migrations(&conn).unwrap();
    apply_v5_migrations(&conn).unwrap();
    assert!(table_exists(&conn, "comprehension_classifiers"));
    assert!(table_exists(&conn, "turn_comprehensions"));
    // Indices exist (queryable through sqlite_master with type='index').
    let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name LIKE 'idx_turn_comprehensions_%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(idx_count, 2);
}

#[test]
fn v5_migration_is_idempotent_on_re_run() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SCHEMA_SQL).unwrap();
    apply_v2_migrations(&conn).unwrap();
    apply_v3_migrations(&conn).unwrap();
    apply_v4_migrations(&conn).unwrap();
    apply_v5_migrations(&conn).unwrap();
    // Running again must not error.
    apply_v5_migrations(&conn).unwrap();
    assert!(table_exists(&conn, "turn_comprehensions"));
}

fn fresh_v5_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(SCHEMA_SQL).unwrap();
    apply_v2_migrations(&conn).unwrap();
    apply_v3_migrations(&conn).unwrap();
    apply_v4_migrations(&conn).unwrap();
    apply_v5_migrations(&conn).unwrap();
    conn
}

fn column_exists_in_test(conn: &Connection, table: &str, column: &str) -> bool {
    let sql = format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1");
    let count: i64 = conn
        .query_row(&sql, rusqlite::params![column], |r| r.get(0))
        .unwrap();
    count > 0
}

#[test]
fn v6_migration_adds_learners_locale_column() {
    let conn = fresh_v5_conn();
    assert!(!column_exists_in_test(&conn, "learners", "locale"));
    apply_v6_migrations(&conn).unwrap();
    assert!(column_exists_in_test(&conn, "learners", "locale"));
}

#[test]
fn v6_migration_adds_concepts_language_tag_column() {
    let conn = fresh_v5_conn();
    assert!(!column_exists_in_test(
        &conn,
        "concepts",
        "concept_language_tag",
    ));
    apply_v6_migrations(&conn).unwrap();
    assert!(column_exists_in_test(
        &conn,
        "concepts",
        "concept_language_tag",
    ));
}

#[test]
fn v6_migration_defaults_existing_rows_to_en() {
    let conn = fresh_v5_conn();
    // Pre-seed a learners row and a concepts row prior to v6 migration.
    // engagement_states is empty (no validate-and-seed has run on this
    // bare connection) so insert the row we need first.
    conn.execute(
        "INSERT INTO engagement_states (id, name) VALUES (1, 'Engaged')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO learners (
                 id, name, age, languages, created_at, last_active,
                 pref_narrative, pref_socratic, pref_visual, pref_kinesthetic,
                 typical_session_minutes, high_engagement_topics,
                 early_disengagement_secs, current_engagement_state_id
             ) VALUES (?1, 'Tester', 8, '[]', '2026-01-01T00:00:00Z',
                      '2026-01-01T00:00:00Z', 0.5, 0.5, 0.5, 0.5, 20.0,
                      '[]', 300, 1)",
        rusqlite::params![uuid::Uuid::new_v4().to_string()],
    )
    .unwrap();
    conn.execute("INSERT INTO concepts (name) VALUES ('photosynthesis')", [])
        .unwrap();

    apply_v6_migrations(&conn).unwrap();

    let learner_locale: String = conn
        .query_row("SELECT locale FROM learners LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(learner_locale, "en");

    let concept_lang: String = conn
        .query_row(
            "SELECT concept_language_tag FROM concepts WHERE name = 'photosynthesis'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(concept_lang, "en");
}

#[test]
fn v6_migration_is_idempotent() {
    let conn = fresh_v5_conn();
    apply_v6_migrations(&conn).unwrap();
    apply_v6_migrations(&conn).unwrap(); // re-run must be a no-op
    assert!(column_exists_in_test(&conn, "learners", "locale"));
    assert!(column_exists_in_test(
        &conn,
        "concepts",
        "concept_language_tag",
    ));
}

fn fresh_v6_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute_batch(SCHEMA_SQL).unwrap();
    apply_v2_migrations(&conn).unwrap();
    apply_v3_migrations(&conn).unwrap();
    apply_v4_migrations(&conn).unwrap();
    apply_v5_migrations(&conn).unwrap();
    apply_v6_migrations(&conn).unwrap();
    conn
}

#[test]
fn v7_migration_adds_learner_concepts_box_level_column() {
    let conn = fresh_v6_conn();
    assert!(!column_exists_in_test(
        &conn,
        "learner_concepts",
        "box_level"
    ));
    apply_v7_migrations(&conn).unwrap();
    assert!(column_exists_in_test(
        &conn,
        "learner_concepts",
        "box_level"
    ));
}

#[test]
fn v7_migration_defaults_existing_rows_to_zero() {
    let conn = fresh_v6_conn();
    // Pre-seed a learners + concepts + learner_concepts row before v7.
    conn.execute(
        "INSERT INTO engagement_states (id, name) VALUES (1, 'Engaged')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO understanding_depths (id, name) VALUES (1, 'Aware')",
        [],
    )
    .unwrap();
    let learner_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO learners (
                 id, name, age, languages, created_at, last_active,
                 pref_narrative, pref_socratic, pref_visual, pref_kinesthetic,
                 typical_session_minutes, high_engagement_topics,
                 early_disengagement_secs, current_engagement_state_id, locale
             ) VALUES (?1, 'Tester', 8, '[]', '2026-01-01T00:00:00Z',
                      '2026-01-01T00:00:00Z', 0.5, 0.5, 0.5, 0.5, 20.0,
                      '[]', 300, 1, 'en')",
        rusqlite::params![learner_id],
    )
    .unwrap();
    conn.execute("INSERT INTO concepts (name) VALUES ('photosynthesis')", [])
        .unwrap();
    let concept_id: i64 = conn
        .query_row(
            "SELECT id FROM concepts WHERE name = 'photosynthesis'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO learner_concepts (
                 learner_id, concept_id, depth_id, confidence,
                 encounter_count, last_encountered, notes
             ) VALUES (?1, ?2, 1, 0.5, 1, NULL, '[]')",
        rusqlite::params![learner_id, concept_id],
    )
    .unwrap();

    apply_v7_migrations(&conn).unwrap();

    let box_level: i64 = conn
        .query_row(
            "SELECT box_level FROM learner_concepts WHERE learner_id = ?1",
            rusqlite::params![learner_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(box_level, 0);
}

#[test]
fn v7_migration_is_idempotent() {
    let conn = fresh_v6_conn();
    apply_v7_migrations(&conn).unwrap();
    apply_v7_migrations(&conn).unwrap(); // re-run must be a no-op
    assert!(column_exists_in_test(
        &conn,
        "learner_concepts",
        "box_level"
    ));
}
