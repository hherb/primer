# Handoff — Hybrid Retrieval Defaults Tuned (Phase 0.2.5 closeout)

**Date:** 2026-05-08 21:14 +0800
**Session focus:** Run the 54-cell hybrid sweep; pick winning cell; land hybrid retrieval defaults; add regression test (#39); close #42.
**Branch:** `feature/hybrid-retrieval-tuning` (3 commits ahead of `c26ddc1`, the merge commit reconciling local `main` with origin's PR #43 + PR #47 squash merges).

## What was shipped

Three commits land Phase 0.2.5 closure:

| SHA       | Subject                                                          |
|-----------|------------------------------------------------------------------|
| `bd7ad6f` | refactor(retrieval): tune HybridParams defaults to (30, 30, 5, 60) |
| `ad80012` | test(retrieval): add hybrid retrieval-quality regression test    |
| `9206166` | docs: Phase 0.2.5 closeout (hybrid retrieval defaults landed)    |

### Tuning decision

The 54-cell hybrid sweep ran end-to-end against BGE-M3 over the 87-query / 20-canonical benchmark. Result table summary:

- All `final_top_k = 3` cells plateau at 95% strict (the seed:en:sun miss persists in top-3).
- All `final_top_k = 5` cells with `bm25_top_k ≥ 20` achieve **100% loose / 100% strict recall** across all `(vector_top_k, rrf_k)` combinations.
- `bm25_top_k = 10` cells fail when widened: `(10, 20, 5, *)` and `(10, 30, 5, *)` drop strict recall to 95%; `(10, 10, 5, 30)` and `(10, 10, 5, 90)` drop loose to 99%.

17 cells tied at 100/100. User chose `(30, 30, 5, 60)` — the harness's notional winner — for headroom against future corpus growth. The 50% wider candidate pool over the BM25-baseline 20 costs almost nothing on a 90-passage corpus and gives us margin before tuning becomes load-bearing again.

### Code changes

- **`primer-core/src/consts.rs`:** `KB_BM25_TOP_K: 20 → 30`, `KB_VECTOR_TOP_K: 20 → 30`. Doc comments updated to record the sweep result and the rationale for the chosen cell. `KB_FINAL_TOP_K = 5` and `RRF_K = 60.0` doc comments augmented with sweep findings (recall is invariant across `rrf_k ∈ {30, 60, 90}` at the chosen leg widths).
- **`primer-core/src/knowledge.rs`:** `HybridParams::default()` rewired to read from `consts::retrieval`. New `hybrid_default_mirrors_consts` test in the existing `retrieval_params_tests` module mirrors the `default_mirrors_consts` pattern for `RetrievalParams`.
- **`primer-kb-load/tests/retrieval_quality_hybrid.rs`** (new): two-flavoured regression test.
  - `hybrid_retrieval_structural_with_stub` — always built; uses `StubEmbedder`. Asserts non-empty hits, ≤ `final_top_k`, idempotent. NO recall floor (the stub embedder produces orthogonal vectors for synonyms; a recall assertion would measure noise).
  - `hybrid_retrieval_recall_with_fastembed` — `#[cfg(feature = "fastembed")]`; uses real BGE-M3. Pins 100% loose and 100% strict at `HybridParams::default()`. Drops break CI under the feature.
- **`primer-kb-load/tests/common/mod.rs`:** Added `KNOWN_FAILING_QUERIES_HYBRID` (empty today). Extended `known_failing_queries_all_appear_in_queries` sanity test to validate both BM25 and hybrid known-failing lists reference real benchmark queries.

### Issue housekeeping

- **#42 closed** — "Corpus gap: BM25 retrieval cannot place seed:en:sun in top-5 for 'how does the sun shine'". The hybrid path now achieves 20/20 strict recall on this query. BM25-only fallback still has the gap; comment on the issue notes that hybrid is the production path.
- **#39 closed** — "primer-kb-load: gate canonical-query test with hybrid retrieval (BM25 + vector)". Closed via the new `retrieval_quality_hybrid.rs`. Comment summarises the two-flavour design and notes that `hybrid_retrieval_recall_with_fastembed` is the truth signal for future `HybridParams::default()` tuning.

### Documentation

- **README.md** — updated the "tuned BM25 retrieval defaults" line to "tuned BM25-only AND hybrid retrieval defaults" with the hybrid-result detail; updated the closing Phase 0.2/0.3 status paragraph.
- **ROADMAP.md** — added a new "✅ Tune `HybridParams` defaults via the 54-cell hybrid sweep" entry alongside the existing BM25 tuning entry.
- **CLAUDE.md** — replaced the "BM25-only retrieval defaults are tuned" gotcha with a unified one covering both BM25-only and hybrid; updated the line listing what's done in Phase 0.2/0.3.

## Verification

| Check                                               | Result            |
|-----------------------------------------------------|-------------------|
| `cargo test --workspace`                            | 591 passed, 0 failed |
| `cargo test --workspace --features primer-kb-load/fastembed` | 597 passed, 0 failed |
| `cargo clippy --workspace --all-targets`            | clean             |
| `cargo clippy --workspace --all-targets --features primer-kb-load/fastembed` | clean |
| `cargo fmt --all -- --check`                        | clean             |
| Hybrid sweep recall at `(30, 30, 5, 60)`            | 100% loose / 100% strict (87/87, 20/20) |

## Acceptance criteria (from prior NEXT_SESSION.md)

| Criterion                                                                              | Status |
|----------------------------------------------------------------------------------------|--------|
| 1. Sweep table shows a clear winner across `(bm25_top_k × vector_top_k × final_top_k × rrf_k)` | ✅ 17 cells tied at 100/100; user chose `(30, 30, 5, 60)` from the tie group |
| 2. Strict recall ≥ 95% (matching BM25 baseline) and ideally lifts the one strict miss tracked in #42 | ✅ Strict recall = 100%; #42 closed |
| 3. Update `KB_BM25_TOP_K`, `KB_VECTOR_TOP_K`, `RRF_K` in `consts.rs` to the chosen cell | ✅ `KB_BM25_TOP_K: 30`, `KB_VECTOR_TOP_K: 30`, `RRF_K: 60.0` (already at target value) |
| 4. Optional: address #39 with a hybrid regression test                                  | ✅ Landed; #39 closed |

## Risks introduced this session

- **`HybridParams::default()` now indirects through `consts::retrieval`.** Adding a new field to `HybridParams` means extending the `hybrid_default_mirrors_consts` test in lockstep. The drift test today covers six fields.
- **Hybrid recall regression test downloads ~570 MB on first run.** Subsequent runs are warm. Until CI validates the cdn.pyke.io path, the recall test runs only on demand under `--features fastembed`. The structural-stub flavour runs in default CI and catches transport-layer regressions.

## Patterns confirmed

- **Always pin `Default` impls of public structs to `consts::*` with a drift-prevention test.** This session re-applied the same fix that landed for `RetrievalParams::default()` in PR #43, on `HybridParams::default()`. Any new `Default for X` where `X` is a public tuning knob should follow this pattern: read from `consts`, add a `default_mirrors_consts`-style test. Drift between code and consts is a silent failure mode.

## Test count change

| Configuration                                    | Before (after PR #47 merge) | After this session | Delta |
|--------------------------------------------------|-----------------------------|--------------------|-------|
| Default features                                 | 584                         | 591                | +7    |
| `--features primer-kb-load/fastembed`            | 584                         | 597                | +13   |

Default-features +7:
- 1 new in `primer-core::knowledge::retrieval_params_tests::hybrid_default_mirrors_consts`.
- 6 in the new `retrieval_quality_hybrid` binary (5 sanity tests via `mod common` re-import + 1 `hybrid_retrieval_structural_with_stub`).

`--features fastembed` adds another 6 over the default-feature delta:
- 5 sanity tests via `mod common` re-import in the now-compiling `retrieval_sweep_hybrid` binary.
- 1 `hybrid_retrieval_recall_with_fastembed`.

(The actual sweep test in `retrieval_sweep_hybrid` is `#[ignore]`'d so it doesn't run by default even under the feature.)
