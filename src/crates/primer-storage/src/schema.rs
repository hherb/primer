//! Database schema definitions and the validate-and-seed helper for
//! lookup tables.

use std::collections::HashMap;

use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;

/// The on-disk schema version this build understands. Stored in
/// `PRAGMA user_version`. A mismatch on `open()` is a hard error.
///
/// Bumped to 2 when we added the rolling-summary fields on `sessions`
/// and the `turn_text_fts` virtual table for searchable session memory.
/// `open()` migrates v1 databases in place; the migration is purely
/// additive (column adds + new objects), so existing data is preserved.
///
/// Bumped to 3 when we added the `engagement_states`, `classifiers`,
/// and `turn_classifications` tables that back the engagement-classifier
/// feature (Phase 0.3).
///
/// Bumped to 4 when we added the `understanding_depths` lookup table
/// plus `learners` and `learner_concepts` tables that back learner-model
/// SQLite persistence (Phase 0.3). Schema-only — adoption of an existing
/// session's `learner_id` happens at the CLI layer, not in this migration.
pub const USER_VERSION: i64 = 4;

/// Idempotent CREATE statements for the base (v1) schema. Run on every
/// `open()`. v2-specific objects are added by `apply_v2_migrations`.
pub const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS speakers (
    id    INTEGER PRIMARY KEY,
    name  TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS pedagogical_intents (
    id    INTEGER PRIMARY KEY,
    name  TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS concepts (
    id    INTEGER PRIMARY KEY AUTOINCREMENT,
    name  TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS sessions (
    id          TEXT PRIMARY KEY,
    learner_id  TEXT NOT NULL,
    started_at  TEXT NOT NULL,
    ended_at    TEXT
);
CREATE INDEX IF NOT EXISTS idx_sessions_learner
    ON sessions(learner_id, started_at);

CREATE TABLE IF NOT EXISTS turns (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    turn_index  INTEGER NOT NULL,
    speaker_id  INTEGER NOT NULL REFERENCES speakers(id),
    text        TEXT NOT NULL,
    timestamp   TEXT NOT NULL,
    intent_id   INTEGER REFERENCES pedagogical_intents(id),
    UNIQUE(session_id, turn_index)
);
CREATE INDEX IF NOT EXISTS idx_turns_session
    ON turns(session_id, turn_index);

CREATE TABLE IF NOT EXISTS turn_concepts (
    turn_id     INTEGER NOT NULL REFERENCES turns(id) ON DELETE CASCADE,
    concept_id  INTEGER NOT NULL REFERENCES concepts(id),
    PRIMARY KEY(turn_id, concept_id)
);
CREATE INDEX IF NOT EXISTS idx_turn_concepts_concept
    ON turn_concepts(concept_id);
"#;

/// Apply v2 migrations idempotently. Safe to run on a fresh DB (after
/// `SCHEMA_SQL` has created the v1 tables), on a v1 DB being upgraded,
/// and on a v2 DB being re-opened.
///
/// All steps run inside a single transaction so a partial failure (e.g.
/// disk full between the FTS create and a trigger create) rolls back to
/// the pre-migration state instead of leaving an inconsistent half-v2
/// database that subsequent saves would silently miswrite to.
///
/// v2 adds:
/// - `sessions.summary` and `sessions.summary_through_turn_index` —
///   rolling LLM-generated summary of pre-window turns.
/// - `turn_text_fts` virtual table for FTS5 retrieval over `turns.text`.
/// - Triggers to keep `turn_text_fts` in sync with `turns`.
pub fn apply_v2_migrations(conn: &Connection) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| PrimerError::Storage(format!("begin v2 migration tx: {e}")))?;

    if !column_exists(&tx, "sessions", "summary")? {
        tx.execute_batch("ALTER TABLE sessions ADD COLUMN summary TEXT NOT NULL DEFAULT '';")
            .map_err(|e| PrimerError::Storage(format!("ALTER sessions ADD summary: {e}")))?;
    }
    if !column_exists(&tx, "sessions", "summary_through_turn_index")? {
        tx.execute_batch(
            "ALTER TABLE sessions ADD COLUMN summary_through_turn_index INTEGER NOT NULL DEFAULT 0;",
        )
        .map_err(|e| {
            PrimerError::Storage(format!(
                "ALTER sessions ADD summary_through_turn_index: {e}"
            ))
        })?;
    }

    // Detect whether the FTS index existed BEFORE we attempt the CREATE.
    // If we are creating it for the first time, backfill from `turns`;
    // otherwise the existing index is already kept in sync by the
    // triggers and a backfill would just duplicate rows.
    let fts_existed = table_exists(&tx, "turn_text_fts")?;

    tx.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS turn_text_fts USING fts5(\
            text, content='turns', content_rowid='id', tokenize='porter unicode61');",
    )
    .map_err(|e| PrimerError::Storage(format!("create turn_text_fts: {e}")))?;

    if !fts_existed {
        tx.execute_batch("INSERT INTO turn_text_fts(rowid, text) SELECT id, text FROM turns;")
            .map_err(|e| PrimerError::Storage(format!("backfill turn_text_fts: {e}")))?;
    }

    // Triggers keep the FTS index in sync as turns are inserted, deleted,
    // or updated. `IF NOT EXISTS` makes them idempotent across re-opens.
    tx.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS turns_ai AFTER INSERT ON turns BEGIN
             INSERT INTO turn_text_fts(rowid, text) VALUES (new.id, new.text);
         END;
         CREATE TRIGGER IF NOT EXISTS turns_ad AFTER DELETE ON turns BEGIN
             INSERT INTO turn_text_fts(turn_text_fts, rowid, text)
                 VALUES ('delete', old.id, old.text);
         END;
         CREATE TRIGGER IF NOT EXISTS turns_au AFTER UPDATE ON turns BEGIN
             INSERT INTO turn_text_fts(turn_text_fts, rowid, text)
                 VALUES ('delete', old.id, old.text);
             INSERT INTO turn_text_fts(rowid, text) VALUES (new.id, new.text);
         END;",
    )
    .map_err(|e| PrimerError::Storage(format!("create FTS triggers: {e}")))?;

    tx.commit()
        .map_err(|e| PrimerError::Storage(format!("commit v2 migration: {e}")))?;
    Ok(())
}

// ─── v3 schema strings ──────────────────────────────────────────────────────

const CREATE_ENGAGEMENT_STATES_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS engagement_states (
        id   INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE
    )
";

const CREATE_CLASSIFIERS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS classifiers (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        identifier TEXT NOT NULL UNIQUE
    )
";

const CREATE_TURN_CLASSIFICATIONS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS turn_classifications (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        turn_id             INTEGER NOT NULL REFERENCES turns(id),
        engagement_state_id INTEGER NOT NULL REFERENCES engagement_states(id),
        classifier_id       INTEGER NOT NULL REFERENCES classifiers(id),
        confidence          REAL NOT NULL,
        reasoning           TEXT,
        classified_at       TIMESTAMP NOT NULL,
        UNIQUE(turn_id, classifier_id)
    )
";

const CREATE_TURN_CLASSIFICATIONS_INDEX: &str = "
    CREATE INDEX IF NOT EXISTS idx_turn_classifications_turn_id
        ON turn_classifications(turn_id)
";

/// Apply v3 migrations idempotently. Safe to run on a fresh DB (after
/// v2 objects exist), on a v2 DB being upgraded, and on a v3 DB being
/// re-opened.
///
/// All steps run inside a single transaction so a partial failure rolls
/// back to the pre-migration state.
///
/// v3 adds:
/// - `engagement_states` lookup table (seeded by Task 7 validate pass).
/// - `classifiers` table for named classifier identifiers.
/// - `turn_classifications` junction table recording per-turn engagement
///   state labels (FK into `turns`, `engagement_states`, `classifiers`).
/// - `idx_turn_classifications_turn_id` index on the junction table.
pub(crate) fn apply_v3_migrations(conn: &Connection) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| PrimerError::Storage(format!("v3 migration: failed to begin tx: {e}")))?;
    tx.execute(CREATE_ENGAGEMENT_STATES_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v3 migration: engagement_states: {e}")))?;
    tx.execute(CREATE_CLASSIFIERS_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v3 migration: classifiers: {e}")))?;
    tx.execute(CREATE_TURN_CLASSIFICATIONS_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v3 migration: turn_classifications: {e}")))?;
    tx.execute(CREATE_TURN_CLASSIFICATIONS_INDEX, [])
        .map_err(|e| PrimerError::Storage(format!("v3 migration: index: {e}")))?;
    tx.commit()
        .map_err(|e| PrimerError::Storage(format!("v3 migration: commit: {e}")))?;
    Ok(())
}

// ─── v4 schema strings ──────────────────────────────────────────────────────
//
// Note on free-text `Vec<String>` columns (`learners.languages`,
// `learners.high_engagement_topics`, `learner_concepts.notes`): these are
// stored as JSON-encoded TEXT, not normalised into a lookup table.
//
// The "categorical text → lookup table" rule (CLAUDE.md) targets *closed*
// vocabularies where a Rust enum is the source of truth (`Speaker`,
// `PedagogicalIntent`, `EngagementState`, `UnderstandingDepth`). The three
// columns above are open-vocabulary, free-text lists owned by the learner
// (preferred languages, high-engagement topic phrases, free-form per-concept
// notes from the dialogue manager). Normalising them would buy nothing the
// `concepts` table doesn't already prove — they aren't FK targets, aren't
// queried by exact match, aren't shared across rows, and aren't bounded.
// JSON-in-TEXT keeps the schema flat and the round-trip lossless.

const CREATE_UNDERSTANDING_DEPTHS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS understanding_depths (
        id   INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE
    )
";

const CREATE_LEARNERS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS learners (
        id                          TEXT PRIMARY KEY,
        name                        TEXT NOT NULL,
        age                         INTEGER NOT NULL,
        languages                   TEXT NOT NULL,
        created_at                  TEXT NOT NULL,
        last_active                 TEXT NOT NULL,
        pref_narrative              REAL NOT NULL,
        pref_socratic               REAL NOT NULL,
        pref_visual                 REAL NOT NULL,
        pref_kinesthetic            REAL NOT NULL,
        typical_session_minutes     REAL NOT NULL,
        high_engagement_topics      TEXT NOT NULL,
        early_disengagement_secs    INTEGER NOT NULL,
        current_engagement_state_id INTEGER NOT NULL REFERENCES engagement_states(id)
    )
";

const CREATE_LEARNER_CONCEPTS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS learner_concepts (
        learner_id        TEXT NOT NULL REFERENCES learners(id) ON DELETE CASCADE,
        concept_id        INTEGER NOT NULL REFERENCES concepts(id),
        depth_id          INTEGER NOT NULL REFERENCES understanding_depths(id),
        confidence        REAL NOT NULL,
        encounter_count   INTEGER NOT NULL,
        last_encountered  TEXT,
        notes             TEXT NOT NULL DEFAULT '[]',
        PRIMARY KEY (learner_id, concept_id)
    )
";

const CREATE_LEARNER_CONCEPTS_INDEX: &str = "
    CREATE INDEX IF NOT EXISTS idx_learner_concepts_learner
        ON learner_concepts(learner_id)
";

/// Apply v4 migrations idempotently. Safe to run on a fresh DB (after
/// v3 objects exist), on a v3 DB being upgraded, and on a v4 DB being
/// re-opened.
///
/// All steps run inside a single transaction so a partial failure rolls
/// back to the pre-migration state.
///
/// v4 adds:
/// - `understanding_depths` lookup table (seeded by the validate pass
///   in `open()` after this migration runs).
/// - `learners` table — one row per learner DB file (application-level
///   invariant), holds profile + preferences + engagement snapshot.
/// - `learner_concepts` junction table for per-learner concept-mastery
///   state, FK'd into `learners`, `concepts`, and `understanding_depths`.
/// - `idx_learner_concepts_learner` index on the junction table.
///
/// Schema-only. Adopting an existing session's `learner_id` for the new
/// learners row is the CLI's responsibility — `apply_v4_migrations` runs
/// without CLI flag access and cannot populate `name` / `age`.
pub(crate) fn apply_v4_migrations(conn: &Connection) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| PrimerError::Storage(format!("v4 migration: failed to begin tx: {e}")))?;
    tx.execute(CREATE_UNDERSTANDING_DEPTHS_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v4 migration: understanding_depths: {e}")))?;
    tx.execute(CREATE_LEARNERS_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v4 migration: learners: {e}")))?;
    tx.execute(CREATE_LEARNER_CONCEPTS_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v4 migration: learner_concepts: {e}")))?;
    tx.execute(CREATE_LEARNER_CONCEPTS_INDEX, [])
        .map_err(|e| PrimerError::Storage(format!("v4 migration: index: {e}")))?;
    tx.commit()
        .map_err(|e| PrimerError::Storage(format!("v4 migration: commit: {e}")))?;
    Ok(())
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let sql = format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1");
    let count: i64 = conn
        .query_row(&sql, rusqlite::params![column], |r| r.get(0))
        .map_err(|e| PrimerError::Storage(format!("table_info({table}): {e}")))?;
    Ok(count > 0)
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            rusqlite::params![name],
            |r| r.get(0),
        )
        .map_err(|e| PrimerError::Storage(format!("check table {name}: {e}")))?;
    Ok(count > 0)
}

/// Validate-and-seed pass for a lookup table.
///
/// `expected` is the canonical source of truth (Rust enum projected to
/// `(id, name)` pairs). The function:
///
/// - Inserts any expected row missing from the table.
/// - Returns `Storage` error if a known id has the wrong name.
/// - Returns `Storage` error if the table contains an id not in `expected`.
///
/// Runs synchronously inside an `&Connection` borrow; caller is
/// responsible for transaction scope.
pub fn validate_and_seed_lookup(
    conn: &Connection,
    table: &str,
    expected: &[(i64, &str)],
) -> Result<()> {
    let actual: HashMap<i64, String> = {
        let sql = format!("SELECT id, name FROM {table}");
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| PrimerError::Storage(format!("prepare for {table}: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| PrimerError::Storage(format!("query {table}: {e}")))?;
        let mut map = HashMap::new();
        for row in rows {
            let (id, name) =
                row.map_err(|e| PrimerError::Storage(format!("read {table} row: {e}")))?;
            map.insert(id, name);
        }
        map
    };

    let expected_ids: std::collections::HashSet<i64> = expected.iter().map(|(id, _)| *id).collect();

    // Step 1: detect unknown ids in the DB.
    for (id, name) in &actual {
        if !expected_ids.contains(id) {
            return Err(PrimerError::Storage(format!(
                "{table} has unknown row id={id} name={name:?} — database may be from a newer build"
            )));
        }
    }

    // Step 2: validate matches and insert missing.
    let insert_sql = format!("INSERT INTO {table} (id, name) VALUES (?1, ?2)");
    for (id, expected_name) in expected {
        match actual.get(id) {
            Some(actual_name) if actual_name == expected_name => {
                // match — nothing to do
            }
            Some(actual_name) => {
                return Err(PrimerError::Storage(format!(
                    "{table} row id={id} has name {actual_name:?}, this build expects {expected_name:?}"
                )));
            }
            None => {
                conn.execute(&insert_sql, rusqlite::params![id, expected_name])
                    .map_err(|e| PrimerError::Storage(format!("insert into {table}: {e}")))?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod v4_tests {
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
    fn user_version_constant_is_four() {
        assert_eq!(USER_VERSION, 4);
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
}
