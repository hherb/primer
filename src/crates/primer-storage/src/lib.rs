//! # primer-storage
//!
//! SQLite-backed implementations of the persistence traits defined in
//! `primer-core::storage`.
//!
//! Mirrors the locking and error patterns of `primer-knowledge`: a
//! single `Connection` wrapped in `Mutex`, async trait methods with
//! synchronous bodies (acceptable at our turn rate; revisit if profiling
//! ever shows contention).

// catalog functions are wired in by tasks 8 and 12; suppress dead-code
// warnings until then.
#[allow(dead_code)]
mod catalog;
mod schema;

use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use primer_core::error::{PrimerError, Result};
use rusqlite::Connection;

/// SQLite-backed session store.
#[derive(Debug)]
pub struct SqliteSessionStore {
    conn: Mutex<Connection>,
}

impl SqliteSessionStore {
    /// Open (or create) a session store at `path`. Use `:memory:` for
    /// an in-memory database.
    ///
    /// Creates the schema if missing, sets `PRAGMA foreign_keys = ON`,
    /// and asserts/sets `PRAGMA user_version`.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .map_err(|e| PrimerError::Storage(format!("open failed: {e}")))?;

        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|e| PrimerError::Storage(format!("PRAGMA foreign_keys failed: {e}")))?;

        // Read existing user_version. A fresh DB returns 0.
        let existing_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| PrimerError::Storage(format!("read user_version failed: {e}")))?;

        if existing_version != 0 && existing_version != schema::USER_VERSION {
            return Err(PrimerError::Storage(format!(
                "incompatible schema version: file is at user_version={existing_version}, this build expects {}",
                schema::USER_VERSION
            )));
        }

        conn.execute_batch(schema::SCHEMA_SQL)
            .map_err(|e| PrimerError::Storage(format!("schema creation failed: {e}")))?;

        if existing_version == 0 {
            conn.execute_batch(&format!("PRAGMA user_version = {};", schema::USER_VERSION))
                .map_err(|e| PrimerError::Storage(format!("set user_version failed: {e}")))?;
        }

        // Validate-and-seed the lookup tables. Borrows the connection
        // directly; no transaction needed because the writes are
        // idempotent INSERTs.
        let speakers = catalog::expected_speakers();
        let intents = catalog::expected_intents();
        schema::validate_and_seed_lookup(&conn, "speakers", &speakers)?;
        schema::validate_and_seed_lookup(&conn, "pedagogical_intents", &intents)?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

#[async_trait]
impl primer_core::storage::SessionStore for SqliteSessionStore {
    async fn save_session(&self, session: &primer_core::conversation::Session) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| PrimerError::Storage(format!("begin tx: {e}")))?;

        // Upsert session metadata.
        tx.execute(
            "INSERT OR REPLACE INTO sessions (id, learner_id, started_at, ended_at)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                session.id.to_string(),
                session.learner_id.to_string(),
                session.started_at.to_rfc3339(),
                session.ended_at.map(|t| t.to_rfc3339()),
            ],
        )
        .map_err(|e| PrimerError::Storage(format!("upsert session: {e}")))?;

        // Clear turns; they get fully rebuilt below. Cascades to turn_concepts.
        tx.execute(
            "DELETE FROM turns WHERE session_id = ?1",
            rusqlite::params![session.id.to_string()],
        )
        .map_err(|e| PrimerError::Storage(format!("delete old turns: {e}")))?;

        // Insert each turn with its speaker_id and intent_id integer FKs.
        let mut insert_turn = tx
            .prepare(
                "INSERT INTO turns (session_id, turn_index, speaker_id, text, timestamp, intent_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .map_err(|e| PrimerError::Storage(format!("prepare insert turn: {e}")))?;

        for (idx, turn) in session.turns.iter().enumerate() {
            let speaker_id = catalog::speaker_id(turn.speaker);
            let intent_id = turn.intent.map(catalog::intent_id);
            insert_turn
                .execute(rusqlite::params![
                    session.id.to_string(),
                    idx as i64,
                    speaker_id,
                    turn.text,
                    turn.timestamp.to_rfc3339(),
                    intent_id,
                ])
                .map_err(|e| PrimerError::Storage(format!("insert turn {idx}: {e}")))?;
        }
        drop(insert_turn);

        tx.commit()
            .map_err(|e| PrimerError::Storage(format!("commit: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use chrono::Utc;
    use primer_core::conversation::{Session, Turn};
    use primer_core::storage::SessionStore;
    use uuid::Uuid;

    fn open_memory() -> SqliteSessionStore {
        SqliteSessionStore::open(&PathBuf::from(":memory:")).expect("open :memory:")
    }

    fn make_turn(speaker: primer_core::conversation::Speaker, text: &str,
                 intent: Option<primer_core::conversation::PedagogicalIntent>,
                 concepts: Vec<String>) -> Turn {
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

        // user_version was set.
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 1);

        // foreign_keys is on.
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);

        // All six tables exist.
        for table in &[
            "speakers",
            "pedagogical_intents",
            "concepts",
            "sessions",
            "turns",
            "turn_concepts",
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
        let err = SqliteSessionStore::open(&tmp).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("99"), "error should mention the bad version: {msg}");
        assert!(msg.contains("Storage"), "error should be a Storage variant: {msg}");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn open_existing_valid_db_is_a_no_op() {
        // First open creates the schema and stamps user_version=1.
        let tmp = tempfile_path();
        {
            let _store = SqliteSessionStore::open(&tmp).unwrap();
        }
        // Second open should succeed cleanly. user_version stays at 1.
        let store = SqliteSessionStore::open(&tmp).unwrap();
        let conn = store.conn.lock().unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 1);
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
        assert_eq!(intent_count, 8);

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
    fn reopen_is_a_no_op_on_seeded_tables() {
        let tmp = tempfile_path();
        {
            let _store = SqliteSessionStore::open(&tmp).unwrap();
        }
        let store = SqliteSessionStore::open(&tmp).unwrap();
        let conn = store.conn.lock().unwrap();
        let speaker_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM speakers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(speaker_count, 2, "second open should not duplicate seed rows");
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
        let err = SqliteSessionStore::open(&tmp).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("WrongName") || msg.contains("SocraticQuestion"),
            "error should mention the conflict: {msg}");
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
        let err = SqliteSessionStore::open(&tmp).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("99") || msg.contains("unknown"),
            "error should mention the unknown id: {msg}");
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
        let store = SqliteSessionStore::open(&tmp).unwrap();
        let conn = store.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM pedagogical_intents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 8, "missing rows should have been seeded");
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
        session.add_turn(make_turn(Speaker::Child, "why is the sky blue", None, vec![]));
        session.add_turn(make_turn(
            Speaker::Primer,
            "What do you notice about the sky during the day?",
            Some(PedagogicalIntent::SocraticQuestion),
            vec![],
        ));

        store.save_session(&session).await.unwrap();

        let conn = store.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT turn_index, speaker_id, text, intent_id FROM turns
                     WHERE session_id = ?1 ORDER BY turn_index")
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

    /// Returns a unique tempfile path using a UUID to avoid collisions
    /// between parallel test threads.
    fn tempfile_path() -> PathBuf {
        std::env::temp_dir()
            .join(format!("primer-storage-test-{}.db", uuid::Uuid::new_v4()))
    }
}
