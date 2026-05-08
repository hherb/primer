//! Default values for invariant numerics shared across primer-core
//! modules. Per the no-magic-numbers convention, every numeric used
//! by primer-core helpers is defined here (or in a sibling settings
//! struct field for tunables).

/// Defaults for the retry helper. Mirrors the per-crate `consts.rs`
/// pattern used by `primer-classifier`, `primer-extractor`, etc.
pub mod retry {
    use std::time::Duration;

    /// Total attempts including the first.
    pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;

    /// Initial backoff delay before the second attempt.
    pub const DEFAULT_BASE_DELAY: Duration = Duration::from_millis(250);

    /// Multiplicative growth factor between attempts (250 → 500 → 1000 ms).
    pub const DEFAULT_BACKOFF_FACTOR: u32 = 2;

    /// Jitter as a fraction of the computed delay (±50 %).
    pub const DEFAULT_JITTER_FRACTION: f32 = 0.5;

    /// Maximum honored Retry-After. Above this we give up immediately
    /// rather than block the conversational hot path.
    pub const DEFAULT_RETRY_AFTER_BUDGET: Duration = Duration::from_secs(5);
}

/// Defaults for the vocabulary spaced-repetition feature.
///
/// See [`crate::vocab`] and the design spec at
/// `docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md`.
pub mod vocab {

    /// Box-level interval table (days). Index = `box_level`.
    /// - box 0 (freshly seen / failed) → review after 1 day
    /// - box 1 (one successful review) → 3 days
    /// - box 2 (two)                    → 7 days
    /// - box 3 (three)                  → 14 days
    /// - box 4 (max — never graduates)  → 30 days
    pub const BOX_INTERVALS_DAYS: &[u32] = &[1, 3, 7, 14, 30];

    /// Highest `box_level` a concept can occupy. After reaching this, further
    /// successful reviews keep `box_level` pinned at MAX (interval stays 30d).
    /// There is no terminal "graduated" state — review continues every 30d
    /// until either the child consistently fails (depth=Aware → box reset)
    /// or the concept is genuinely never engaged with again.
    pub const MAX_BOX_LEVEL: u8 = 4;

    /// Minimum confidence for a comprehension assessment to count toward
    /// box advancement. Assessments below this threshold reset the box to 0.
    /// Numerically equal to the comprehension classifier's
    /// `confidence_threshold` (also 0.6) but kept independent so a future
    /// researcher can tune box-advancement strictness without affecting
    /// depth promotion.
    pub const MIN_CONF_FOR_BOX_PROMOTION: f32 = 0.6;

    /// Default cap on overdue concepts injected into the system prompt
    /// per turn. Configurable via `VocabSettings::max_per_prompt` and the
    /// `--vocab-max-per-prompt` CLI flag.
    pub const DEFAULT_VOCAB_MAX_PER_PROMPT: usize = 4;
}

/// Defaults for the session-time-based break-suggestion feature.
///
/// See [`crate::session_timing`] and the design spec at
/// `docs/superpowers/specs/2026-05-05-session-break-suggestion-design.md`.
pub mod break_suggest {

    /// Minutes between break-suggestion nudges. After this many minutes
    /// of session time (or this many minutes since the last suggestion,
    /// whichever is more recent), the next pedagogical intent decision
    /// returns `SuggestBreak`. Configurable via `PedagogyConfig` and the
    /// `--session-break-after-mins` CLI flag. Must be `>= 1` when enabled
    /// (a value of 0 disables the gate entirely).
    pub const DEFAULT_INTERVAL_MINUTES: u32 = 30;
}

/// Defaults for hybrid retrieval (BM25 + dense-vector RRF). Used by the
/// dialogue manager when an `Embedder` is wired; mirror the shape of
/// [`crate::knowledge::HybridParams`] and feed into it directly.
pub mod retrieval {
    /// BM25 leg top-K for knowledge-base retrieval. Wider than the
    /// final K so RRF has a real candidate pool to fuse over. Tuned
    /// against the 90-passage seed corpus + 87-query benchmark via
    /// the 54-cell hybrid sweep at `tests/retrieval_sweep_hybrid.rs`
    /// (run with `--features fastembed`). Every cell with
    /// `bm25_top_k ∈ {20, 30}` and `final_top_k = 5` achieved 100%
    /// loose / 100% strict recall (lifting the BM25-only strict
    /// miss for "how does the sun shine"). 30 was picked as the
    /// final value to leave headroom for corpus growth — the 50%
    /// candidate-pool bump over the BM25-baseline 20 costs almost
    /// nothing on a corpus this size.
    pub const KB_BM25_TOP_K: usize = 30;

    /// Dense-vector leg top-K for knowledge-base retrieval. Same
    /// rationale as `KB_BM25_TOP_K` — tuned via the hybrid sweep.
    /// Every cell with `bm25_top_k ≥ 20` and `final_top_k = 5` hit
    /// 100/100 across `vector_top_k ∈ {10, 20, 30}`; 30 chosen for
    /// symmetry with the BM25 leg and corpus-growth headroom.
    pub const KB_VECTOR_TOP_K: usize = 30;

    /// Number of fused passages handed to the prompt builder per turn.
    /// Matches the BM25-only fallback path's top-K so the system prompt
    /// stays the same shape regardless of which retrieval mode is live.
    /// Tuned against the 90-passage seed corpus via the sweep at
    /// `tests/retrieval_sweep.rs` — see
    /// `docs/superpowers/specs/2026-05-06-retrieval-tuning-design.md`.
    /// At top_k=5 the BM25 path achieves 100% loose recall and 95%
    /// strict recall on the 87-query benchmark; top_k=3 plateaued at
    /// 95% loose. Going beyond 5 added no further gains.
    ///
    /// **Cost note:** Each retrieved passage is injected into the system
    /// prompt every turn. The 3 → 5 bump adds ~67% more retrieval payload
    /// per turn (~200–500 extra tokens at typical passage length).
    /// Comfortable for cloud Anthropic; revisit when the local llama.cpp
    /// path lands and the context window gets tighter.
    pub const KB_FINAL_TOP_K: usize = 5;

    /// Post-fusion score floor for the KB hybrid path. Zero rather than
    /// `f64::NEG_INFINITY` so the fused list stays positive (RRF
    /// contributions are always > 0) without filtering anything that
    /// appeared in either leg.
    pub const KB_MIN_SCORE: f64 = 0.0;

    /// BM25 leg top-K for long-term-memory (session-turn) retrieval.
    /// Smaller than the KB path because the session corpus is usually
    /// orders of magnitude smaller and the fused candidate set
    /// shouldn't drown the prompt builder in noise.
    pub const LTM_BM25_TOP_K: usize = 10;

    /// Dense-vector leg top-K for long-term-memory retrieval.
    pub const LTM_VECTOR_TOP_K: usize = 10;

    /// Number of fused turns handed back to the dialogue manager.
    pub const LTM_FINAL_TOP_K: usize = 3;

    /// Reciprocal Rank Fusion constant `k`. The published default from
    /// Cormack et al. 2009; works well across many IR domains. Smaller
    /// values weight the very top of each list more, larger values
    /// flatten the curve. Confirmed by the 54-cell hybrid sweep:
    /// at `bm25_top_k ≥ 20, final_top_k = 5`, recall is invariant
    /// across `rrf_k ∈ {30, 60, 90}` on this corpus — the canonical
    /// 60 stays.
    pub const RRF_K: f64 = 60.0;

    /// Minimum BM25 score for the BM25-only knowledge-base path
    /// (the fallback when no embedder is wired). Higher = stricter,
    /// fewer noisy hits. The sweep at `tests/retrieval_sweep.rs`
    /// against the 90-passage seed corpus showed every value in
    /// {0.0, 0.25, 0.5, 0.75, 1.0, 1.5} produces identical recall —
    /// every BM25 score in this corpus comfortably exceeds 1.5.
    /// Kept at 0.5 as a defensive floor: a no-op today, but bites
    /// if a future larger corpus dilutes term frequencies and pushes
    /// scores down. See
    /// `docs/superpowers/specs/2026-05-06-retrieval-tuning-design.md`.
    pub const KB_BM25_ONLY_MIN_SCORE: f64 = 0.5;
}
