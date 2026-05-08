//! `SessionStore` save side: save_* and update_* writes plus per-turn embedding persistence.
//!
//! Inherent `pub(super) async fn *_inner` methods on `SqliteSessionStore`.
//! The trait dispatch lives in `super::session`; each trait method is a
//! one-line delegation to the matching `_inner`. Keeps the trait surface
//! tiny and the heavy bodies grouped by responsibility.

use chrono::Utc;
use primer_core::error::{PrimerError, Result};

use super::SqliteSessionStore;
use super::embeddings::{upsert_storage_embedding_model, vec_to_blob};

impl SqliteSessionStore {
    pub(super) async fn save_session_inner(
        &self,
        session: &primer_core::conversation::Session,
    ) -> Result<()> {
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
                .prepare(
                    "INSERT OR IGNORE INTO concepts (name, concept_language_tag) VALUES (?1, ?2)",
                )
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
            let locale_tag = self.locale.pack_id();

            for (idx, turn) in session.turns.iter().enumerate().skip(persisted_count) {
                let speaker_id = crate::catalog::speaker_id(turn.speaker);
                let intent_id = turn.intent.map(crate::catalog::intent_id);
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
                                .execute(rusqlite::params![name, locale_tag])
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

    pub(super) async fn save_classification_inner(
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

        let classifier_id =
            crate::catalog::get_or_create_classifier_id(&conn, classifier_identifier)?;
        let state_id = crate::catalog::engagement_state_id(assessment.state);

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

    pub(super) async fn update_turn_concepts_inner(
        &self,
        session_id: primer_core::conversation::SessionId,
        turn_index: usize,
        concepts: &[String],
    ) -> Result<()> {
        if concepts.is_empty() {
            return Ok(());
        }

        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| PrimerError::Storage(format!("update_turn_concepts begin tx: {e}")))?;

        // Resolve (session_id, turn_index) → turn_id.
        let turn_id: i64 = tx
            .query_row(
                "SELECT id FROM turns WHERE session_id = ?1 AND turn_index = ?2",
                rusqlite::params![session_id.to_string(), turn_index as i64],
                |r| r.get(0),
            )
            .map_err(|e| {
                PrimerError::Storage(format!(
                    "resolve turn (session={session_id}, index={turn_index}): {e}"
                ))
            })?;

        let mut insert_concept = tx
            .prepare("INSERT OR IGNORE INTO concepts (name, concept_language_tag) VALUES (?1, ?2)")
            .map_err(|e| PrimerError::Storage(format!("prepare insert concept: {e}")))?;
        let mut select_concept = tx
            .prepare("SELECT id FROM concepts WHERE name = ?1")
            .map_err(|e| PrimerError::Storage(format!("prepare select concept: {e}")))?;
        let mut link_concept = tx
            .prepare("INSERT OR IGNORE INTO turn_concepts (turn_id, concept_id) VALUES (?1, ?2)")
            .map_err(|e| PrimerError::Storage(format!("prepare link concept: {e}")))?;
        let locale_tag = self.locale.pack_id();

        for name in concepts {
            insert_concept
                .execute(rusqlite::params![name, locale_tag])
                .map_err(|e| PrimerError::Storage(format!("upsert concept {name}: {e}")))?;
            let cid: i64 = select_concept
                .query_row(rusqlite::params![name], |r| r.get(0))
                .map_err(|e| PrimerError::Storage(format!("select concept {name}: {e}")))?;
            link_concept
                .execute(rusqlite::params![turn_id, cid])
                .map_err(|e| PrimerError::Storage(format!("link concept {name}: {e}")))?;
        }

        drop(link_concept);
        drop(select_concept);
        drop(insert_concept);

        tx.commit()
            .map_err(|e| PrimerError::Storage(format!("update_turn_concepts commit: {e}")))?;
        Ok(())
    }

    pub(super) async fn update_exchange_concepts_inner(
        &self,
        session_id: primer_core::conversation::SessionId,
        child_turn_index: usize,
        child_concepts: &[String],
        primer_turn_index: usize,
        primer_concepts: &[String],
    ) -> Result<()> {
        if child_concepts.is_empty() && primer_concepts.is_empty() {
            return Ok(());
        }

        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| PrimerError::Storage(format!("update_exchange_concepts begin tx: {e}")))?;

        // Per-call concept-name cache so a concept appearing in both
        // lists (or repeated in one list, though normalize_concepts
        // already dedupes those) hits the DB once.
        let mut concept_name_cache: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();

        {
            let locale_tag = self.locale.pack_id();
            let mut insert_concept = tx
                .prepare(
                    "INSERT OR IGNORE INTO concepts (name, concept_language_tag) VALUES (?1, ?2)",
                )
                .map_err(|e| PrimerError::Storage(format!("prepare insert concept: {e}")))?;
            let mut select_concept = tx
                .prepare("SELECT id FROM concepts WHERE name = ?1")
                .map_err(|e| PrimerError::Storage(format!("prepare select concept: {e}")))?;
            let mut select_turn_id = tx
                .prepare("SELECT id FROM turns WHERE session_id = ?1 AND turn_index = ?2")
                .map_err(|e| PrimerError::Storage(format!("prepare select turn: {e}")))?;
            let mut link_concept = tx
                .prepare(
                    "INSERT OR IGNORE INTO turn_concepts (turn_id, concept_id) VALUES (?1, ?2)",
                )
                .map_err(|e| PrimerError::Storage(format!("prepare link concept: {e}")))?;

            // Closure capturing the prepared statements + cache.
            let mut apply_one = |turn_index: usize, concepts: &[String]| -> Result<()> {
                if concepts.is_empty() {
                    return Ok(());
                }
                let turn_id: i64 = select_turn_id
                    .query_row(
                        rusqlite::params![session_id.to_string(), turn_index as i64],
                        |r| r.get(0),
                    )
                    .map_err(|e| {
                        PrimerError::Storage(format!(
                            "resolve turn (session={session_id}, index={turn_index}): {e}"
                        ))
                    })?;
                for name in concepts {
                    let cid = match concept_name_cache.get(name).copied() {
                        Some(id) => id,
                        None => {
                            insert_concept
                                .execute(rusqlite::params![name, locale_tag])
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
                        .execute(rusqlite::params![turn_id, cid])
                        .map_err(|e| PrimerError::Storage(format!("link concept {name}: {e}")))?;
                }
                Ok(())
            };

            apply_one(child_turn_index, child_concepts)?;
            apply_one(primer_turn_index, primer_concepts)?;
        }

        tx.commit()
            .map_err(|e| PrimerError::Storage(format!("update_exchange_concepts commit: {e}")))?;
        Ok(())
    }

    pub(super) async fn save_comprehensions_inner(
        &self,
        session_id: primer_core::conversation::SessionId,
        primer_turn_index: usize,
        assessments: &[primer_core::comprehension::ComprehensionAssessment],
        classifier_identifier: &str,
    ) -> Result<()> {
        if assessments.is_empty() {
            return Ok(());
        }

        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| PrimerError::Storage(format!("save_comprehensions begin tx: {e}")))?;

        // Resolve (session_id, primer_turn_index) → turn.id
        let turn_id: i64 = tx
            .query_row(
                "SELECT id FROM turns WHERE session_id = ?1 AND turn_index = ?2",
                rusqlite::params![session_id.to_string(), primer_turn_index as i64],
                |r| r.get(0),
            )
            .map_err(|e| {
                PrimerError::Storage(format!(
                    "save_comprehensions: turn_id lookup ({session_id}, {primer_turn_index}): {e}"
                ))
            })?;

        let classifier_id =
            crate::catalog::get_or_create_comprehension_classifier_id(&tx, classifier_identifier)?;

        let now = Utc::now().to_rfc3339();
        // Per-call cache of concept names → ids so a concept appearing
        // in multiple assessments resolves to the same row without
        // hitting the DB twice. Aligns with the cache pattern used in
        // update_exchange_concepts.
        let mut concept_id_cache: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        let locale_tag = self.locale.pack_id();

        for a in assessments {
            let concept_id = if let Some(&id) = concept_id_cache.get(&a.concept) {
                id
            } else {
                tx.execute(
                    "INSERT OR IGNORE INTO concepts (name, concept_language_tag) VALUES (?1, ?2)",
                    rusqlite::params![a.concept, locale_tag],
                )
                .map_err(|e| {
                    PrimerError::Storage(format!("save_comprehensions: upsert concept: {e}"))
                })?;
                let id: i64 = tx
                    .query_row(
                        "SELECT id FROM concepts WHERE name = ?1",
                        rusqlite::params![a.concept],
                        |r| r.get(0),
                    )
                    .map_err(|e| {
                        PrimerError::Storage(format!("save_comprehensions: select concept: {e}"))
                    })?;
                concept_id_cache.insert(a.concept.clone(), id);
                id
            };
            let depth_id = crate::catalog::understanding_depth_id(a.depth);

            tx.execute(
                "INSERT OR IGNORE INTO turn_comprehensions \
                     (session_id, turn_id, concept_id, depth_id, confidence, classifier_id, evidence, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    session_id.to_string(),
                    turn_id,
                    concept_id,
                    depth_id,
                    a.confidence,
                    classifier_id,
                    a.evidence.as_deref(),
                    now,
                ],
            )
            .map_err(|e| PrimerError::Storage(format!("save_comprehensions: insert: {e}")))?;
        }

        tx.commit()
            .map_err(|e| PrimerError::Storage(format!("save_comprehensions commit: {e}")))?;
        Ok(())
    }

    pub(super) async fn save_turn_embedding_inner(
        &self,
        session_id: primer_core::conversation::SessionId,
        turn_index: usize,
        model_id: &str,
        dim: usize,
        vec: &[f32],
    ) -> Result<()> {
        if vec.is_empty() || dim == 0 {
            return Err(PrimerError::Storage(
                "save_turn_embedding: empty vec or zero dim".into(),
            ));
        }
        if vec.len() != dim {
            return Err(PrimerError::Storage(format!(
                "save_turn_embedding: vec len {} != declared dim {dim}",
                vec.len()
            )));
        }
        let conn = self.conn.lock().unwrap();
        let model_row = upsert_storage_embedding_model(&conn, model_id, dim)?;
        let turn_id: i64 = conn
            .query_row(
                "SELECT id FROM turns WHERE session_id = ?1 AND turn_index = ?2",
                rusqlite::params![session_id.to_string(), turn_index as i64],
                |r| r.get(0),
            )
            .map_err(|e| {
                PrimerError::Storage(format!(
                    "save_turn_embedding: lookup turn ({session_id}, {turn_index}): {e}"
                ))
            })?;
        let blob = vec_to_blob(vec);
        conn.execute(
            "INSERT INTO embeddings_turns(turn_id, model_id, vec) VALUES (?1, ?2, ?3)
             ON CONFLICT(turn_id) DO UPDATE SET
                model_id = excluded.model_id,
                vec      = excluded.vec",
            rusqlite::params![turn_id, model_row, blob],
        )
        .map_err(|e| PrimerError::Storage(format!("save_turn_embedding: upsert: {e}")))?;
        Ok(())
    }
}
