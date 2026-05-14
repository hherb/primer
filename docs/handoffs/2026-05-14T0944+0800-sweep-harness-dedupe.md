# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-14T0944+0800 (after opening PR #97 — retrieval-sweep harness dedupe; commit `1948de2` on `refactor/sweep-harness-dedupe-66`; closes #66 and #67).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **739 Rust tests** under default features. 3 ignored. Add `--features primer-kb-load/fastembed` for the embedding-backed sweeps + the real-BGE-M3 recall tests (downloads BGE-M3 ~570 MB on first run; cached afterwards). Plus **135 Python tests** in `data/ingest/` (unchanged this session — Rust-only work).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.

## Branch status

`refactor/sweep-harness-dedupe-66` carries 1 commit, pushed, and on **PR #97** (https://github.com/hherb/primer/pull/97). Built off `origin/main` at `acbc7c3` (PR #94 squash — cargo fmt + clippy fixes to unblock CI). Once #97 merges, the branch can be deleted.

## What we shipped this session

**Refactor: dedupe the four retrieval-sweep harnesses into one shared helper.** Pre-refactor, the codebase carried two pairs of near-identical sweep harnesses — `retrieval_sweep{,_de}.rs` for BM25 and `retrieval_sweep_hybrid{,_de}.rs` for hybrid. Each pair was ~95% bit-identical Rust (~1000 lines across 4 files). Adding a third locale (Hindi `hi`) would have expanded to **six** harness files and tripled the duplication.

**Commits on `refactor/sweep-harness-dedupe-66`:**

- `1948de2` — `refactor(tests): dedupe retrieval-sweep harnesses into tests/common/sweep.rs`

**Concrete deliverables:**

- **New shared helper at [src/crates/primer-kb-load/tests/common/sweep.rs](src/crates/primer-kb-load/tests/common/sweep.rs)** (557 lines): exposes `Bm25SweepConfig`, `run_bm25_sweep`, `HybridSweepConfig`, `run_hybrid_sweep`. The helper is one source of truth for: the grid constants (`BM25_TOP_K_GRID`, `BM25_MIN_SCORE_GRID`, the four `HYBRID_*_GRID`s, and `HYBRID_PREFERRED_RRF_K`), the lex-selection rule (uniformly `.partial_cmp(...).unwrap_or(Ordering::Equal)` — closes #67), the print format, and the self-validation step.
- **Four per-locale shims**: [retrieval_sweep.rs](src/crates/primer-kb-load/tests/retrieval_sweep.rs) (59 lines), [retrieval_sweep_de.rs](src/crates/primer-kb-load/tests/retrieval_sweep_de.rs) (56 lines), [retrieval_sweep_hybrid.rs](src/crates/primer-kb-load/tests/retrieval_sweep_hybrid.rs) (44 lines), [retrieval_sweep_hybrid_de.rs](src/crates/primer-kb-load/tests/retrieval_sweep_hybrid_de.rs) (47 lines). Each shim supplies a `*SweepConfig` with locale + queries + cluster list + table labels.
- **CLAUDE.md updated** with a paragraph describing the new harness shape (search "Sweep harness implementation is shared across locales").

**Verification:**

- `~/.cargo/bin/cargo test --workspace --no-fail-fast` → 739 passed / 0 failed / 3 ignored (baseline unchanged)
- `~/.cargo/bin/cargo fmt --all -- --check` clean
- `RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets` clean
- `~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_sweep -- --ignored sweep_retrieval_params --nocapture` → byte-identical sweep table to pre-refactor (stripping compile/run timing lines)
- `~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_sweep_de -- --ignored sweep_retrieval_params_de --nocapture` → byte-identical
- `~/.cargo/bin/cargo build --tests -p primer-kb-load --features fastembed` clean

**Net diff:** 7 files changed, 648 insertions(+), 931 deletions(-).

**Design choices that may be relevant later:**

- **Per-cluster counts in `Bm25CellMetrics` are now `Vec<(Cluster, usize, usize)>` instead of a fixed-size array.** EN's 6-cluster and DE's 5-cluster harnesses now share one cell type — adding Hindi (whatever cluster count it needs) is data, not code. Cost is one heap allocation per cell per grid iteration, dwarfed by the SQLite/BM25 work.
- **`dash_width` is an explicit `Bm25SweepConfig` field.** EN passes 96, DE passes 90. The widths are visual choices the pre-refactor harnesses had hardcoded; preserving them as config knobs guarantees byte-identical output. They're easy to derive (`60 + N*6` for `N` clusters) but the explicit form is more obviously correct.
- **Hybrid sweep helpers live inside `mod hybrid { ... }` inside `sweep.rs`, gated on `#[cfg(feature = "fastembed")]`.** The hybrid helpers are re-exported via `pub use hybrid::{HybridSweepConfig, run_hybrid_sweep};` carrying `#[allow(unused_imports)]` because each test binary in `primer-kb-load` includes the full `common` tree via `mod common;`, and only the hybrid shim binaries actually use these items. Without the suppression, every non-hybrid test binary fired a warning.
- **The helper is 557 lines — 11% over the project's 500-line guideline.** Logically coherent today (BM25 section + hybrid section in one place). If Hindi adds enough config knobs to push it noticeably further, split into `sweep/bm25.rs` + `sweep/hybrid.rs`.

## What's next

### The most defensible smaller-scope follow-ups

- **Issue #90 — Voice asset path re-resolution server-side (security hardening).** `download_voice_assets` currently round-trips a full `MissingAsset` payload (path + suggested_url) through the webview; if the webview is ever compromised, a hostile payload could direct writes outside `~/.cache/primer/models/`. Concrete fix: have the frontend echo only `kind` strings, re-resolve `path`+`suggested_url` server-side via the existing `assets::resolve_voice_assets`. Surface from the PR #89 review.
- **Hindi (`hi`) locale pack rollout.** Now that #66 is closed, adding Hindi is a data-only change to `tests/common/`. The required adds are: `tests/common/hi.rs` (define `QUERIES_HI` mirroring the EN/DE shape), parallel `retrieval_quality_hi.rs` (mirror `retrieval_quality_de.rs`), parallel `retrieval_sweep_hi.rs` + `retrieval_sweep_hybrid_hi.rs` (~50-line shims each via the new `run_bm25_sweep` / `run_hybrid_sweep`), a `WikiSource` preset in `data/ingest/wiki/source.py`, and a children's-vocabulary corpus source. Wikipedia's "बाल विकिपीडिया" (Bal Vikipedia) is the obvious analogue of Klexikon and Simple English — confirm it's actually live and CC-licensed before commitment. Schema + i18n boundary are already locale-keyed; no Rust core changes expected.

### Larger queue items (carried forward)

- **Local llama.cpp inference (Phase 1.1).** `LlamaCppBackend` stub at `primer-inference` is the entry point.
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality, not production). The voice mode (Phase A) GUI work landed in PR #89 — production polish is the still-open piece. Issues #90/91/92 are the immediate review-fallout follow-ups.
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default. The relevant flips: `default = ["embedding"]` in `primer-cli/Cargo.toml` and the `--embedder-backend` CLI default.

### Klexikon corpus follow-ups (carried forward)

- **Klexikon corpus expansion past 66.** Klexikon has ~3000 articles. The 2 corpus gaps from the PR #93 session (gänsehaut reflex; tides on the mond article) would need either expanded articles or additional Klexikon titles to lift. Concrete next-batch acceptance: pick 30-50 more titles in still-thin clusters — body could use Atmung-related topics; life could use Ökosystem, Affe/Pferd/Fledermaus; how-things-work could use Computer/Internet/Telefon/Maschine. Re-run pipeline. No code change required. Once corpus grows past ~150 passages, re-run the sweep harnesses to verify production defaults still flatline at 100% strict from `top_k=5` onward on non-paraphrase queries (and that the BM25 floor's defensive 0.5 hasn't been challenged by the larger statistic).
- **Klexikon license claim spot-check.** Concrete acceptance: WebFetch a sample of ~5 article footers (e.g. `Sonne`, `Atome und Moleküle`, `Skelett`, `Bienen`, `Mondfinsternis`) and verify each footer shows CC-BY-SA-4.0. If a per-page divergence appears, document a per-passage license override field in `WikiSource`. Low-priority.

### Smaller-scope follow-ups still open

- **#86** — primer-gui: avoid double session-DB open on resume (enhancement).
- **#87** — primer-gui: end-to-end resume_session test for cross-locale inheritance (enhancement).
- **#80** — GUI: expose Locale::ALL via a Tauri command instead of hand-mirroring it in settings.js (enhancement).
- **#81** — GUI: settings modal needs a focus trap (enhancement).
- **#71** — GUI: tighten CSP before ship (remove `'unsafe-inline'`).
- **#91** — voice: move `VoiceStateCopy` strings into per-locale TOML packs.
- **#92** — voice: `download_voice_assets` needs timeout, resume, and max-size cap.
- **#69** — primer-engine: embedder helpers should return Result, not `std::process::exit`.
- **#46** — explore post-RRF `min_score` as a fifth grid axis in the hybrid sweep. (Now lives in the shared helper; the change is a one-axis addition to `HybridSweepConfig` + the loop body.)
- **#40** — aggregate per-source attribution row for Wikipedia (and Klexikon).
- **#41** — tighten disambiguation regex if false positives appear.
- **#22** — cache prepared statements for the corpus-bootstrap path (Phase 0.2; enhancement).
- **#21** — separate `--languages` preference list from bound `--language` locale.
- **#20** — i18n placeholder validator can false-fail on translator narrative text.
- **Failed-batch persistence sidecar (issue #38 optional).** Was deferred during brainstorming; file as new issue if scheduled ingest ever ships.
- **Network-error retry.** `requests.exceptions.ConnectionError` / `Timeout` still propagate unchanged.
- **Pre-commit fmt hook (workflow-level).** Carried forward from PR #94. Drift accumulated 13 files across 5 PRs before CI noticed; a local `cargo fmt --check` pre-commit hook would prevent it. Defer until the next time drift happens.

## Open decisions / risks

Carried-forward open items (still relevant from prior sessions):

- Smoke #6 (Ollama daemon down) deferred verification.
- `is_request()` heuristic conflates config errors with network errors.
- HTTP-date form `Retry-After` silently dropped (true on both Rust and Python sides).
- Pre-flight auth check at startup deferred.
- Voice-mode bridging hook is data-only.
- `close_session` may drop the final classifier task's `turn_classifications` row on a short conversation.
- Identifier-format divergence across structured-output crates (`llm:{model}` vs `llm:{backend}:{model}`).
- `apply_comprehension` insertion policy (only updates concepts already in `learner.concepts`).
- Concepts are monotonic on `learner_concepts` — saved-once stays saved.
- No cancellation token on streaming-task spawns.
- Background-task spawn order is load-bearing on serialized backends.
- "I just don't want to see this word" escape valve for vocab not yet implemented.

**New observations from this session:**

- **Hybrid sweep output was not re-verified live.** The byte-identical guarantee for the BM25 sweeps was confirmed by re-running and diffing against pre-refactor baselines. The hybrid sweeps are structurally identical (same grid iteration, same lex-selection rule, same print format) but BGE-M3 was not cached locally, and the ~570 MB download wasn't justified for a refactor that produces byte-identical output by construction. A follow-up session can re-run the hybrid sweeps end-to-end if it has BGE-M3 cached — the printed table should be bit-identical to the last hybrid-sweep run on the same corpus.
- **The 557-line helper is slightly over the project's 500-line guideline.** Logically coherent as one file today; splitting into `sweep/bm25.rs` + `sweep/hybrid.rs` is the obvious path if Hindi or a fifth axis (#46) pushes it materially further.
- **`#[allow(unused_imports)]` on the `pub use hybrid::{...}` re-export is load-bearing.** Each test binary in `primer-kb-load` includes the full `common` tree via `mod common;`, and only the hybrid shim binaries actually consume these items. Without the suppression, every non-hybrid test binary fired a "unused imports" warning, which `RUSTFLAGS="-D warnings"` would have promoted to a CI error.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing, with one new pattern added this session.)

- **NEW: Shared test harness with `*Config` carrier struct + locale-specific shim.** Pattern shape: one helper module (`tests/common/sweep.rs`), one config carrier struct per algorithmic shape (`Bm25SweepConfig`, `HybridSweepConfig`), one entrypoint per shape (`run_bm25_sweep`, `run_hybrid_sweep`), thin per-locale shims that fill the config. Output is the public contract — verify with byte-identical diff against pre-refactor baselines, not arbitrary assertions. The same pattern can be applied to the `retrieval_quality{,_de}{,_hybrid,_hybrid_de}` regression tests if they grow a third locale.
- **Pure functions in dedicated modules** for algorithmic cores — tested directly without I/O.
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level.
- **Carrier structs with `disabled()` no-op constructors** for parameter bundles that not all callers need to configure.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed.
- **Constants in `consts.rs` submodules** (Rust) or top-of-module `_DEFAULT_*` constants (Python). No magic numbers anywhere.
- **TDD discipline.** Tests first; watch them fail; implement to green. **Adapted this session** — for a "produce byte-identical output" refactor, the test is the diff against pre-refactor baselines, captured before any code change.
- **File-size hygiene.** Keep modules under 500 lines.
- **Network-injection test seam** for any data-ingest pipeline (`http_client` parameter, substitute `FakeHttpClient`/`KlexikonFakeHttpClient` in tests).
- **Subagent-driven development workflow** for plan execution (or inline executing-plans for small TDD-shaped plans).
- **Defensive sanity tests at the data layer.**
- **Always pin `Default` impls of public structs to `consts::*`, with a drift-prevention test.** Mirrored on the Python side as `RetrySettings.default()` ↔ module-level `DEFAULT_*` consts.
- **Pure helpers + their own unit tests, even in `#[ignore]`'d integration tests.**
- **Frozen dataclasses for process-wide configuration** (the `WikiSource` pattern; `RetrySettings` follows the same shape).
- **String discriminators for strategy selection, with allow-list validation.**
- **Re-run the live ingest after changing any fetch-path helper.** Tiny expected diff = success, anything bigger = a flag worth investigating.
- **Kwarg-injected side-effect functions for TDD seams in Python.**
- **Structural ingest-time defences beat manual probing habits.**
- **Back-compat re-export shims when bulk-editing test imports would dilute a structural-refactor PR.**
- **Plan-then-execute inline for mechanical refactors with a strong test safety net.**
- **Two-commit refactor: "set up the change" then "remove the old".** (This refactor was small enough for one commit; the two-commit form is for larger PRs.)
- **Ship the follow-up issue body with explicit acceptance + per-file checklist when a PR ships a transitional state.**
- **Per-locale dataset modules under `tests/common/<pack>.rs`** for benchmark data.
- **Locale-specific cluster sizing via a named const** (`DE_CLUSTERS = 5`).
- **Required-loose-substrings drawn from the canonical article's lead, not the query phrasing.** EN-side convention from PR #45; re-confirmed standing in PR #93. Makes the loose check measure retrieval, not vocabulary alignment.

## Exact commands needed to resume

```bash
# Resume on main (after PR #97 merges):
cd /Users/hherb/src/primer
git status                       # confirm clean
git checkout main
git pull
git log --oneline -10            # the PR-97 squash-merge commit should be near the top

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 739 passed, 0 failed, 3 ignored.

~/.cargo/bin/cargo fmt --all -- --check
# Expected: clean.

RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets
# Expected: clean exit 0 (mirrors CI).
```

To re-run the German regression benchmarks (both flavours):

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_de
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_hybrid_de
# Real BGE-M3 recall floor (downloads ~570 MB on first run; cached afterwards):
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_quality_hybrid_de
```

To re-run the German sweep diagnostics (now via the shared helper at `tests/common/sweep.rs`):

```bash
cd /Users/hherb/src/primer/src

# BM25-only (always built; ~250ms wallclock):
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_sweep_de \
    -- --ignored sweep_retrieval_params_de --nocapture

# Hybrid (downloads ~570 MB BGE-M3 on first run; ~78s wallclock when cached):
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed \
    --test retrieval_sweep_hybrid_de \
    -- --ignored sweep_retrieval_params_hybrid_de --nocapture
```

For the Python ingestion pipeline tests (uv-only — never invoke pip directly):

```bash
cd /Users/hherb/src/primer/data/ingest
# (venv was set up per data/ingest/README.md: `uv venv .venv` +
# `uv pip install --python .venv/bin/python -r requirements.txt`)
.venv/bin/pytest tests/
# Expected: 135 passed.
```

For mypy on the ingest tree:

```bash
cd /Users/hherb/src/primer/data/ingest
mypy --python-executable .venv/bin/python simple_wikipedia.py wiki/ retry.py build_whitelist.py
# Expected: Success: no issues found in 7 source files.
```

For real-LLM smoke testing (Anthropic):

```bash
cd /Users/hherb/src/primer/src
RUST_LOG=debug ~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name SmokeTester --age 9 --no-persist --verbose 2>&1 | tee /tmp/smoke.log
```

For real-LLM smoke testing in German (66 Klexikon passages auto-loaded):

```bash
cd /Users/hherb/src/primer/src
RUST_LOG=debug ~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name Lukas --age 9 --language de --no-persist --verbose 2>&1 | tee /tmp/smoke_de.log
# Expected: KB auto-loads 66 Klexikon passages on locale=de.
```

For re-running the Klexikon ingest (rare; only when the whitelist changes or articles drift):

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/python simple_wikipedia.py --language de
# Writes ../seed/wiki_passages.de.jsonl. Commit any diff.
# Live HTTP traffic to klexikon.zum.de; ~66 sequential requests with 1s pacing
# (per-page parse strategy = no batching; takes ~66s on a warm network).
# 429/5xx retried 3× with backoff (PR #57).
# Post-resolution duplicate-id collisions raise RuntimeError (PR #59).
```

For re-running the Simple English Wikipedia ingest:

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/python simple_wikipedia.py --language en
# Writes ../seed/wiki_passages.en.jsonl. Commit any diff.
# Live HTTP traffic to simple.wikipedia.org; ~2 batched requests of 20 titles.
# 429/5xx retried 3× with backoff (PR #57).
```

For the speech build path: `~/.cargo/bin/cargo build --workspace --features primer-cli/speech`.

For the embedding feature build path:

```bash
~/.cargo/bin/cargo build --workspace --features primer-cli/embedding
~/.cargo/bin/cargo run --bin primer -- --embedder-backend fastembed ...
# First run downloads BGE-M3 (~570 MB) into the fastembed cache.
```

For running the BM25 sweep diagnostic (English):

```bash
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_sweep \
    -- --ignored sweep_retrieval_params --nocapture 2>&1 | tee /tmp/sweep.txt
```

For running the BM25 floor tripwire:

```bash
~/.cargo/bin/cargo test -p primer-kb-load --test bm25_floor_tripwire \
    -- --ignored bm25_score_floor_tripwire --nocapture
```

For running the hybrid sweep (English):

```bash
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed \
    --test retrieval_sweep_hybrid \
    -- --ignored sweep_retrieval_params_hybrid --nocapture 2>&1 | tee /tmp/sweep_hybrid.txt
```

For running the EN hybrid regression test:

```bash
# Structural (always built):
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_hybrid

# Real-recall (--features fastembed, real BGE-M3):
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed \
    --test retrieval_quality_hybrid -- --nocapture
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If you discover that Horst did interim work that changes the plan, flag it. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.
