//! Defaults for hybrid retrieval (BM25 + dense-vector RRF). Used by the
//! dialogue manager when an `Embedder` is wired; mirror the shape of
//! [`crate::knowledge::HybridParams`] and feed into it directly.

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

/// Fused-passage count handed to the prompt builder when the active
/// backend is a small-context (≈4K-token) backend
/// ([`crate::backend::is_small_context_backend`]). Three passages keep
/// the per-turn retrieval payload small enough to leave context-window
/// headroom for the conversation history under a 4K budget. Measured
/// cost of the `5 → 3` shrink at the production `min_score = 0.5`
/// (BM25-only sweeps, `primer-kb-load/tests/retrieval_sweep{,_de}.rs`):
/// EN loose recall 99% → 95% (strict 88% unchanged); DE loose 90% → 87%
/// (strict 88% → 84%). The handful of additional misses are the
/// already-documented corpus-coverage paraphrase gaps (e.g. the DE
/// gänsehaut / ebbe-und-flut queries), not ranking-depth losses that
/// more passages would recover. See `KB_FINAL_TOP_K` for the
/// large-context default. Phase 1.2 step 1.2.5.
pub const KB_FINAL_TOP_K_SMALL_CONTEXT: usize = 3;

/// Post-fusion score floor for the KB hybrid path. Zero rather than
/// `f64::NEG_INFINITY` so the fused list stays positive (RRF
/// contributions are always > 0) without filtering anything that
/// appeared in either leg.
pub const KB_MIN_SCORE: f64 = 0.0;

/// Embedder model id of the project's default real model (BGE-M3, via
/// fastembed). Lives here so the retrieval calibration below can key on
/// it; `primer_embedding::BGE_M3_MODEL_ID` re-uses this value.
pub const BGE_M3_MODEL_ID: &str = "bge-m3";

/// Off-topic query gate for the KB hybrid path under BGE-M3
/// ([`crate::knowledge::HybridParams::min_top_cosine`], issue #307): a
/// query whose best passage cosine is below this retrieves nothing.
/// Measured on the shipped EN + DE corpora: the 122 benchmark queries
/// have a best cosine of 0.473–0.772 (median 0.652); 33 off-topic
/// utterances (greetings, "can we stop now", "mir ist langweilig")
/// score 0.365–0.473 (median 0.426). The two ranges touch, so this
/// value trades precision for a recall margin: 0.45 loses no benchmark
/// query (0.023 below the lowest) and empties 26 of the 33 off-topic
/// ones; 0.47 would empty 32 but leave almost no margin for a real
/// child's on-topic question. Re-measure with the `#[ignore]`'d
/// `off_topic_gate_probe` in `primer-kb-load/tests/` when the corpus or
/// the model changes.
pub const KB_QUERY_GATE_MIN_COSINE_BGE_M3: f64 = 0.45;

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
/// every *correct* top-K hit comfortably exceeds 1.5, and the
/// sub-1.5 scores that exist are 5th-place noise on marginal
/// queries (no query's best hit drops anywhere near the floor;
/// the worst top-1 score across the 87-query benchmark is 3.35).
/// Kept at 0.5 as a defensive floor: a no-op for recall today,
/// but bites if a future larger corpus dilutes term frequencies
/// and pushes the marginal scores below 0.5. The tripwire at
/// `primer-kb-load/tests/bm25_floor_tripwire.rs` (run with
/// `--ignored`) probes the actual top-K score distribution and
/// fires loudly when the margin closes. See
/// `docs/superpowers/specs/2026-05-06-retrieval-tuning-design.md`.
pub const KB_BM25_ONLY_MIN_SCORE: f64 = 0.5;
