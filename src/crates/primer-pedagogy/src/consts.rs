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

/// Initial confidence assigned to a freshly-encountered concept (depth
/// = `Aware`). 0.5 reads as "we have one positive signal but no
/// comprehension probe yet". Promoted/demoted as the learner model
/// gains evidence; this is just the bootstrap value.
pub const INITIAL_CONCEPT_CONFIDENCE: f32 = 0.5;

/// Context-limit recovery: how many times the dialogue manager retries a
/// truncated turn with a progressively smaller prompt before accepting the
/// partial reply. 2 retries ⇒ at most 3 inference calls
/// (Full → NoKnowledge → Minimal). See
/// docs/superpowers/specs/2026-06-15-context-limit-graceful-recovery-design.md.
pub const MAX_TRUNCATION_RETRIES: usize = 2;

/// Recent-turn window (in turns) for the `Minimal` recovery tier — the
/// tightest prompt, used on the second retry. Capped well below the
/// small-context default so the prompt leaves maximum room for the reply.
pub const MINIMAL_TIER_CONTEXT_WINDOW_TURNS: usize = 4;

/// Separator streamed around an in-turn notice (apology / soft-stop cue)
/// so it reads as a distinct beat rather than gluing onto the partial reply.
pub const TURN_NOTICE_SEPARATOR: &str = "\n\n";
