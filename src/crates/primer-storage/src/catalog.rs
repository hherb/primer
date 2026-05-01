//! Canonical mappings between Rust enum variants and their on-disk
//! integer IDs. This module is the single source of truth for the
//! contents of the `speakers` and `pedagogical_intents` lookup tables.

use primer_core::conversation::{PedagogicalIntent, Speaker};

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
