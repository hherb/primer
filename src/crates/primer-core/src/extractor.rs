//! Extractor types — output and input for the concept extractor.
//!
//! `ConceptExtraction` is one extractor output (child + primer concepts
//! for one completed exchange). `ExtractionContext` is its input —
//! newtype-wrapped so future fields can be added without breaking the
//! trait signature.

use serde::{Deserialize, Serialize};

use crate::conversation::Turn;

/// One extractor output. Concepts the child surfaced and concepts the
/// Primer introduced, separated so each turn's `concepts` field can be
/// populated correctly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConceptExtraction {
    pub child_concepts: Vec<String>,
    pub primer_concepts: Vec<String>,
}

impl ConceptExtraction {
    /// Empty extraction. Used as the soft-fail return on extractor
    /// errors (parse failure, network error, invalid output). Never
    /// shaped as `Err` — failures must not propagate.
    pub fn empty() -> Self {
        Self::default()
    }
}

/// Input to the extractor. Newtype'd so future fields can be added
/// without breaking the trait signature.
pub struct ExtractionContext<'a> {
    /// The child's most recent turn — exactly the turn whose concepts
    /// we want extracted into `child_concepts`.
    pub child_turn: &'a Turn,
    /// The Primer's response to that turn — concepts INTRODUCED by the
    /// Primer end up in `primer_concepts`.
    pub primer_turn: &'a Turn,
    /// A small slice of preceding turns for context (oldest-first).
    /// Helps the LLM disambiguate pronoun references across turns.
    pub recent_turns: &'a [Turn],
    // Reserved for extension. Adding fields here is non-breaking
    // because the trait method takes ExtractionContext by value.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_factory_returns_empty_vecs() {
        let e = ConceptExtraction::empty();
        assert!(e.child_concepts.is_empty());
        assert!(e.primer_concepts.is_empty());
    }
}
