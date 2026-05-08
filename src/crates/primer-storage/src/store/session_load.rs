//! `SessionStore` load side: load_session, recent classifier assessments, and the most-recent learner-id helper.
//!
//! Inherent `pub(super) async fn *_inner` methods on `SqliteSessionStore`.
//! The trait dispatch lives in `super::session`; each trait method is a
//! one-line delegation to the matching `_inner`. Keeps the trait surface
//! tiny and the heavy bodies grouped by responsibility.

use primer_core::error::{PrimerError, Result};
use rusqlite::OptionalExtension;
use uuid::Uuid;

use super::SqliteSessionStore;
use super::conv::parse_rfc3339;

impl SqliteSessionStore {
    pub(super) async fn load_session_inner(
        &self,
        id: Uuid,
    ) -> Result<Option<primer_core::conversation::Session>> {
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
                let speaker = crate::catalog::speaker_from_id(speaker_id).ok_or_else(|| {
                    PrimerError::Storage(format!("unknown speaker_id {speaker_id}"))
                })?;
                let intent =
                    match intent_id {
                        None => None,
                        Some(id) => Some(crate::catalog::intent_from_id(id).ok_or_else(|| {
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

    pub(super) async fn load_recent_assessments_inner(
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
                let Some(state) = crate::catalog::engagement_state_from_id(state_id) else {
                    // Row written by a newer build that knows an EngagementState
                    // variant this build does not. Drop the row but make the
                    // skew visible so the operator can spot version-rollback
                    // issues; otherwise rehydrated history would silently shrink.
                    tracing::warn!(
                        engagement_state_id = state_id,
                        classifier = classifier_identifier,
                        "load_recent_assessments: dropping row with unknown engagement_state_id (DB written by newer build?)"
                    );
                    return None;
                };
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

    pub(super) async fn most_recent_session_learner_id_inner(&self) -> Result<Option<Uuid>> {
        let conn = self.conn.lock().unwrap();
        let row: Option<String> = conn
            .query_row(
                "SELECT learner_id FROM sessions ORDER BY started_at DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| PrimerError::Storage(format!("most_recent_session_learner_id: {e}")))?;

        match row {
            None => Ok(None),
            Some(s) => {
                let uuid = Uuid::parse_str(&s)
                    .map_err(|e| PrimerError::Storage(format!("parse learner_id {s}: {e}")))?;
                Ok(Some(uuid))
            }
        }
    }
}
