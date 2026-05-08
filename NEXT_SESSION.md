# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-08 (after hybrid retrieval defaults landed — Phase 0.2.5 closeout).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **591 Rust tests** under default features, **597 under `--features primer-kb-load/fastembed`**, plus **43 Python tests** in `data/ingest/` (pytest from a venv set up per `data/ingest/README.md`).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes.

## Branch status

`feature/hybrid-retrieval-tuning` carries 3 commits ready to land via PR. Once merged, the branch can be deleted from the remote at the user's discretion. The branch is built off the merge commit `c26ddc1` ("Merge remote-tracking branch 'origin/main'") that reconciled local `main`'s pre-PR-43 spec/plan duplicates with the squash-merged PR #43 + PR #47 on `origin/main`.

## What we shipped this session

**Phase 0.2.5 closeout — hybrid retrieval defaults tuned.** The 54-cell hybrid sweep at `tests/retrieval_sweep_hybrid.rs` ran end-to-end against BGE-M3 on the 87-query benchmark; production `HybridParams::default()` was bumped to the chosen cell `(bm25_top_k=30, vector_top_k=30, final_top_k=5, rrf_k=60)`, achieving **100% loose / 100% strict recall** and lifting the BM25-only strict miss for "how does the sun shine" via semantic embedding. Closes #42 and #39.

**The sequence of 3 commits:**

- `bd7ad6f` — `refactor(retrieval): tune HybridParams defaults to (30, 30, 5, 60)`
- `ad80012` — `test(retrieval): add hybrid retrieval-quality regression test`
- `9206166` — `docs: Phase 0.2.5 closeout (hybrid retrieval defaults landed)`

**Concrete deliverables:**

- **Sweep run:** 54-cell sweep completed; 17 cells tied at 100/100. The narrow failure cluster is `bm25_top_k=10 + vector_top_k≥20` — too narrow a BM25 leg lets the dense leg's noise push canonical hits out. Every cell with `bm25_top_k ≥ 20 && final_top_k = 5` achieves 100/100 across all `rrf_k ∈ {30, 60, 90}`. Output saved at `/tmp/sweep_hybrid.txt` (gitignored).
- **Tuned consts:** `KB_BM25_TOP_K: 20 → 30`, `KB_VECTOR_TOP_K: 20 → 30` in `primer-core/src/consts.rs`. `KB_FINAL_TOP_K = 5` and `RRF_K = 60.0` unchanged. Doc comments record the sweep result and chosen-cell rationale.
- **`HybridParams::default()` aligned with consts:** previously held inline magic numbers (20, 20, 5, 60.0, 0.0). Now reads from `consts::retrieval`. New `hybrid_default_mirrors_consts` test pins the alignment, mirroring the existing `default_mirrors_consts` for `RetrievalParams`.
- **Hybrid regression test (#39):** new file `primer-kb-load/tests/retrieval_quality_hybrid.rs`. Two flavours sharing `tests/common/mod.rs` with the BM25 path:
  - `hybrid_retrieval_structural_with_stub` — always built; `StubEmbedder`; asserts non-empty hits, ≤ `final_top_k`, idempotent. NO recall floor (stub vectors are orthogonal for synonyms — a recall assertion would measure noise).
  - `hybrid_retrieval_recall_with_fastembed` — `--features fastembed` only; real BGE-M3; pins 100% loose / 100% strict at `HybridParams::default()`. Drops break CI under that feature.
- **`KNOWN_FAILING_QUERIES_HYBRID`** added to `tests/common/mod.rs` (empty today). Sanity test extended to validate both BM25 and hybrid known-failing lists reference real benchmark queries.
- **Issue housekeeping:** #42 closed with explanatory comment (BM25-only fallback path still has the gap; hybrid is the production path); #39 closed with summary of the regression-test design.
- **Docs:** README.md, ROADMAP.md, CLAUDE.md updated to reflect Phase 0.2.5 closure (no longer "tuned BM25 retrieval defaults" — now "tuned BM25-only AND hybrid retrieval defaults").
- `cargo fmt --all -- --check` clean. `cargo clippy --workspace --all-targets` clean (default + `--features primer-kb-load/fastembed`).

**Test count after merge:**
- Default features: **591 Rust** (was 584 baseline + 6 new in `retrieval_quality_hybrid` binary's `mod common` re-import + 1 new `hybrid_default_mirrors_consts` in `primer-core`).
- With `--features primer-kb-load/fastembed`: **597 Rust** (default 591 + 5 sanity-tests-via-`mod-common` re-import in `retrieval_sweep_hybrid` binary + 1 new `hybrid_retrieval_recall_with_fastembed`).
- 43 Python tests in `data/ingest/` (unchanged).

## What's next

### Larger queue items (carried forward)

- **Local llama.cpp inference (Phase 1.1).** `LlamaCppBackend` stub at `primer-inference` is the entry point.
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality, not production).
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **Hindi (`hi`) and German (`de`) locale pack rollout** — the locale-keyed schema is in place; only the templates and per-locale wiki layers need to be added. Per-locale wiki layers would auto-load via `auto_seed_if_empty`'s multi-file discovery.
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default. The relevant flips: `default = ["embedding"]` in `primer-cli/Cargo.toml` and the `--embedder-backend` CLI default.

### Phase 0.2 follow-ups still open

- **#38** — add 429/5xx backoff to `fetch_leads` (Python ingestion). Not blocking; only one-time runs need it today.
- **#40** — aggregate per-source attribution row for Wikipedia. Not urgent.
- **#41** — tighten disambiguation regex if false positives appear.

## Open decisions / risks

Carried-forward open items (still relevant from prior sessions):

- Smoke #6 (Ollama daemon down) deferred verification.
- `is_request()` heuristic conflates config errors with network errors.
- HTTP-date form `Retry-After` silently dropped.
- Pre-flight auth check at startup deferred.
- Voice-mode bridging hook is data-only.
- `close_session` may drop the final classifier task's `turn_classifications` row on a short conversation.
- Identifier-format divergence across structured-output crates (`llm:{model}` vs `llm:{backend}:{model}`).
- `apply_comprehension` insertion policy (only updates concepts already in `learner.concepts`).
- Concepts are monotonic on `learner_concepts` — saved-once stays saved.
- No cancellation token on streaming-task spawns.
- Background-task spawn order is load-bearing on serialized backends.
- "I just don't want to see this word" escape valve for vocab not yet implemented.

**Carried forward from the prior session:**

- 4 paraphrase queries dropped during Phase 0.2 closeout (real signal lost): "what makes my tummy growl when I am hungry", "what is inside a tiny bug", "why do flowers smell nice", "why does the brain need oxygen from the lungs". Worth checking whether the new hybrid path covers them — they were dropped because BM25-only couldn't surface them. The current 87-query benchmark may have room to grow back to ~91 queries if hybrid handles them.
- `min_score=0.5` in BM25-only path is a no-op against the current 90-passage corpus (every BM25 score sits comfortably above 1.5). Only relevant if a future corpus expansion dilutes term frequencies.

**New risks introduced by this session:**

- **`HybridParams::default()` now indirects through `consts::retrieval`.** A future change to a const that's not covered by the drift test (e.g. adding a new field to `HybridParams` and forgetting to extend `default_mirrors_consts`) would silently regress the alignment. The existing test now covers six fields — extend it whenever `HybridParams` grows.
- **The hybrid recall test downloads ~570 MB on first run** (`~/.cache/primer/models/`). Subsequent runs are warm. CI can't run `--features primer-kb-load/fastembed` without paying that download once; until CI validates the cdn.pyke.io path, the recall test only runs locally on demand. The structural-stub flavour DOES run in default CI and catches transport-layer regressions.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing.)

- **Pure functions in `primer-core`** for algorithmic cores — tested by injection of `now`, no async, no I/O.
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level.
- **Carrier structs with `disabled()` no-op constructors** for parameter bundles that not all callers need to configure.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed.
- **Constants in `consts.rs` submodules.** No magic numbers anywhere.
- **TDD discipline.** Tests first; watch them fail; implement to green.
- **File-size hygiene.** Keep modules under 500 lines.
- **Network-injection test seam** for any data-ingest pipeline (`http_client` parameter, substitute `FakeHttpClient` in tests).
- **Subagent-driven development workflow** for plan execution.
- **Defensive sanity tests at the data layer.** When test data has multiple consumers, put cheap sanity tests in the shared `tests/common/mod.rs` that validate invariants. They run inside every test binary that imports the module — duplication is fine; the cost is microseconds and the safety net catches typos.

**New pattern from this session (re-applied, not invented):**

- **Always pin `Default` impls of public structs to `consts::*`, with a drift-prevention test.** The session bumped `HybridParams::default()` from inline magic numbers to consts-backed values, mirroring the same fix that landed for `RetrievalParams::default()` in PR #43. Any new `Default for X` where `X` is a public tuning knob should follow the same pattern: read from `consts`, add `mod params_tests { default_mirrors_consts }`. Drift between code and consts is silent failure mode.

## Exact commands needed to resume

```bash
# Resume on main (after this PR merges):
cd /Users/hherb/src/primer
git status                       # confirm clean
git checkout main
git pull
git log --oneline -10            # recent activity (the hybrid-retrieval-tuning PR's merge commit should be near the top)

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 591 passed, 0 failed (as of 2026-05-08).

~/.cargo/bin/cargo clippy --workspace --all-targets
~/.cargo/bin/cargo fmt --all -- --check
```

For the Python ingestion pipeline tests:

```bash
cd /Users/hherb/src/primer/data/ingest
python3 -m venv .venv
.venv/bin/pip install -r requirements.txt pytest
.venv/bin/pytest tests/
# Expected: 43 passed.
```

For real-LLM smoke testing (Anthropic):

```bash
cd /Users/hherb/src/primer/src
RUST_LOG=debug ~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name SmokeTester --age 9 --no-persist --verbose 2>&1 | tee /tmp/smoke.log
```

For the speech build path: `~/.cargo/bin/cargo build --workspace --features primer-cli/speech`.

For the embedding feature build path:

```bash
~/.cargo/bin/cargo build --workspace --features primer-cli/embedding
~/.cargo/bin/cargo run --bin primer -- --embedder-backend fastembed ...
# First run downloads BGE-M3 (~570 MB) into ~/.cache/primer/models/.
```

For running the BM25 sweep diagnostic:

```bash
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_sweep \
    -- --ignored sweep_retrieval_params --nocapture 2>&1 | tee /tmp/sweep.txt
# Prints 24-cell table; identifies winning cell algorithmically.
```

For running the hybrid sweep:

```bash
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed \
    --test retrieval_sweep_hybrid \
    -- --ignored sweep_retrieval_params_hybrid --nocapture 2>&1 | tee /tmp/sweep_hybrid.txt
# 54-cell table; ~5–15 min runtime once the model is cached (570 MB on first run).
```

For running the hybrid regression test:

```bash
# Structural (always built):
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_hybrid

# Real-recall (--features fastembed, real BGE-M3):
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed \
    --test retrieval_quality_hybrid -- --nocapture
# ~30s once model is cached; first run downloads BGE-M3.
```

For re-running the Wikipedia ingest (rare; only when the whitelist changes):

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/python simple_wikipedia.py
# Writes ../seed/wiki_passages.en.jsonl. Commit any diff.
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If you discover that Horst did interim work that changes the plan, flag it.
