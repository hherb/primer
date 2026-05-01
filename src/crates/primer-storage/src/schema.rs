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
pub const USER_VERSION: i64 = 2;

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
