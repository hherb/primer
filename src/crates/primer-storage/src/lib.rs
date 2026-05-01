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
use chrono::{DateTime, Utc};
use primer_core::error::{PrimerError, Result};
use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

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
    /// asserts/sets `PRAGMA user_version`, and applies v2 and v3
    /// migrations to bring older DBs up to date. The migrations are
    /// idempotent — safe to run on fresh, v1, v2, or v3 DBs. A version
    /// newer than this build understands is a hard error rather than a
    /// silent downgrade.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .map_err(|e| PrimerError::Storage(format!("open failed: {e}")))?;

        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|e| PrimerError::Storage(format!("PRAGMA foreign_keys failed: {e}")))?;

        // Read existing user_version. A fresh DB returns 0; v1 DBs from
        // before the rolling-summary work return 1; current builds stamp 2.
        let existing_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| PrimerError::Storage(format!("read user_version failed: {e}")))?;

        if existing_version > schema::USER_VERSION {
            return Err(PrimerError::Storage(format!(
                "incompatible schema version: file is at user_version={existing_version}, this build expects {}",
                schema::USER_VERSION
            )));
        }

        conn.execute_batch(schema::SCHEMA_SQL)
            .map_err(|e| PrimerError::Storage(format!("schema creation failed: {e}")))?;

        // v2 migrations: idempotent on every open. Adds summary columns
        // and the FTS5 turn-text index if not already present.
        schema::apply_v2_migrations(&conn)?;

        // v3 migrations: idempotent on every open. Adds engagement_states,
        // classifiers, and turn_classifications tables.
        schema::apply_v3_migrations(&conn)?;

        if existing_version != schema::USER_VERSION {
            conn.execute_batch(&format!("PRAGMA user_version = {};", schema::USER_VERSION))
                .map_err(|e| PrimerError::Storage(format!("set user_version failed: {e}")))?;
        }

        // Validate-and-seed the lookup tables. Borrows the connection
        // directly; no transaction needed because the writes are
        // idempotent INSERTs.
        let speakers = catalog::expected_speakers();
        let intents = catalog::expected_intents();
        let engagement_states = catalog::expected_engagement_states();
        schema::validate_and_seed_lookup(&conn, "speakers", &speakers)?;
        schema::validate_and_seed_lookup(&conn, "pedagogical_intents", &intents)?;
        schema::validate_and_seed_lookup(&conn, "engagement_states", &engagement_states)?;

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
        // (ON CONFLICT … DO UPDATE) updates in place. `learner_id` and
        // `started_at` are pinned at session start; `ended_at`,
        // `summary`, and `summary_through_turn_index` may change as the
        // conversation evolves.
        tx.execute(
            "INSERT INTO sessions
                 (id, learner_id, started_at, ended_at,
                  summary, summary_through_turn_index)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 ended_at = excluded.ended_at,
                 summary = excluded.summary,
                 summary_through_turn_index = excluded.summary_through_turn_index",
            rusqlite::params![
                session.id.to_string(),
                session.learner_id.to_string(),
                session.started_at.to_rfc3339(),
                session.ended_at.map(|t| t.to_rfc3339()),
                session.summary,
                session.summary_through_turn_index as i64,
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

    /// Load a session by id. Returns `Ok(None)` when no session with
    /// that id exists. Three SELECTs: session metadata, turns ordered
    /// by `turn_index`, and a single concept join keyed by turn rowid.
    ///
    /// The shape mirrors `save_session`'s three discrete prepared
    /// statements rather than collapsing into one `LEFT JOIN` + group —
    /// at conversation row counts the perf delta is nil and the explicit
    /// concept-grouping loop is more readable.
    async fn load_session(&self, id: Uuid) -> Result<Option<primer_core::conversation::Session>> {
        use primer_core::conversation::{Session, Turn};

        let conn = self.conn.lock().unwrap();

        // Step 1: session row.
        let row: Option<(String, String, String, Option<String>, String, i64)> = conn
            .query_row(
                "SELECT id, learner_id, started_at, ended_at,
                        summary, summary_through_turn_index
                 FROM sessions WHERE id = ?1",
                rusqlite::params![id.to_string()],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| PrimerError::Storage(format!("select session: {e}")))?;

        let Some((id_str, learner_str, started_str, ended_opt, summary, summary_through)) = row
        else {
            return Ok(None);
        };

        let session_uuid = Uuid::parse_str(&id_str)
            .map_err(|e| PrimerError::Storage(format!("parse session id {id_str}: {e}")))?;
        let learner_uuid = Uuid::parse_str(&learner_str)
            .map_err(|e| PrimerError::Storage(format!("parse learner id {learner_str}: {e}")))?;
        let started_at = parse_rfc3339(&started_str, "started_at")?;
        let ended_at = ended_opt
            .as_deref()
            .map(|s| parse_rfc3339(s, "ended_at"))
            .transpose()?;

        // Step 2: turns ordered by turn_index. Capture each turn's rowid
        // so we can attach concepts in step 3.
        let mut turns_with_id: Vec<(i64, Turn)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT id, speaker_id, text, timestamp, intent_id
                     FROM turns WHERE session_id = ?1 ORDER BY turn_index",
                )
                .map_err(|e| PrimerError::Storage(format!("prepare select turns: {e}")))?;
            let rows = stmt
                .query_map(rusqlite::params![id.to_string()], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<i64>>(4)?,
                    ))
                })
                .map_err(|e| PrimerError::Storage(format!("query turns: {e}")))?;

            let mut out = Vec::new();
            for row in rows {
                let (turn_id, speaker_id, text, ts_str, intent_id) =
                    row.map_err(|e| PrimerError::Storage(format!("read turn row: {e}")))?;
                let speaker = catalog::speaker_from_id(speaker_id).ok_or_else(|| {
                    PrimerError::Storage(format!("unknown speaker_id {speaker_id}"))
                })?;
                let intent =
                    match intent_id {
                        None => None,
                        Some(id) => Some(catalog::intent_from_id(id).ok_or_else(|| {
                            PrimerError::Storage(format!("unknown intent_id {id}"))
                        })?),
                    };
                let timestamp = parse_rfc3339(&ts_str, "turn timestamp")?;
                out.push((
                    turn_id,
                    Turn {
                        speaker,
                        text,
                        timestamp,
                        intent,
                        concepts: vec![],
                    },
                ));
            }
            out
        };

        // Step 3: concepts per turn, grouped by turn_id.
        if !turns_with_id.is_empty() {
            let mut stmt = conn
                .prepare(
                    "SELECT tc.turn_id, c.name
                     FROM turn_concepts tc
                     JOIN concepts c ON c.id = tc.concept_id
                     WHERE tc.turn_id IN (
                         SELECT id FROM turns WHERE session_id = ?1
                     )
                     ORDER BY c.name",
                )
                .map_err(|e| PrimerError::Storage(format!("prepare concepts: {e}")))?;
            let rows = stmt
                .query_map(rusqlite::params![id.to_string()], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(|e| PrimerError::Storage(format!("query concepts: {e}")))?;
            let mut grouped: std::collections::HashMap<i64, Vec<String>> =
                std::collections::HashMap::new();
            for row in rows {
                let (turn_id, name) =
                    row.map_err(|e| PrimerError::Storage(format!("read concept row: {e}")))?;
                grouped.entry(turn_id).or_default().push(name);
            }
            for (turn_id, turn) in turns_with_id.iter_mut() {
                if let Some(concepts) = grouped.remove(turn_id) {
                    turn.concepts = concepts;
                }
            }
        }

        let turns: Vec<Turn> = turns_with_id.into_iter().map(|(_, t)| t).collect();

        Ok(Some(Session {
            id: session_uuid,
            learner_id: learner_uuid,
            started_at,
            ended_at,
            turns,
            summary,
            summary_through_turn_index: summary_through.max(0) as usize,
        }))
    }

    /// FTS5 retrieval over a single session's turns. Treats `query` as a
    /// literal phrase: any quote characters in the input are stripped and
    /// the whole string is wrapped in `"..."` so FTS5 operators like `OR`,
    /// `NEAR`, `*`, `^` and column qualifiers cannot be smuggled in by a
    /// child's input.
    async fn retrieve_session_turns(
        &self,
        session_id: Uuid,
        query: &str,
        k: usize,
        exclude_indices_at_or_after: usize,
    ) -> Result<Vec<primer_core::conversation::Turn>> {
        use primer_core::conversation::Turn;

        let phrase = sanitize_fts_phrase(query);
        // Empty input → nothing to match. Avoids issuing a `MATCH '""'`
        // which FTS5 rejects as a syntax error.
        if phrase.is_empty() {
            return Ok(vec![]);
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT t.speaker_id, t.text, t.timestamp, t.intent_id
                 FROM turn_text_fts f
                 JOIN turns t ON t.id = f.rowid
                 WHERE f.text MATCH ?1
                   AND t.session_id = ?2
                   AND t.turn_index < ?3
                 ORDER BY bm25(turn_text_fts)
                 LIMIT ?4",
            )
            .map_err(|e| PrimerError::Storage(format!("prepare retrieve: {e}")))?;

        let rows = stmt
            .query_map(
                rusqlite::params![
                    phrase,
                    session_id.to_string(),
                    exclude_indices_at_or_after as i64,
                    k as i64,
                ],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                    ))
                },
            )
            .map_err(|e| PrimerError::Storage(format!("query retrieve: {e}")))?;

        let mut out = Vec::with_capacity(k);
        for row in rows {
            let (speaker_id, text, ts_str, intent_id) =
                row.map_err(|e| PrimerError::Storage(format!("read retrieve row: {e}")))?;
            let speaker = catalog::speaker_from_id(speaker_id)
                .ok_or_else(|| PrimerError::Storage(format!("unknown speaker_id {speaker_id}")))?;
            let intent = match intent_id {
                None => None,
                Some(id) => Some(
                    catalog::intent_from_id(id)
                        .ok_or_else(|| PrimerError::Storage(format!("unknown intent_id {id}")))?,
                ),
            };
            let timestamp = parse_rfc3339(&ts_str, "turn timestamp")?;
            out.push(Turn {
                speaker,
                text,
                timestamp,
                intent,
                concepts: vec![],
            });
        }
        Ok(out)
    }

    async fn save_classification(
        &self,
        session_id: primer_core::conversation::SessionId,
        turn_index: usize,
        assessment: &primer_core::classifier::EngagementAssessment,
        classifier_identifier: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // Resolve (session_id, turn_index) → turn.id
        let turn_id: i64 = conn
            .query_row(
                "SELECT id FROM turns WHERE session_id = ?1 AND turn_index = ?2",
                rusqlite::params![session_id.to_string(), turn_index as i64],
                |r| r.get(0),
            )
            .map_err(|e| {
                PrimerError::Storage(format!(
                    "save_classification: turn_id lookup ({session_id}, {turn_index}): {e}"
                ))
            })?;

        let classifier_id = catalog::get_or_create_classifier_id(&conn, classifier_identifier)?;
        let state_id = catalog::engagement_state_id(assessment.state);

        conn.execute(
            "INSERT INTO turn_classifications
                 (turn_id, engagement_state_id, classifier_id, confidence, reasoning, classified_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                turn_id,
                state_id,
                classifier_id,
                assessment.confidence,
                assessment.reasoning.as_deref(),
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|e| PrimerError::Storage(format!("save_classification: insert: {e}")))?;

        Ok(())
    }

    async fn load_recent_assessments(
        &self,
        session_id: primer_core::conversation::SessionId,
        classifier_identifier: &str,
        k: usize,
    ) -> Result<Vec<primer_core::classifier::EngagementAssessment>> {
        let conn = self.conn.lock().unwrap();

        // If the classifier has never been created, there are no rows.
        let classifier_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM classifiers WHERE identifier = ?1",
                rusqlite::params![classifier_identifier],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| {
                PrimerError::Storage(format!("load_recent_assessments: classifier lookup: {e}"))
            })?;

        let Some(classifier_id) = classifier_id else {
            return Ok(vec![]);
        };

        // Fetch the k most-recent rows (DESC by classified_at), then
        // reverse so the caller gets oldest-first within the window.
        let mut stmt = conn
            .prepare(
                "SELECT tc.engagement_state_id, tc.confidence, tc.reasoning
                 FROM turn_classifications tc
                 JOIN turns t ON t.id = tc.turn_id
                 WHERE t.session_id = ?1
                   AND tc.classifier_id = ?2
                 ORDER BY tc.classified_at DESC
                 LIMIT ?3",
            )
            .map_err(|e| PrimerError::Storage(format!("load_recent_assessments: prepare: {e}")))?;

        let mut rows: Vec<primer_core::classifier::EngagementAssessment> = stmt
            .query_map(
                rusqlite::params![session_id.to_string(), classifier_id, k as i64],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, f32>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .map_err(|e| PrimerError::Storage(format!("load_recent_assessments: query: {e}")))?
            .filter_map(|res| {
                let (state_id, confidence, reasoning) = res.ok()?;
                let state = catalog::engagement_state_from_id(state_id)?;
                Some(primer_core::classifier::EngagementAssessment {
                    state,
                    confidence,
                    reasoning,
                })
            })
            .collect();

        // Reverse DESC → oldest-first for the caller's trajectory buffer.
        rows.reverse();
        Ok(rows)
    }
}

fn parse_rfc3339(s: &str, field: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| PrimerError::Storage(format!("parse {field} {s:?}: {e}")))
}

/// Sanitize an arbitrary user query into an FTS5 expression that is
/// safe to pass to `MATCH`. Tokenizes on whitespace, strips every
/// non-alphanumeric character per token (kills `*`, `^`, `:`, `"`, `(`,
/// `)`, slashes, etc.), drops the FTS5 reserved keywords (`AND`, `OR`,
/// `NOT`, `NEAR`), wraps each surviving token in double quotes (so any
/// special character the tokenizer would otherwise see is inert), and
/// joins the tokens with explicit `OR`. An empty result means "no
/// useful tokens"; the caller should skip the query rather than issue
/// `MATCH ''` which FTS5 rejects.
///
/// `OR` is chosen over implicit-AND so that "noise" tokens introduced
/// by sanitization (e.g. fragments from stripped punctuation) do not
/// torpedo the entire query. BM25 ranking + the caller's `LIMIT k` keep
/// the result list focused on the most relevant matches.
fn sanitize_fts_phrase(query: &str) -> String {
    const RESERVED: &[&str] = &["AND", "OR", "NOT", "NEAR"];
    let tokens: Vec<String> = query
        .split_whitespace()
        .map(|tok| {
            tok.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
        })
        .filter(|tok| !tok.is_empty())
        .filter(|tok| !RESERVED.iter().any(|r| r.eq_ignore_ascii_case(tok)))
        .collect();
    if tokens.is_empty() {
        return String::new();
    }
    tokens
        .iter()
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(" OR ")
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

        // user_version is the current schema version.
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, schema::USER_VERSION);

        // foreign_keys is on.
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);

        // All base tables exist, plus the v2 FTS index and the v3 tables.
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
        // First open creates the schema and stamps the current user_version.
        let tmp = tempfile_path();
        {
            let _store = SqliteSessionStore::open(&tmp).unwrap();
        }
        // Second open should succeed cleanly. user_version stays put.
        let store = SqliteSessionStore::open(&tmp).unwrap();
        let conn = store.conn.lock().unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, schema::USER_VERSION);
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
        for (id, name) in catalog::expected_engagement_states() {
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
        let err = SqliteSessionStore::open(&tmp).unwrap_err();
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

        let result = schema::apply_v2_migrations(&conn);
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
            schema::USER_VERSION
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
        let store = SqliteSessionStore::open(&tmp).unwrap();
        let conn = store.conn.lock().unwrap();

        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, schema::USER_VERSION);

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
        conn.execute_batch(schema::SCHEMA_SQL).unwrap();
        schema::apply_v2_migrations(&conn).unwrap();

        schema::apply_v3_migrations(&conn).unwrap();

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
        conn.execute_batch(schema::SCHEMA_SQL).unwrap();
        schema::apply_v2_migrations(&conn).unwrap();
        schema::apply_v3_migrations(&conn).unwrap();
        schema::apply_v3_migrations(&conn).unwrap(); // second call must succeed
    }

    #[test]
    fn apply_v3_migrations_creates_index_on_turn_classifications() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(schema::SCHEMA_SQL).unwrap();
        schema::apply_v2_migrations(&conn).unwrap();
        schema::apply_v3_migrations(&conn).unwrap();

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
    fn user_version_is_three() {
        assert_eq!(schema::USER_VERSION, 3);
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
}
