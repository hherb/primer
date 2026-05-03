//! Default values for `ExtractorSettings`. Per the no-magic-numbers
//! convention, every numeric used by the extractor subsystem is
//! defined here (or in a sibling settings struct field).

pub const DEFAULT_BLOCKING_TIMEOUT_MS: u64 = 1500;
pub const DEFAULT_RECENT_CONTEXT_TURNS: usize = 4;

/// Hard cap on the LLM's raw output length before parsing. Generous
/// — truncating valuable concept text is worse than carrying a few
/// hundred bytes of trailing junk into the parser, which the
/// brace-balanced JSON extractor already discards. Sized to fit a
/// double-budget extraction (16 child + 16 primer concepts × the
/// per-concept char cap below, plus JSON framing).
pub const DEFAULT_MAX_EXTRACTOR_OUTPUT_CHARS: usize = 4096;

/// Hard cap on extracted concepts per speaker. Generous — children
/// rarely surface more than a handful per turn, but bumping the
/// ceiling avoids silently dropping concepts when a verbose Primer
/// response introduces several. Truncated post-parse so a runaway
/// list doesn't bloat `concepts` table cardinality.
pub const DEFAULT_MAX_CONCEPTS_PER_SPEAKER: usize = 16;

/// Hard cap on chars per individual concept name. Generous — supports
/// noun phrases like "the second law of thermodynamics" without
/// trimming. The cap exists only to defend against a pathological
/// "concept = entire sentence" output from a misbehaving LLM, not as
/// a brevity-enforcement mechanism.
pub const DEFAULT_PER_CONCEPT_CHARS: usize = 128;

pub const DEFAULT_EXTRACTOR_MAX_TOKENS: u32 = 384;
pub const DEFAULT_EXTRACTOR_TEMPERATURE: f32 = 0.1;
pub const DEFAULT_EXTRACTOR_TOP_P: f32 = 0.9;

/// Char cap on the snippet of unparseable LLM output included in
/// the warn log. Tracing concern only — not user-tunable.
pub const LLM_DEBUG_SNIPPET_CHARS: usize = 256;
