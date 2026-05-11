# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-11T2206+0800 (after opening PR #65 — German retrieval-sweep diagnostics; commit `aa4e084` on `feat/de-retrieval-sweep`).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **658 Rust tests** under default features after PR #65 merges (was 648; +10 from the new sweep test binary duplicating `common::de` sanity tests). 3 ignored. Add `--features primer-kb-load/fastembed` for the embedding-backed sweeps + the real-BGE-M3 recall test (downloads BGE-M3 ~570 MB on first run; cached afterwards). Plus **135 Python tests** in `data/ingest/` (unchanged this session — Rust-only work).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.

## Branch status

`feat/de-retrieval-sweep` carries 1 commit, pushed, and on **PR #65** (https://github.com/hherb/primer/pull/65). Built off `origin/main` after PR #63 merged (squash commit `9093a99`). Once #65 merges, the branch can be deleted.

## What we shipped this session

**Added parallel German retrieval-parameter sweep diagnostics** mirroring the existing English sweep tooling. Two new `#[ignore]`'d harnesses measure the BM25-only and hybrid retrieval grids against `QUERIES_DE` over the 66-passage Klexikon corpus. They close the prior open follow-up "no DE sweep diagnostic exists yet" and serve as drift guards for future German corpus expansion.

**Commits on `feat/de-retrieval-sweep`:**

- `aa4e084` — `test(retrieval): add German retrieval-sweep diagnostics`

**Concrete deliverables:**

- `src/crates/primer-kb-load/tests/retrieval_sweep_de.rs` (~260 lines): 24-cell `(top_k × min_score)` BM25-only sweep against `QUERIES_DE`. `#[ignore]`'d so it stays out of default CI. Per-cluster recall breakdown over five clusters (`Cluster::Wiki` omitted — Klexikon IS the German wiki; same justification as the cluster-floor sanity test).
- `src/crates/primer-kb-load/tests/retrieval_sweep_hybrid_de.rs` (~210 lines): 54-cell `(bm25_top_k × vector_top_k × final_top_k × rrf_k)` hybrid sweep gated on `--features fastembed`. Real BGE-M3, ~570 MB first-run download (cached afterwards).
- Doc updates: `CLAUDE.md` (added a parallel-bullet describing the DE sweep diagnostics, replacing the prior "no German sweep diagnostic exists yet" open-follow-up text); `README.md` (parallel German diagnostics now mentioned alongside the EN sweep summary in the tuned-hybrid-defaults bullet); `ROADMAP.md` (new `✅ German retrieval-sweep diagnostics` bullet directly under the German retrieval-quality benchmark closure).

**Findings against today's 66-passage Klexikon corpus + 26-query benchmark:**

- **BM25 sweep:** strict recall is 95% at `top_k=3` (one space query miss across all `min_score` values), then **flatlines at 100% from `top_k=5` onward regardless of `min_score`**. Production `(top_k=5, min_score=0.5)` is a comfortable choice — every cell at fixed `top_k≥5` produced identical recall. The algorithmic winner per the lex rule is `top_k=5, min_score=1.50` but production deliberately stays at 0.5 for the same reason as the EN side (brittle against future corpus dilution).
- **Hybrid sweep:** **100% loose / 100% strict in every one of the 54 cells.** The multilingual dense leg adds no measurable lift for this corpus/query mix — corroborating the standing observation from PR #63 that BM25 alone already clears the German benchmark.

No production defaults change. The diagnostics serve purely as drift guards.

**Verification:**
- `~/.cargo/bin/cargo build --workspace` clean
- `~/.cargo/bin/cargo test --workspace` → 658 passed / 0 failed / 3 ignored (was 648 / 0 / 2 after PR #63 squash-merged to `9093a99` on main)
- `~/.cargo/bin/cargo clippy --workspace --all-targets` clean
- `~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-kb-load/fastembed` clean
- `~/.cargo/bin/cargo fmt --all -- --check` clean
- `~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_sweep_de -- --ignored sweep_retrieval_params_de --nocapture` → 1 passed; winning cell `top_k=5, min_score=1.50` at 100% loose / 100% strict; per-cluster table prints `Spc/Bod/How/Lif/Erw` (no Wik column)
- `~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_sweep_hybrid_de -- --ignored sweep_retrieval_params_hybrid_de --nocapture` → 1 passed; ~70s wallclock with BGE-M3 already cached

**Net diff:** 5 files changed, 519 insertions, 3 deletions.

**Design choices that may be relevant later:**

- The DE BM25 sweep's `per_cluster` array is sized via a `const DE_CLUSTERS: usize = 5;` constant rather than reusing the EN harness's hard-coded `6`. This makes the omission of `Cluster::Wiki` self-documenting and minimises drift risk if the EN harness adds a seventh cluster.
- The print header drops the `Wik` column for DE so the table stays aligned without dummy `-` cells. Width arithmetic was adjusted (90-char rule line vs. the EN 96-char rule).
- The hybrid harness's `--features fastembed` gate matches the EN side exactly. Combined fastembed builds (`--features primer-kb-load/fastembed`) compile both new files cleanly under workspace clippy.
- The selection-rule docstring in `pick_winner` references the standing design spec (`docs/superpowers/specs/2026-05-06-retrieval-tuning-design.md`) instead of restating it, since the rule is identical for `de`.
- Both files run cleanly on the existing BGE-M3 cache from prior PR #63 testing — no first-run download time on this machine.

## What's next

### The most defensible smaller-scope follow-ups

- **DE stress-paraphrase queries.** Add 5-10 deliberately mis-aligned child paraphrases to `QUERIES_DE` — child-style phrasings whose canonical passage uses different vocabulary, so the BM25 leg can't reach them but the dense leg can. Same shape as EN issue #45's paraphrase expansion. Concrete acceptance: each new paraphrase carries a strict `canonical_id`, BM25-only retrieval misses ≥80% of them at production defaults (so they motivate the hybrid lift), hybrid still hits 100%. Populates `KNOWN_FAILING_QUERIES_DE` for the BM25-only test while keeping `KNOWN_FAILING_QUERIES_DE_HYBRID` empty. Closes the long-standing open follow-up flagged in both NEXT_SESSION.md and CLAUDE.md.
- **Klexikon corpus expansion past 66** (carried forward). Klexikon has ~3000 articles. Concrete next-batch acceptance: pick 30-50 more titles in still-thin clusters — body could use Atmung-related topics; life could use Ökosystem, Affe/Pferd/Fledermaus; how-things-work could use Computer/Internet/Telefon/Maschine. Re-run pipeline. No code change required. Once corpus grows past ~150 passages, re-run the new DE sweep harnesses to verify production defaults still flatline at 100% strict from `top_k=5` onward (and that the BM25 floor's defensive 0.5 hasn't been challenged by the larger statistic).
- **Klexikon license claim spot-check** (carried forward). Concrete acceptance: WebFetch a sample of ~5 article footers (e.g. `Sonne`, `Atome und Moleküle`, `Skelett`, `Bienen`, `Mondfinsternis`) and verify each footer shows CC-BY-SA-4.0. If a per-page divergence appears, document a per-passage license override field in `WikiSource`. Low-priority.

### Larger queue items (carried forward)

- **Local llama.cpp inference (Phase 1.1).** `LlamaCppBackend` stub at `primer-inference` is the entry point.
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality, not production).
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **Hindi (`hi`) locale pack rollout.** Schema + i18n boundary are already locale-keyed; the open work is the prompt-pack TOML, the seed corpus / wiki layer, and BGE-M3 retrieval validation in Devanagari. Check Wikipedia for an analogue of "Bal Vikipedia" (बाल विकिपीडिया) — the German pattern (Klexikon over de.wikipedia) confirms the heuristic: prefer the children's-vocabulary source whenever one exists. With the test-file pattern from PRs #63 and #65, adding a Hindi benchmark is a `tests/common/hi.rs` + parallel `retrieval_quality_hi.rs` + parallel `retrieval_sweep_hi.rs` plus a `WikiSource` preset in `wiki/source.py`.
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default. The relevant flips: `default = ["embedding"]` in `primer-cli/Cargo.toml` and the `--embedder-backend` CLI default.

### Smaller-scope follow-ups still open

- **#46** — explore post-RRF `min_score` as a fifth grid axis in the hybrid sweep.
- **#40** — aggregate per-source attribution row for Wikipedia (and Klexikon).
- **#41** — tighten disambiguation regex if false positives appear.
- **#22** — cache prepared statements for the corpus-bootstrap path (Phase 0.2; enhancement).
- **#21** — separate `--languages` preference list from bound `--language` locale.
- **#20** — i18n placeholder validator can false-fail on translator narrative text.
- **Failed-batch persistence sidecar (issue #38 optional).** Was deferred during brainstorming; file as new issue if scheduled ingest ever ships.
- **Network-error retry.** `requests.exceptions.ConnectionError` / `Timeout` still propagate unchanged.

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

- **The DE BM25 sweep showed the same flatline shape as the EN sweep** — `top_k=3` is the only column where strict recall falls below 100% (95% in DE; equivalent dip in EN). This is consistent across both languages and corpora, supporting the choice of `KB_FINAL_TOP_K = 5` as a load-bearing constant rather than something to revisit per-locale.
- **Every cell in the 54-cell DE hybrid grid clears 100% / 100%.** This is a stronger signal than the EN side, where some `(low_bm25_top_k, low_vector_top_k)` cells do dip on stress paraphrases. For the German corpus + queries-aligned-to-article-vocabulary mix, hybrid parameter choice is unconstrained — confirming the stress-paraphrase follow-up is a real signal generator, not just a theoretical concern.
- **The sweep harnesses execute fast enough to be future-CI candidates.** DE BM25: ~250ms. DE hybrid: ~70s with BGE-M3 cached. The hybrid time is dominated by single-thread embedding inference; if hybrid sweeps ever move into CI, a `--features fastembed` job with a model-cache key is the path. Today both stay `#[ignore]`'d to keep CI fast.
- **The DE_CLUSTERS constant explicitly captures the omission of `Cluster::Wiki`.** This is the third place in the test suite that has to know "Wiki doesn't exist for DE" (cluster-floor sanity test, BM25 sweep, hybrid sweep). If/when a German hand-drafted seed layer is added, all three sites need to flip in lockstep. The cluster-floor sanity test already carries a comment to this effect; the BM25 sweep's `DE_CLUSTERS` constant is the new equivalent flag.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing, with one new pattern from this session.)

- **Pure functions in dedicated modules** for algorithmic cores — tested directly without I/O.
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level.
- **Carrier structs with `disabled()` no-op constructors** for parameter bundles that not all callers need to configure.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed.
- **Constants in `consts.rs` submodules** (Rust) or top-of-module `_DEFAULT_*` constants (Python). No magic numbers anywhere.
- **TDD discipline.** Tests first; watch them fail; implement to green. (Soft this session: the new sweep harnesses are diagnostic test infrastructure, not regression tests with up-front assertions — but the run-and-classify cycle was honoured. Both harnesses were authored, the BM25 one was run to discover the flatline shape, and the hybrid one was run to confirm the 100/100 ceiling, before doc updates landed.)
- **File-size hygiene.** Keep modules under 500 lines. ✅ This session: `retrieval_sweep_de.rs` 259 lines, `retrieval_sweep_hybrid_de.rs` 211 lines.
- **Network-injection test seam** for any data-ingest pipeline (`http_client` parameter, substitute `FakeHttpClient`/`KlexikonFakeHttpClient` in tests).
- **Subagent-driven development workflow** for plan execution (or inline executing-plans for small TDD-shaped plans).
- **Defensive sanity tests at the data layer.** Confirmed standing.
- **Always pin `Default` impls of public structs to `consts::*`, with a drift-prevention test.** Mirrored on the Python side as `RetrySettings.default()` ↔ module-level `DEFAULT_*` consts.
- **Pure helpers + their own unit tests, even in `#[ignore]`'d integration tests.**
- **Frozen dataclasses for process-wide configuration** (the `WikiSource` pattern; `RetrySettings` follows the same shape).
- **String discriminators for strategy selection, with allow-list validation.**
- **Re-run the live ingest after changing any fetch-path helper.** Tiny expected diff = success, anything bigger = a flag worth investigating.
- **Kwarg-injected side-effect functions for TDD seams in Python.**
- **Structural ingest-time defences beat manual probing habits.** Confirmed via PR #59 review feedback.
- **Back-compat re-export shims when bulk-editing test imports would dilute a structural-refactor PR.** Confirmed PR #60 / #61 — shim landed first, lived for one PR cycle, then went away as a 2-commit cleanup.
- **Plan-then-execute inline for mechanical refactors with a strong test safety net.** Re-confirmed.
- **Two-commit refactor: "set up the change" then "remove the old".** Splits a "migrate + delete" diff into a bisectable mid-state and a clean cleanup commit.
- **Ship the follow-up issue body with explicit acceptance + per-file checklist when a PR ships a transitional state.**
- **Per-locale dataset modules under `tests/common/<pack>.rs`** for benchmark data. Pattern from PR #63 — confirmed reusable in PR #65 (sweep harnesses).
- **Locale-specific cluster sizing via a named const** (`DE_CLUSTERS = 5`). New this session. Captures "this locale has N clusters" in one place; the line containing the const declaration is the single grep target for adding a future locale's cluster count. Better than naked `6` (EN) / `5` (DE) literals scattered across array sizes and cluster-array initializers.

## Exact commands needed to resume

```bash
# Resume on main (after PR #65 merges):
cd /Users/hherb/src/primer
git status                       # confirm clean
git checkout main
git pull
git log --oneline -10            # the PR-65 squash-merge commit should be near the top

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 658 passed, 0 failed, 3 ignored (after PR #65 merges).

~/.cargo/bin/cargo clippy --workspace --all-targets
~/.cargo/bin/cargo fmt --all -- --check
```

To re-run the German sweep diagnostics:

```bash
cd /Users/hherb/src/primer/src

# BM25-only (always built; ~250ms wallclock):
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_sweep_de \
    -- --ignored sweep_retrieval_params_de --nocapture

# Hybrid (downloads ~570 MB BGE-M3 on first run; ~70s wallclock when cached):
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed \
    --test retrieval_sweep_hybrid_de \
    -- --ignored sweep_retrieval_params_hybrid_de --nocapture
```

To re-run the German regression benchmarks (both flavours):

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_de
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_hybrid_de
# Real BGE-M3 recall floor (downloads ~570 MB on first run; cached afterwards):
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_quality_hybrid_de
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
