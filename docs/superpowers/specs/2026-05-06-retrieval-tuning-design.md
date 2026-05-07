# Retrieval-parameter tuning (Phase 0.2 closeout)

**Status:** Approved 2026-05-06.
**Author:** Horst Herb + Claude Code (paired brainstorming).
**Phase:** 0.2 — Knowledge-base bootstrapping (final task).
**Predecessor:** [2026-05-05-hybrid-retrieval-design.md](2026-05-05-hybrid-retrieval-design.md), [2026-05-06-wikipedia-ingestion-design.md](2026-05-06-wikipedia-ingestion-design.md).

## Goal

Tune the BM25-only knowledge-base retrieval defaults (`KB_FINAL_TOP_K`, `KB_BM25_ONLY_MIN_SCORE` in `primer_core::consts::retrieval`) against the now-90-passage seed corpus (55 hand-drafted CC0 + 35 Simple Wikipedia CC-BY-SA-3.0). Land the chosen values, update the regression test to assert against them, and ship a `#[ignore]`'d sweep harness so future tuning passes are one command away. Scaffold a hybrid-leg sweep behind `--features fastembed --ignored` so a follow-up session can tune the hybrid defaults against a real embedder without rebuilding the harness.

The motivation is concrete: the existing canonical-query test runs at `top_k=5, min_score=NEG_INFINITY`, which is more permissive than production (`top_k=3, min_score=0.5`). Either the production defaults are too tight (test passes, real children miss the right passage) or the test is too lax (test passes, but doesn't actually validate production behaviour). The corpus is now broad enough to discriminate, so we resolve the gap.

This work also closes Phase 0.2 of the roadmap.

## Non-goals

- **Hybrid path defaults.** `RRF_K`, `KB_BM25_TOP_K`, `KB_VECTOR_TOP_K`, `KB_MIN_SCORE` stay at their current values. Tuning the hybrid path against `StubEmbedder` is structurally meaningless (hash vectors carry no semantic signal, so synonyms map to orthogonal vectors and rrf_k sweeps tune for noise). A real-embedder sweep needs network + ~570 MB of model download + manual review of results — out of scope for this session. Scaffold lands; numbers wait for a session where the user has time to look at them.
- **LTM (long-term memory) tuning.** The benchmark surface for LTM doesn't exist; LTM has session-internal dynamics that a corpus-level benchmark can't simulate. `LTM_BM25_TOP_K`, `LTM_VECTOR_TOP_K`, `LTM_FINAL_TOP_K` stay where they are.
- **Corpus expansion.** If the sweep surfaces queries no parameter cell can satisfy (per the failure-mode policy below), file an issue and stop. Expanding the corpus is a separate task.
- **A new retrieval algorithm.** We are tuning the existing BM25 path's `top_k` and `min_score` knobs, not changing how BM25 ranks or how queries are tokenised.
- **Production-time A/B comparison harness.** No CLI flag or runtime mode — this is a one-shot tuning exercise driven by tests.

## Architecture

Three test files in `src/crates/primer-kb-load/tests/` plus a shared module:

```
tests/
├── common/
│   └── mod.rs                    (new)       — BenchQuery, Cluster, QUERIES const, shared helpers
├── retrieval_quality.rs          (modified)  — regression test, runs in CI
├── retrieval_sweep.rs            (new)       — diagnostic sweep, #[ignore]'d
└── retrieval_sweep_hybrid.rs     (new)       — hybrid-leg sweep, --features fastembed --ignored
```

Each test file uses `mod common;` to pull the shared data and helpers — Cargo's standard pattern for test-shared code (`tests/common/mod.rs` is treated as a module rather than a separate test binary).

Two source files in `src/crates/primer-core/src/`:

```
src/
├── consts.rs       (modified)  — KB_FINAL_TOP_K, KB_BM25_ONLY_MIN_SCORE updated to chosen values
└── knowledge.rs    (modified)  — RetrievalParams::default() aligned with consts (single source of truth)
```

One Python file in `data/ingest/` (issue #37 fix folded in):

```
data/ingest/
└── simple_wikipedia.py    (modified)  — User-Agent contact replaced with my.list.subscriptions@gmail.com
```

The sweep test is inline data + a sweep loop in one file. No new module, no library extraction, no binary harness — see the brainstorming session for the rationale (sweep cadence ≈ once a year; abstraction cost > benefit at this cadence).

## Components

### Expanded benchmark (`retrieval_quality.rs` data)

The current `QUERIES` const is `&[(query, &[required_terms], top_k)]`. It evolves to a struct so per-query metadata can grow:

```rust
#[derive(Debug)]
struct BenchQuery {
    query: &'static str,
    required: &'static [&'static str],   // loose-substring set (existing semantics)
    canonical_id: Option<&'static str>,  // Some = strict-subset member
    cluster: Cluster,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum Cluster {
    Space,
    Body,
    HowThingsWork,
    Life,
    EarthWeather,
    Wiki,
}
```

`top_k` moves out of per-query data — it's a sweep dimension now, not per-query metadata. `Cluster` is closed so the per-cluster recall breakdown is type-safe.

### Query expansion (~58 → ~85)

Three categories of new queries:

- **~10 child-language paraphrases** of existing concepts (e.g. "why does my heart go thump-thump" alongside the existing "how does the heart pump blood"). Tests BM25 robustness against vocabulary drift in real children's question style.
- **~10 cross-cluster questions** (e.g. "why do stars twinkle but planets don't" — touches space + atmosphere; "what makes a baby grow inside its mum" — body + life). Tests whether retrieval handles topic blending without relying on a single dominant term.
- **~5 wiki-only queries** beyond the current 8 (drilling into wiki articles already in the corpus that aren't yet exercised by a query).

Total target: ~85 queries.

### Strict subset (~15–20 entries)

The strict subset is whichever `BenchQuery` entries have `canonical_id = Some`. Construction:

- All 8 wiki-targeted queries get a canonical id (each maps cleanly to a single wiki passage by topic).
- ~10 hand-drafted seed queries where there's an unambiguous canonical passage (one obvious "right" answer in the corpus). I'll grep `data/seed/seed_passages.en.jsonl` to confirm the id per query.

The strict-subset assertion is `canonical_id ∈ {hits[i].id for i in 0..top_k}` — a stronger bar than substring matching. Strict-set construction is a one-time cost; ids are stable across normal seed edits because passage ids are content-addressed (e.g. `seed-body-heart-001`).

### Sweep grid + metric + selection rule

**Grid (24 cells):**
- `top_k` ∈ {3, 5, 7, 10}
- `min_score` ∈ {0.0, 0.25, 0.5, 0.75, 1.0, 1.5}

**Per-cell metrics:**
- **Loose recall** = fraction of all queries where `≥1 required substring` appears in the combined top-k text (existing semantics).
- **Strict recall** = fraction of strict-subset queries where the canonical id is in top-k by id.
- **Per-cluster loose recall** — diagnostic only, surfaces whether one cluster is harder than others.

**Selection rule (lexicographic):**
1. Maximise strict recall (primary signal — this is "did we surface the right passage")
2. Then maximise loose recall (secondary — covers queries we didn't hand-label)
3. Then minimise `top_k` (less prompt noise, smaller context window consumption)
4. Then maximise `min_score` (stronger noise floor against irrelevant passages)

**Output:** the sweep test prints a 24-row table to stdout (captured via `cargo test -- --ignored sweep --nocapture`) showing each cell's metrics. The winning cell is identified explicitly. Per-query failure detail is printed for the winning cell so corpus gaps are easy to enumerate.

### Failure-mode policy

If the winning cell does not hit 100% strict recall, the implementation:

1. Picks the best-achievable cell (per the lexicographic selection rule above).
2. Files a GitHub issue listing every unsatisfied strict-subset query plus its canonical id.
3. Lands the chosen defaults anyway.

The discipline is: tuning is for what tuning can fix. Strict-subset misses indicate corpus gaps (the right passage isn't there or doesn't BM25-retrieve under any threshold), and the corrective is corpus expansion in a separate task — not retuning.

### Hybrid sweep scaffold (`retrieval_sweep_hybrid.rs`)

A `#[cfg(feature = "fastembed")] #[ignore]`-gated test that:
- Loads `FastEmbedBackend` with the default BGE-M3 model (downloaded on first run; warm thereafter).
- Loads the same `BenchQuery` data (shared via `mod common;` in tests/).
- Sweeps `(bm25_top_k, vector_top_k, final_top_k, rrf_k)` over a small grid (~16 cells).
- Prints the same recall + per-cluster table.

This is **infrastructure only this session.** It compiles, it has no `unwrap`s on shapes that depend on hand-tuned numbers, and a successful run end-to-end is part of the commit's verification. **No hybrid defaults change.** A future session can run it, eyeball the table, and update the hybrid consts in a follow-up PR.

### `RetrievalParams::default()` alignment

Currently `RetrievalParams::default()` is `top_k=5, min_score=0.0`, while the production path reads `KB_FINAL_TOP_K=3, KB_BM25_ONLY_MIN_SCORE=0.5` from consts. The two have drifted because nothing today actually invokes the struct's `Default` impl in production code (the dialogue manager builds a struct literal from consts).

After tuning, `RetrievalParams::default()` is updated to mirror whatever the consts become. This is a defensive consistency move — if a future caller does write `RetrievalParams::default()`, they'll get production-faithful defaults. Verified by a unit test that asserts `RetrievalParams::default().top_k == KB_FINAL_TOP_K` and similarly for `min_score`.

### Issue #37: User-Agent contact

One-line change in `data/ingest/simple_wikipedia.py`: replace `PrimerSeedBuilder/0.1 (contact: see-repo-readme)` with `PrimerSeedBuilder/0.1 (contact: my.list.subscriptions@gmail.com)`. The Python tests that pin the User-Agent string (if any) are updated accordingly.

## Testing strategy

**Existing test (`retrieval_quality.rs`):**
- Updated to use the *tuned* `top_k` and `min_score` (whatever the sweep picks). The assertion shape stays the same — combined-top-k loose-substring matching — so a regression in any benchmark query continues to fail CI.
- The `BenchQuery` struct, `Cluster` enum, and the `QUERIES` const live in `tests/common/mod.rs`; this file imports them via `mod common;`.

**New test (`retrieval_sweep.rs`):**
- `#[ignore]`'d so it doesn't run in default CI (sweeps are slow + noisy).
- Drives the 24-cell sweep, prints the recall table.
- Asserts that the chosen-default cell achieves 100% loose recall and ≥ chosen-strict recall — making the test self-validating against the very thing we tuned for. If the sweep test passes, the chosen defaults are consistent with the data.

**New test (`retrieval_sweep_hybrid.rs`):**
- `#[cfg(feature = "fastembed")]` + `#[ignore]`.
- Builds, runs end-to-end against BGE-M3, prints a hybrid recall table.
- No assertion on chosen-default cell (we're not landing hybrid defaults this session). Just structural correctness — the harness drives the swap, the table prints.

**No mocking required.** The sweep runs against a real `SqliteKnowledgeBase` populated by `auto_seed_if_empty` against the in-repo seed corpus. This is the ground truth.

## Verification

After implementation, the user runs (from `src/`):

```bash
~/.cargo/bin/cargo test --workspace                                      # 550+N green
~/.cargo/bin/cargo test -p primer-kb-load -- --ignored sweep --nocapture # diagnostic table prints
~/.cargo/bin/cargo clippy --workspace --all-targets                      # clean
~/.cargo/bin/cargo fmt --all -- --check                                  # clean
```

Optional follow-up (when network + time allow):

```bash
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed -- --ignored sweep_hybrid --nocapture
```

## Risks & open questions

- **Overfitting to a 90-passage corpus.** The benchmark queries also drive the tuning, so the chosen `top_k` and `min_score` are optimal *for the test*. Mitigation: query expansion targets paraphrase + cross-cluster + child-language coverage to reduce structural overlap with the seed text. Production behaviour on real children's questions may still diverge; that's what the next phase of testing (real children, real sessions) addresses.
- **Strict subset hand-labeling cost.** ~15-20 mappings is fine to do in one pass but rots if seed passage ids ever rename. Mitigation: passage ids are content-addressed and stable; if we ever bulk-rename, the strict subset gets fixed in the same PR.
- **`min_score=1.5` may be too aggressive.** BM25 raw scores depend on document frequency and term rarity; a fixed floor risks over-filtering on smaller corpora. Mitigation: the sweep tests this empirically — if every query at `min_score=1.5` has zero hits, the cell scores 0% and is rejected by the selection rule.
- **Hybrid path stays at unverified defaults.** The current `KB_BM25_TOP_K=20`, `KB_VECTOR_TOP_K=20`, `RRF_K=60.0` come from the spec author's reading of the IR literature, not measurement. Mitigation: the scaffold makes the follow-up cheap; until then, the BM25-only path is the default runtime path (`--embedder-backend none`) so production retrieval is fully covered by this session's tuning.

## Out of scope (deferred to future sessions)

- Hybrid `RRF_K` tuning (needs the gated harness + a session for review).
- LTM tuning (needs an LTM-specific benchmark).
- Corpus expansion if strict-recall < 100%.
- Per-locale tuning (German, Hindi) — locale-specific corpora don't exist yet.
- Re-tokenisation for BM25 (e.g. stop-word removal, stemming) — different design.
- Live A/B harness — different design.

## Acceptance criteria

A PR closes this work when:

1. ~85 benchmark queries with cluster labels and ~15-20 strict-subset canonical-id mappings live in `tests/common/`.
2. `retrieval_sweep.rs` prints a 24-cell table when run with `--ignored --nocapture`.
3. `retrieval_sweep_hybrid.rs` exists, compiles under `--features fastembed`, and runs to completion against BGE-M3 (verified locally; not gated in CI).
4. `KB_FINAL_TOP_K` and `KB_BM25_ONLY_MIN_SCORE` in `consts.rs` are set to the winning cell's values.
5. `RetrievalParams::default()` mirrors the consts; a unit test asserts the alignment.
6. The CI-default `retrieval_quality.rs` test passes against the tuned defaults.
7. If strict recall < 100%: a GitHub issue is filed listing the unsatisfied queries.
8. `simple_wikipedia.py` User-Agent carries `my.list.subscriptions@gmail.com` (issue #37 closed).
9. README, ROADMAP, and CLAUDE.md reflect Phase 0.2 closure if the chosen defaults are landed.
10. `cargo test --workspace`, `cargo clippy --workspace --all-targets`, and `cargo fmt --all -- --check` all pass.
