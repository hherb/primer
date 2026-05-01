//! Pedagogy-layer constants. Per the no-magic-numbers convention,
//! invariant numeric values live here (not inline).

/// Word-count threshold below which a child's last turn routes to
/// ComprehensionCheck (instead of Extension or SocraticQuestion).
/// Currently the same heuristic the placeholder used; kept stable here
/// until comprehension assessment lands as a separate Phase 0.3 piece.
pub const SHORT_TURN_WORD_BOUNDARY: usize = 10;

/// How many recent turns `extract_active_concepts` scans for active
/// concepts when deciding Extension intent.
pub const ACTIVE_CONCEPT_LOOKBACK: usize = 4;
