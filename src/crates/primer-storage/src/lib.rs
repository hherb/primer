//! # primer-storage
//!
//! SQLite-backed implementations of the persistence traits defined in
//! `primer-core::storage`.
//!
//! Mirrors the locking and error patterns of `primer-knowledge`: a
//! single `Connection` wrapped in `Mutex`, async trait methods with
//! synchronous bodies (acceptable at our turn rate; revisit if profiling
//! ever shows contention).
//!
//! ## Concurrency caveat
//!
//! The lock is `std::sync::Mutex`, taken from inside an async fn. On a
//! slow disk that means we block the tokio runtime while the SQLite
//! write completes. Acceptable for a single-user CLI; if a future
//! deployment ever has multiple concurrent writers (parallel learners
//! sharing a runtime, or a multi-process consumer), revisit with a
//! `tokio::sync::Mutex` and/or `spawn_blocking`.

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
    /// Append-only persist. Re-saving a session that hasn't grown is a
    /// true row-level no-op for `turns` and `turn_concepts` (no DELETEs,
    /// no re-INSERTs) — only the `sessions` row is upserted to capture
    /// `ended_at` updates. Turns persisted in earlier saves keep their
    /// auto-incremented `id`s across subsequent saves, which matters for
    /// any future feature that wants to FK into `turns.id`.
    ///
    /// Pre-condition: `session.turns` is append-only in memory. This
    /// codebase's `Session` type only ever appends, so we exploit that
    /// to skip the work of reconciling deletions or modifications to
    /// already-persisted turns.
    async fn save_session(&self, session: &primer_core::conversation::Session) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| PrimerError::Storage(format!("begin tx: {e}")))?;

        // Upsert session metadata. Plain `INSERT OR REPLACE` would do a
        // DELETE-then-INSERT, which cascades through the FK and wipes
        // every turn we've already persisted. The proper SQLite UPSERT
        // (ON CONFLICT … DO UPDATE) updates in place. Only `ended_at`
        // is allowed to change after creation; `learner_id` and
        // `started_at` are pinned at session start.
        tx.execute(
            "INSERT INTO sessions (id, learner_id, started_at, ended_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET ended_at = excluded.ended_at",
            rusqlite::params![
                session.id.to_string(),
                session.learner_id.to_string(),
                session.started_at.to_rfc3339(),
                session.ended_at.map(|t| t.to_rfc3339()),
            ],
        )
        .map_err(|e| PrimerError::Storage(format!("upsert session: {e}")))?;

        // How many turns are already on disk for this session. Append
        // anything in memory beyond that.
        let persisted_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM turns WHERE session_id = ?1",
                rusqlite::params![session.id.to_string()],
                |r| r.get(0),
            )
            .map_err(|e| PrimerError::Storage(format!("count persisted turns: {e}")))?;
        let persisted_count = persisted_count as usize;

        if persisted_count < session.turns.len() {
            let mut insert_turn = tx
                .prepare(
                    "INSERT INTO turns (session_id, turn_index, speaker_id, text, timestamp, intent_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .map_err(|e| PrimerError::Storage(format!("prepare insert turn: {e}")))?;
            let mut insert_concept = tx
                .prepare("INSERT OR IGNORE INTO concepts (name) VALUES (?1)")
                .map_err(|e| PrimerError::Storage(format!("prepare insert concept: {e}")))?;
            let mut select_concept = tx
                .prepare("SELECT id FROM concepts WHERE name = ?1")
                .map_err(|e| PrimerError::Storage(format!("prepare select concept: {e}")))?;
            let mut link_concept = tx
                .prepare(
                    "INSERT OR IGNORE INTO turn_concepts (turn_id, concept_id) VALUES (?1, ?2)",
                )
                .map_err(|e| PrimerError::Storage(format!("prepare link concept: {e}")))?;

            // Per-call cache so the same concept name within one save
            // doesn't hit the DB twice.
            let mut concept_name_cache: std::collections::HashMap<String, i64> =
                std::collections::HashMap::new();

            for (idx, turn) in session.turns.iter().enumerate().skip(persisted_count) {
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
                // Capture the turn's rowid before we INSERT anything else
                // (concept inserts would shift `last_insert_rowid`).
                let turn_db_id = tx.last_insert_rowid();

                for name in &turn.concepts {
                    let id = match concept_name_cache.get(name).copied() {
                        Some(id) => id,
                        None => {
                            insert_concept
                                .execute(rusqlite::params![name])
                                .map_err(|e| {
                                    PrimerError::Storage(format!("upsert concept {name}: {e}"))
                                })?;
                            let id: i64 = select_concept
                                .query_row(rusqlite::params![name], |r| r.get(0))
                                .map_err(|e| {
                                    PrimerError::Storage(format!("select concept {name}: {e}"))
                                })?;
                            concept_name_cache.insert(name.clone(), id);
                            id
                        }
                    };
                    link_concept
                        .execute(rusqlite::params![turn_db_id, id])
                        .map_err(|e| PrimerError::Storage(format!("link concept {name}: {e}")))?;
                }
            }
            // Drop borrows of `tx` before commit consumes it.
            drop(link_concept);
            drop(select_concept);
            drop(insert_concept);
            drop(insert_turn);
        }

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
        let err = SqliteSessionStore::open(&tmp).unwrap_err();
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
        let err = SqliteSessionStore::open(&tmp).unwrap_err();
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
            let v = catalog::intent_from_id(id).expect("known id");
            variants_seen.push(v);
        }
        let expected: Vec<PedagogicalIntent> = PedagogicalIntent::ALL.to_vec();
        assert_eq!(variants_seen, expected);
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
}
