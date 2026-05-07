# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-07 (after PR for retrieval-parameter tuning opened — Phase 0.2 closeout).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **561 Rust tests** across the workspace plus **43 Python tests** in `data/ingest/` (pytest from a venv set up per `data/ingest/README.md`).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes.

## Branch status

`main` (post-merge of the retrieval-tuning PR). The feature branch `feature/retrieval-tuning` carries 13 commits that landed Phase 0.2 closure. The `feature/retrieval-tuning` branch can be deleted from the remote at the user's discretion after merge.

## What we shipped this session

**Phase 0.2 closeout — retrieval-parameter tuning.** Tunes the BM25-only KB retrieval defaults from a 24-cell sweep over the now-90-passage seed corpus, scaffolds a hybrid sweep behind `--features fastembed` for follow-up tuning, folds in issue #37 (Wikipedia ingest User-Agent contact), and files issue #42 for the one strict-recall miss.

**The sequence of 13 commits:**

- `66a14df` — `test(retrieval): migrate QUERIES to BenchQuery struct in shared common module`
- `19b8aa0` — `test(retrieval): expand benchmark to 87 queries with paraphrases and cross-cluster coverage`
- `36c829b` — `test(retrieval): annotate 20 strict-subset queries with canonical_id`
- `cffce51` — `test(retrieval): add #[ignore]'d sweep diagnostic over (top_k × min_score) grid`
- `1d59e59` — `refactor(retrieval): tune KB_FINAL_TOP_K to 5 from 24-cell sweep`
- `325c05c` — `refactor(retrieval): align RetrievalParams::default() with consts`
- `e21bb62` — `test(retrieval): pin regression test to tuned production defaults`
- `c7a932f` — `docs(retrieval): pin corpus-gap issue #42 in KNOWN_FAILING_QUERIES`
- `3fb0294` — `test(retrieval): scaffold hybrid sweep behind --features fastembed`
- `656273a` — `fix(ingest): replace placeholder User-Agent contact (closes #37)`
- `2f51c54` — `style(retrieval): apply rustfmt to test files`
- `ae20579` — `docs: Phase 0.2 closeout (retrieval-parameter tuning landed)`
- `f3752c0` — `test(retrieval): sanity-check that KNOWN_FAILING_QUERIES entries exist in QUERIES`

**Concrete deliverables:**

- 87-query benchmark in `src/crates/primer-kb-load/tests/common/mod.rs` (was 64; expanded with 7 paraphrases + 9 cross-cluster + 7 wiki). 20 entries carry `canonical_id` for the strict-recall metric. Five sanity tests guard size/cluster/strict-subset/canonical-id-existence/known-failing-consistency.
- 24-cell `(top_k × min_score)` BM25 sweep at `src/crates/primer-kb-load/tests/retrieval_sweep.rs` (`#[ignore]`'d, prints recall table + auto-picks winner per lex rule).
- 54-cell hybrid sweep scaffold at `src/crates/primer-kb-load/tests/retrieval_sweep_hybrid.rs` (`#![cfg(feature = "fastembed")] #[ignore]`'d). Compiles cleanly under `--features fastembed`; not run end-to-end this session (would need ~570 MB BGE-M3 download + 5–15 min runtime).
- Tuned production defaults: **`KB_FINAL_TOP_K: 3 → 5`** (the clear knee from the sweep); `KB_BM25_ONLY_MIN_SCORE: 0.5` unchanged but doc updated to record what the sweep showed (every value in `{0.0..1.5}` produces identical recall on the current corpus; kept at 0.5 as defensive floor).
- `RetrievalParams::default()` now reads from `consts::retrieval` so the struct's Default impl can never drift away from the consts again. New unit test asserts the alignment.
- Regression test (`retrieval_quality.rs`) pinned to production defaults with both loose-substring AND new strict-canonical-id assertions. `KNOWN_FAILING_QUERIES` excludes the one query (`"how does the sun shine"`) where the canonical `seed:en:sun` doesn't make top-5.
- Issue #37 closed (User-Agent contact `my.list.subscriptions@gmail.com` lands in both `data/ingest/simple_wikipedia.py` and `data/ingest/build_whitelist.py`).
- Issue #42 filed for the one strict-recall miss; tracked from `KNOWN_FAILING_QUERIES` doc comment.
- `cargo fmt --all -- --check` clean (the rustfmt commit `2f51c54` collapsed long single-line struct literals to multi-line form per the 100-char default).

**Documentation:** `README.md`, `ROADMAP.md`, `CLAUDE.md` updated to reflect Phase 0.2 closure (no longer "(MVP)"). Spec at [docs/superpowers/specs/2026-05-06-retrieval-tuning-design.md](docs/superpowers/specs/2026-05-06-retrieval-tuning-design.md). Plan at [docs/superpowers/plans/2026-05-06-retrieval-tuning.md](docs/superpowers/plans/2026-05-06-retrieval-tuning.md).

**Test count after merge:** 561 Rust (550 baseline + 11 new sanity/alignment tests) + 43 Python (unchanged).

## What's next

### Phase 0.3 follow-ups already filed

- **#39** — hybrid-leg version of the canonical-query test. The current regression test (`retrieval_quality.rs`) is BM25-only; a hybrid analogue using `StubEmbedder` (no model download in CI) plus an opt-in real-`fastembed` variant would surface vector-leg regressions. **Not addressed by this PR's hybrid scaffold** — `retrieval_sweep_hybrid.rs` is a sweep harness (no assertions), not a regression test.
- **#42** — corpus gap surfaced by this session: BM25 retrieval cannot place `seed:en:sun` in top-5 for the query `"how does the sun shine"`. The fix is corpus expansion (rewrite the seed:en:sun passage so it lexically dominates the question more, OR wait for hybrid retrieval to be tuned — semantic embedding bridges paraphrase gaps).

### The next big piece

**Tune hybrid retrieval defaults.** The hybrid sweep scaffold is now in place; running it requires ~570 MB BGE-M3 download + 5–15 min:

```bash
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed \
    --test retrieval_sweep_hybrid \
    -- --ignored sweep_retrieval_params_hybrid --nocapture 2>&1 | tee /tmp/sweep_hybrid.txt
```

Acceptance criteria for landing hybrid defaults:
1. Sweep table shows a clear winner across `(bm25_top_k × vector_top_k × final_top_k × rrf_k)`.
2. Strict recall ≥ 95% (matching BM25 baseline) and ideally lifts the one strict miss tracked in #42.
3. Update `KB_BM25_TOP_K`, `KB_VECTOR_TOP_K`, `RRF_K` in `consts.rs` to the chosen cell.
4. Optional: address #39 with a hybrid regression test.

### Larger queue items (carried forward)

- Local llama.cpp inference (Phase 1.1).
- Voice-loop hardening (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality, not production).
- Hardware integration (Phase 3 — display, audio, enclosure).
- Hindi (`hi`) and German (`de`) locale pack rollout — the locale-keyed schema is in place; only the templates and per-locale wiki layers need to be added.

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

**Carried forward from the prior session that this session did not address:**
- #38 — add 429/5xx backoff to `fetch_leads` (Python ingestion). Not blocking; only one-time runs need it today.
- #40 — aggregate per-source attribution row for Wikipedia. Not urgent.
- #41 — tighten disambiguation regex if false positives appear.

**New risks introduced by this session:**

- **`min_score=0.5` is a no-op against the current 90-passage corpus** (every BM25 score sits comfortably above 1.5). A future corpus expansion that dilutes term frequencies and pushes scores down could silently start filtering hits. The doc comment in `consts.rs` flags this; if you observe retrieval-quality regressions after a corpus expansion, check whether `KB_BM25_ONLY_MIN_SCORE` is now biting.
- **4 paraphrase queries were dropped during Task 2** (Plan-permitted, but real signal lost): "what makes my tummy growl when I am hungry", "what is inside a tiny bug", "why do flowers smell nice", "why does the brain need oxygen from the lungs". The corpus covers them, but BM25 can't surface them at top_k=5. They should re-appear in the benchmark once hybrid retrieval is tuned (semantic embedding bridges paraphrase gaps) or once the corpus is reworked.

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
- **Subagent-driven development workflow** for plan execution. Each task: dispatch implementer subagent → spec compliance review → code quality review → mark complete. The discipline catches lots of small drifts that would compound over a 13-task PR.

**New pattern from this session:**

- **Defensive sanity tests at the data layer.** When test data has multiple consumers (e.g. `QUERIES` consumed by `retrieval_quality.rs` + `retrieval_sweep.rs`), put cheap sanity tests in the shared `tests/common/mod.rs` that validate invariants (size floor, per-cluster floor, strict-subset floor, canonical-id-exists-in-corpus, known-failing-list-no-typos). They run inside every test binary that imports the module — duplication is fine; the cost is microseconds and the safety net catches typos that silently disable assertions.

## Exact commands needed to resume

```bash
# Resume on main:
cd /Users/hherb/src/primer
git status                       # confirm clean
git checkout main
git pull
git log --oneline -15            # recent activity (the retrieval-tuning PR's merge commit should be at the top)

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 561 passed, 0 failed (as of 2026-05-07).

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

For inspecting the seed corpus + running the retrieval-quality test:
```bash
~/.cargo/bin/cargo test -p primer-kb-load
# Should pass; the regression test now pins production defaults.
```

For running the BM25 sweep diagnostic:
```bash
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_sweep \
    -- --ignored sweep_retrieval_params --nocapture 2>&1 | tee /tmp/sweep.txt
# Prints 24-cell table; identifies winning cell algorithmically.
```

For running the hybrid sweep (the next session's headline task):
```bash
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed \
    --test retrieval_sweep_hybrid \
    -- --ignored sweep_retrieval_params_hybrid --nocapture 2>&1 | tee /tmp/sweep_hybrid.txt
# First run downloads BGE-M3 (~570 MB); 5–15 min runtime; 54-cell table.
```

For re-running the Wikipedia ingest (rare; only when the whitelist changes):
```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/python simple_wikipedia.py
# Writes ../seed/wiki_passages.en.jsonl. Commit any diff that reflects
# real upstream article-content changes.
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If you discover that Horst did interim work that changes the plan, flag it.
