//! Canonical mappings between Rust enum variants and their on-disk
//! integer IDs. This module is the single source of truth for the
//! contents of the `speakers`, `pedagogical_intents`, and
//! `engagement_states` lookup tables.

use primer_core::conversation::{PedagogicalIntent, Speaker};
use primer_core::error::{PrimerError, Result};
use primer_core::learner::EngagementState;
use rusqlite::{params, Connection, OptionalExtension};

/// Stable on-disk integer ID for every `Speaker` variant.
///
/// Explicit values (no `as i64`) so reordering variants in `primer-core`
/// cannot silently shift IDs already on disk.
pub fn speaker_id(speaker: Speaker) -> i64 {
    match speaker {
        Speaker::Child => 1,
        Speaker::Primer => 2,
    }
}

/// Human-readable name stored alongside the ID. Used by the
/// validate-and-seed pass and useful for debugging.
pub fn speaker_name(speaker: Speaker) -> &'static str {
    match speaker {
        Speaker::Child => "Child",
        Speaker::Primer => "Primer",
    }
}

/// Stable on-disk integer ID for every `PedagogicalIntent` variant.
pub fn intent_id(intent: PedagogicalIntent) -> i64 {
    match intent {
        PedagogicalIntent::SocraticQuestion => 1,
        PedagogicalIntent::ComprehensionCheck => 2,
        PedagogicalIntent::Scaffolding => 3,
        PedagogicalIntent::Encouragement => 4,
        PedagogicalIntent::Extension => 5,
        PedagogicalIntent::DirectAnswer => 6,
        PedagogicalIntent::AnswerThenPivot => 7,
        PedagogicalIntent::SessionClose => 8,
    }
}

pub fn intent_name(intent: PedagogicalIntent) -> &'static str {
    match intent {
        PedagogicalIntent::SocraticQuestion => "SocraticQuestion",
        PedagogicalIntent::ComprehensionCheck => "ComprehensionCheck",
        PedagogicalIntent::Scaffolding => "Scaffolding",
        PedagogicalIntent::Encouragement => "Encouragement",
        PedagogicalIntent::Extension => "Extension",
        PedagogicalIntent::DirectAnswer => "DirectAnswer",
        PedagogicalIntent::AnswerThenPivot => "AnswerThenPivot",
        PedagogicalIntent::SessionClose => "SessionClose",
    }
}

/// Reverse lookup: integer ID → `PedagogicalIntent`. Returns `None` for
/// IDs the current build doesn't know about — used by `load_session` and
/// `retrieve_session_turns` to decode persisted turns.
pub fn intent_from_id(id: i64) -> Option<PedagogicalIntent> {
    PedagogicalIntent::ALL
        .iter()
        .copied()
        .find(|v| intent_id(*v) == id)
}

pub fn speaker_from_id(id: i64) -> Option<Speaker> {
    Speaker::ALL.iter().copied().find(|v| speaker_id(*v) == id)
}

/// All `(id, name)` pairs the storage layer expects to see in the
/// `speakers` lookup table. Used by the validate-and-seed pass.
pub fn expected_speakers() -> Vec<(i64, &'static str)> {
    Speaker::ALL
        .iter()
        .map(|s| (speaker_id(*s), speaker_name(*s)))
        .collect()
}

/// All `(id, name)` pairs the storage layer expects to see in the
/// `pedagogical_intents` lookup table.
pub fn expected_intents() -> Vec<(i64, &'static str)> {
    PedagogicalIntent::ALL
        .iter()
        .map(|i| (intent_id(*i), intent_name(*i)))
        .collect()
}

/// Stable on-disk integer ID for every `EngagementState` variant.
///
/// Explicit match arms so reordering variants in `primer-core`
/// cannot silently shift IDs already on disk.
pub fn engagement_state_id(state: EngagementState) -> i64 {
    match state {
        EngagementState::Engaged => 1,
        EngagementState::Reflecting => 2,
        EngagementState::FrustratedStuck => 3,
        EngagementState::FrustratedTrying => 4,
        EngagementState::Disengaging => 5,
        EngagementState::Unknown => 6,
    }
}

/// Human-readable name stored alongside the ID in the `engagement_states`
/// lookup table.
pub fn engagement_state_name(state: EngagementState) -> &'static str {
    match state {
        EngagementState::Engaged => "Engaged",
        EngagementState::Reflecting => "Reflecting",
        EngagementState::FrustratedStuck => "FrustratedStuck",
        EngagementState::FrustratedTrying => "FrustratedTrying",
        EngagementState::Disengaging => "Disengaging",
        EngagementState::Unknown => "Unknown",
    }
}

/// Reverse lookup: integer ID → `EngagementState`. Returns `None` for
/// IDs the current build doesn't know about.
pub fn engagement_state_from_id(id: i64) -> Option<EngagementState> {
    EngagementState::ALL
        .iter()
        .copied()
        .find(|v| engagement_state_id(*v) == id)
}

/// All `(id, name)` pairs the storage layer expects to see in the
/// `engagement_states` lookup table. Used by the validate-and-seed pass.
pub fn expected_engagement_states() -> Vec<(i64, &'static str)> {
    EngagementState::ALL
        .iter()
        .map(|s| (engagement_state_id(*s), engagement_state_name(*s)))
        .collect()
}

/// Look up an existing `classifiers` row by `identifier`, or insert one
/// and return the freshly-assigned `AUTOINCREMENT` id.
///
/// This is the single choke-point for mapping a classifier identifier
/// string (e.g. `"stub"` or `"llm:claude-haiku-4-5"`) to its integer FK
/// used in `turn_classifications`. Callers never need to know whether the
/// row was created now or already existed.
pub(crate) fn get_or_create_classifier_id(conn: &Connection, identifier: &str) -> Result<i64> {
    if let Some(id) = conn
        .query_row(
            "SELECT id FROM classifiers WHERE identifier = ?1",
            params![identifier],
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| PrimerError::Storage(format!("classifier lookup: {e}")))?
    {
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO classifiers (identifier) VALUES (?1)",
        params![identifier],
    )
    .map_err(|e| PrimerError::Storage(format!("classifier insert: {e}")))?;
    Ok(conn.last_insert_rowid())
}

#[cfg(test)]
mod classifier_row_tests {
    use super::*;
    use rusqlite::Connection;

    fn v3_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::schema::SCHEMA_SQL).unwrap();
        crate::schema::apply_v2_migrations(&conn).unwrap();
        crate::schema::apply_v3_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn get_or_create_classifier_id_creates_on_first_call() {
        let conn = v3_conn();
        let id1 = get_or_create_classifier_id(&conn, "stub").unwrap();
        let id2 = get_or_create_classifier_id(&conn, "stub").unwrap();
        assert_eq!(id1, id2, "second call must return the same id");
    }

    #[test]
    fn get_or_create_classifier_id_handles_distinct_identifiers() {
        let conn = v3_conn();
        let stub = get_or_create_classifier_id(&conn, "stub").unwrap();
        let llm = get_or_create_classifier_id(&conn, "llm:claude-haiku-4-5").unwrap();
        assert_ne!(stub, llm);
    }
}

#[cfg(test)]
mod engagement_state_tests {
    use super::*;
    use primer_core::learner::EngagementState;

    #[test]
    fn engagement_state_id_is_stable_for_every_variant() {
        // Document the canonical id->variant mapping. If you ever
        // need to change these ids, retired ones must NEVER be reused.
        assert_eq!(engagement_state_id(EngagementState::Engaged), 1);
        assert_eq!(engagement_state_id(EngagementState::Reflecting), 2);
        assert_eq!(engagement_state_id(EngagementState::FrustratedStuck), 3);
        assert_eq!(engagement_state_id(EngagementState::FrustratedTrying), 4);
        assert_eq!(engagement_state_id(EngagementState::Disengaging), 5);
        assert_eq!(engagement_state_id(EngagementState::Unknown), 6);
    }

    #[test]
    fn engagement_state_from_id_is_inverse_of_id() {
        for state in EngagementState::ALL {
            let id = engagement_state_id(*state);
            assert_eq!(engagement_state_from_id(id), Some(*state));
        }
    }

    #[test]
    fn engagement_state_from_id_returns_none_on_unknown_id() {
        assert!(engagement_state_from_id(999).is_none());
    }

    #[test]
    fn expected_engagement_states_covers_all_variants() {
        let pairs = expected_engagement_states();
        assert_eq!(pairs.len(), EngagementState::ALL.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_speaker_round_trips_through_id() {
        for &s in Speaker::ALL {
            assert_eq!(speaker_from_id(speaker_id(s)), Some(s));
        }
    }

    #[test]
    fn every_intent_round_trips_through_id() {
        for &i in PedagogicalIntent::ALL {
            assert_eq!(intent_from_id(intent_id(i)), Some(i));
        }
    }

    #[test]
    fn intent_ids_are_unique() {
        let mut ids: Vec<i64> = PedagogicalIntent::ALL
            .iter()
            .map(|i| intent_id(*i))
            .collect();
        ids.sort();
        let len = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), len, "intent_id assigned duplicate IDs");
    }

    #[test]
    fn speaker_ids_are_unique() {
        let mut ids: Vec<i64> = Speaker::ALL.iter().map(|s| speaker_id(*s)).collect();
        ids.sort();
        let len = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), len);
    }

    #[test]
    fn unknown_intent_id_returns_none() {
        assert_eq!(intent_from_id(9999), None);
    }
}
