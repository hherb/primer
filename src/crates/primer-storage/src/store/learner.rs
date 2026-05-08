//! `LearnerStore` trait impl for `SqliteSessionStore`.
//!
//! Save/load round-trip for the `LearnerModel` umbrella struct. Both
//! methods take a single connection lock for the duration of the
//! transaction. `load_learner` validates every integer narrowing
//! through `super::conv::*` so corrupt-DB reads surface a typed error
//! rather than silent overflow.

use async_trait::async_trait;
use primer_core::error::{PrimerError, Result};
use rusqlite::OptionalExtension;
use uuid::Uuid;

use super::SqliteSessionStore;
use super::conv::{i64_to_u8, i64_to_u32, i64_to_u64, parse_rfc3339};

#[async_trait]
impl primer_core::storage::LearnerStore for SqliteSessionStore {
    async fn save_learner(&self, learner: &primer_core::learner::LearnerModel) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| PrimerError::Storage(format!("save_learner begin tx: {e}")))?;

        // 1. Upsert the learners row. Use proper UPSERT (ON CONFLICT DO
        // UPDATE) so we do NOT cascade-wipe learner_concepts via the FK.
        // INSERT OR REPLACE would do exactly that — see the save_session
        // notes for the same footgun.
        let languages_json = serde_json::to_string(&learner.profile.languages)
            .map_err(|e| PrimerError::Storage(format!("encode languages: {e}")))?;
        let topics_json = serde_json::to_string(&learner.preferences.high_engagement_topics)
            .map_err(|e| PrimerError::Storage(format!("encode high_engagement_topics: {e}")))?;
        let early_secs = learner.preferences.early_disengagement_threshold.as_secs() as i64;
        let engagement_state_id = crate::catalog::engagement_state_id(learner.current_engagement);

        tx.execute(
            "INSERT INTO learners (
                 id, name, age, languages, created_at, last_active,
                 pref_narrative, pref_socratic, pref_visual, pref_kinesthetic,
                 typical_session_minutes, high_engagement_topics,
                 early_disengagement_secs, current_engagement_state_id, locale
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 age = excluded.age,
                 languages = excluded.languages,
                 last_active = excluded.last_active,
                 pref_narrative = excluded.pref_narrative,
                 pref_socratic = excluded.pref_socratic,
                 pref_visual = excluded.pref_visual,
                 pref_kinesthetic = excluded.pref_kinesthetic,
                 typical_session_minutes = excluded.typical_session_minutes,
                 high_engagement_topics = excluded.high_engagement_topics,
                 early_disengagement_secs = excluded.early_disengagement_secs,
                 current_engagement_state_id = excluded.current_engagement_state_id,
                 locale = excluded.locale",
            rusqlite::params![
                learner.profile.id.to_string(),
                learner.profile.name,
                learner.profile.age as i64,
                languages_json,
                learner.profile.created_at.to_rfc3339(),
                learner.profile.last_active.to_rfc3339(),
                learner.preferences.narrative as f64,
                learner.preferences.socratic as f64,
                learner.preferences.visual as f64,
                learner.preferences.kinesthetic as f64,
                learner.preferences.typical_session_minutes as f64,
                topics_json,
                early_secs,
                engagement_state_id,
                learner.profile.locale.pack_id(),
            ],
        )
        .map_err(|e| PrimerError::Storage(format!("upsert learner: {e}")))?;

        // 2. For each concept, ensure the concepts row exists and upsert
        //    learner_concepts.
        if !learner.concepts.is_empty() {
            let mut insert_concept = tx
                .prepare(
                    "INSERT OR IGNORE INTO concepts (name, concept_language_tag) VALUES (?1, ?2)",
                )
                .map_err(|e| PrimerError::Storage(format!("prepare insert concept: {e}")))?;
            let mut select_concept = tx
                .prepare("SELECT id FROM concepts WHERE name = ?1")
                .map_err(|e| PrimerError::Storage(format!("prepare select concept: {e}")))?;
            let mut upsert_lc = tx
                .prepare(
                    "INSERT INTO learner_concepts (
                         learner_id, concept_id, depth_id, confidence,
                         encounter_count, last_encountered, notes, box_level
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(learner_id, concept_id) DO UPDATE SET
                         depth_id = excluded.depth_id,
                         confidence = excluded.confidence,
                         encounter_count = excluded.encounter_count,
                         last_encountered = excluded.last_encountered,
                         notes = excluded.notes,
                         box_level = excluded.box_level",
                )
                .map_err(|e| PrimerError::Storage(format!("prepare upsert lc: {e}")))?;

            // Per-call cache to skip re-querying concepts within one save.
            let mut concept_id_cache: std::collections::HashMap<String, i64> =
                std::collections::HashMap::new();
            let locale_tag = self.locale.pack_id();

            for concept in &learner.concepts {
                let cid = match concept_id_cache.get(&concept.concept_id).copied() {
                    Some(id) => id,
                    None => {
                        insert_concept
                            .execute(rusqlite::params![concept.concept_id, locale_tag])
                            .map_err(|e| {
                                PrimerError::Storage(format!(
                                    "upsert concept {}: {e}",
                                    concept.concept_id
                                ))
                            })?;
                        let id: i64 = select_concept
                            .query_row(rusqlite::params![concept.concept_id], |r| r.get(0))
                            .map_err(|e| {
                                PrimerError::Storage(format!(
                                    "select concept {}: {e}",
                                    concept.concept_id
                                ))
                            })?;
                        concept_id_cache.insert(concept.concept_id.clone(), id);
                        id
                    }
                };

                let notes_json = serde_json::to_string(&concept.notes)
                    .map_err(|e| PrimerError::Storage(format!("encode notes: {e}")))?;
                let last_encountered = concept.last_encountered.map(|t| t.to_rfc3339());

                upsert_lc
                    .execute(rusqlite::params![
                        learner.profile.id.to_string(),
                        cid,
                        crate::catalog::understanding_depth_id(concept.depth),
                        concept.confidence as f64,
                        concept.encounter_count as i64,
                        last_encountered,
                        notes_json,
                        concept.box_level as i64,
                    ])
                    .map_err(|e| PrimerError::Storage(format!("upsert learner_concept: {e}")))?;
            }

            drop(upsert_lc);
            drop(select_concept);
            drop(insert_concept);
        }

        tx.commit()
            .map_err(|e| PrimerError::Storage(format!("save_learner commit: {e}")))?;
        Ok(())
    }

    async fn load_learner(&self) -> Result<Option<primer_core::learner::LearnerModel>> {
        use primer_core::learner::{
            ConceptState, LearnerModel, LearnerProfile, LearningPreferences,
        };
        use std::time::Duration;

        let conn = self.conn.lock().unwrap();

        // Step 1: the learners row.
        type LearnerRow = (
            String,
            String,
            i64,
            String,
            String,
            String,
            f64,
            f64,
            f64,
            f64,
            f64,
            String,
            i64,
            i64,
            String,
        );
        // The application invariant is one learner per DB file (the file
        // path is the identity boundary), so any row here is THE learner.
        // `ORDER BY id` is defensive: if a future bug or test fixture ever
        // inserts a second row, we deterministically pick the lowest id
        // rather than relying on SQLite's undefined no-ORDER-BY ordering.
        let row: Option<LearnerRow> = conn
            .query_row(
                "SELECT id, name, age, languages, created_at, last_active,
                        pref_narrative, pref_socratic, pref_visual, pref_kinesthetic,
                        typical_session_minutes, high_engagement_topics,
                        early_disengagement_secs, current_engagement_state_id,
                        locale
                 FROM learners ORDER BY id LIMIT 1",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                        r.get(10)?,
                        r.get(11)?,
                        r.get(12)?,
                        r.get(13)?,
                        r.get(14)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| PrimerError::Storage(format!("load_learner select: {e}")))?;

        let Some((
            id_str,
            name,
            age,
            languages_json,
            created_str,
            last_active_str,
            pref_narrative,
            pref_socratic,
            pref_visual,
            pref_kinesthetic,
            typical_session_minutes,
            topics_json,
            early_secs,
            engagement_state_id,
            locale_str,
        )) = row
        else {
            return Ok(None);
        };

        let id = Uuid::parse_str(&id_str)
            .map_err(|e| PrimerError::Storage(format!("parse learner id {id_str}: {e}")))?;
        let languages: Vec<String> = serde_json::from_str(&languages_json)
            .map_err(|e| PrimerError::Storage(format!("decode languages: {e}")))?;
        let high_engagement_topics: Vec<String> = serde_json::from_str(&topics_json)
            .map_err(|e| PrimerError::Storage(format!("decode high_engagement_topics: {e}")))?;
        let created_at = parse_rfc3339(&created_str, "learners.created_at")?;
        let last_active = parse_rfc3339(&last_active_str, "learners.last_active")?;
        let current_engagement = crate::catalog::engagement_state_from_id(engagement_state_id)
            .ok_or_else(|| {
                PrimerError::Storage(format!(
                    "unknown engagement_state_id {engagement_state_id} on learners row"
                ))
            })?;

        // Defensive integer narrowing: a corrupt or hostile DB row must
        // produce a clear `Storage` error rather than silently truncate.
        // Sources of badness include: a future schema migration that
        // widens an int column without updating the loader, manual
        // sqlite3 edits, a third-party tool writing the file, or a
        // hardware-level bit flip. The `as` cast on i64 → u8/u32 wraps
        // mod 2^N — that's the wrong failure mode here.
        // Decode the locale pack id. Unknown ids on disk shouldn't happen
        // in normal operation (the column is constrained at write time to
        // pack ids `Locale::pack_id()` produced) but defensively fall
        // back to `Locale::default()` and log a warning rather than
        // erroring — a corrupted locale shouldn't make a child unable
        // to resume their session.
        let locale = primer_core::i18n::Locale::from_pack_id(&locale_str).unwrap_or_else(|| {
            tracing::warn!(
                "unknown learners.locale {:?} in DB; defaulting to {}",
                locale_str,
                primer_core::i18n::Locale::default().pack_id()
            );
            primer_core::i18n::Locale::default()
        });

        let profile = LearnerProfile {
            id,
            name,
            age: i64_to_u8(age, "learners.age")?,
            languages,
            locale,
            created_at,
            last_active,
        };
        // Float narrowing (f64 → f32) is left as `as`. Rust's `as`
        // semantics saturate to ±infinity on overflow, which is loud
        // (NaN/inf will surface in any downstream comparison) and the
        // values we store are bounded f32-range by construction.
        let early_disengagement_secs = i64_to_u64(early_secs, "learners.early_disengagement_secs")?;
        let preferences = LearningPreferences {
            narrative: pref_narrative as f32,
            socratic: pref_socratic as f32,
            visual: pref_visual as f32,
            kinesthetic: pref_kinesthetic as f32,
            typical_session_minutes: typical_session_minutes as f32,
            high_engagement_topics,
            early_disengagement_threshold: Duration::from_secs(early_disengagement_secs),
        };

        // Step 2: every learner_concepts row, joined to concepts for the
        // string concept_id.
        let mut stmt = conn
            .prepare(
                "SELECT c.name, lc.depth_id, lc.confidence, lc.encounter_count,
                        lc.last_encountered, lc.notes, lc.box_level
                 FROM learner_concepts lc
                 JOIN concepts c ON c.id = lc.concept_id
                 WHERE lc.learner_id = ?1",
            )
            .map_err(|e| PrimerError::Storage(format!("prepare load_learner concepts: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params![id.to_string()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, f64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, i64>(6)?,
                ))
            })
            .map_err(|e| PrimerError::Storage(format!("query learner_concepts: {e}")))?;

        let mut concepts: Vec<ConceptState> = Vec::new();
        for row in rows {
            let (
                concept_name,
                depth_id,
                confidence,
                encounter_count,
                last_encountered_opt,
                notes_json,
                box_level_raw,
            ) = row.map_err(|e| PrimerError::Storage(format!("read learner_concepts: {e}")))?;
            let depth = crate::catalog::understanding_depth_from_id(depth_id)
                .ok_or_else(|| PrimerError::Storage(format!("unknown depth_id {depth_id}")))?;
            let last_encountered = last_encountered_opt
                .as_deref()
                .map(|s| parse_rfc3339(s, "learner_concepts.last_encountered"))
                .transpose()?;
            let notes: Vec<String> = serde_json::from_str(&notes_json)
                .map_err(|e| PrimerError::Storage(format!("decode notes: {e}")))?;
            let box_level: u8 = box_level_raw.try_into().map_err(|_| {
                PrimerError::Storage(format!(
                    "learner_concepts.box_level out of u8 range: {box_level_raw}"
                ))
            })?;
            concepts.push(ConceptState {
                concept_id: concept_name,
                depth,
                confidence: confidence as f32,
                encounter_count: i64_to_u32(encounter_count, "learner_concepts.encounter_count")?,
                last_encountered,
                notes,
                box_level,
            });
        }

        Ok(Some(LearnerModel {
            profile,
            concepts,
            preferences,
            current_engagement,
            recent_assessments: vec![],
        }))
    }
}
