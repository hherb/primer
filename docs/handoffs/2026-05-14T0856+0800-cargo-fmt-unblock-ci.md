# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-14T0856+0800 (after opening PR #94 — `cargo fmt` across workspace to unblock CI; commit `4db02ec` on `chore/cargo-fmt-main`).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **739 Rust tests** under default features (unchanged from the previous brief — PR #94 is fmt-only). 3 ignored. Add `--features primer-kb-load/fastembed` for the embedding-backed sweeps + the real-BGE-M3 recall tests (downloads BGE-M3 ~570 MB on first run; cached afterwards). Plus **135 Python tests** in `data/ingest/` (unchanged this session — Rust-only work).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.

## Branch status

`chore/cargo-fmt-main` carries 1 commit, pushed, and on **PR #94** (https://github.com/hherb/primer/pull/94). Built off `origin/main` at `10709ca` (PR #93 squash — German stress-paraphrase queries). Once #94 merges, the branch can be deleted.

**Note for next-session sanity check:** the prior NEXT_SESSION.md (2026-05-14T0818+0800) accurately described PR #93's state, but it also called the workspace fmt drift a "pre-existing fmt issue on main, should be filed as a follow-up issue." That follow-up *was* this session — the fmt-strict workflow (`cargo fmt --check` in `.github/workflows/ci.yml`) was already wired and had been failing on main since 2026-05-12 across PRs #85, #88, #89, #93. Always verify the current state of any flagged issue before acting on it.

## What we shipped this session

**Unblocked CI on `main`.** `cargo fmt --check` had been failing on every push to main since 2026-05-12 because pre-existing fmt drift in voice-mode and GUI code (PRs #79/#82/#83/#85/#89) never went through `cargo fmt`. Every subsequent PR carried a red CI mark for reasons unrelated to its own changes; signal had been eroded for ~5 days.

**Commits on `chore/cargo-fmt-main`:**

- `4db02ec` — `chore: cargo fmt across workspace to unblock CI`

**Concrete deliverables:**

- `~/.cargo/bin/cargo fmt --all` applied across the workspace. 13 files touched, all in voice/GUI code: `primer-cli/src/speech_loop/mod.rs`, `primer-gui/build.rs`, `primer-gui/src/{commands/session,commands/voice,config,lib,state}.rs`, `primer-gui/src/voice/{assets,download,observer}.rs`, `primer-speech/src/voice_loop/{backends,mod,state_machine}.rs`.
- Pure mechanical reformatting: multi-line `#[cfg(all(feature = ..., feature = ...))]` expansion, alphabetically-sorted `use` lists (rustfmt 2024 edition's default), standard line-length wrapping. No semantic changes.

**Verification:**

- `~/.cargo/bin/cargo fmt --all -- --check` clean (was failing before)
- `~/.cargo/bin/cargo clippy --workspace --all-targets` exit 0 (warnings unchanged from baseline; clippy lints aren't promoted to errors by CI)
- `~/.cargo/bin/cargo test --workspace --no-fail-fast` → 739 passed / 0 failed / 3 ignored (baseline unchanged)

**Net diff:** 13 files changed, 85 insertions, 65 deletions.

**Design choices that may be relevant later:**

- **No follow-up `cargo fmt` rule added.** The rustfmt 2024-edition defaults (notably alphabetical import sort) only manifest when someone runs `cargo fmt`; the CI check catches drift but doesn't auto-format. A pre-commit hook that runs `cargo fmt --check` locally would prevent drift from landing in the first place, but adding it touches developer-workflow expectations and isn't a one-line change — defer until the next time drift happens.
- **All 13 files belong to recent voice/GUI work.** This isn't a codebase-wide accumulation; it's specifically the post-PR-#79 work that landed without the author running `cargo fmt`. Consider it a signal that the GUI/voice workflow doesn't go through the same local check as Rust-core work — worth flagging in any future agent-instruction iteration.

## What's next

### The most defensible smaller-scope follow-ups

- **Issue #67 — Defensive `.unwrap()` → `.unwrap_or(Ordering::Equal)` in hybrid `pick_winner` partial_cmp chains.** Trivial mechanical fix (~6 lines across `retrieval_sweep_hybrid.rs` and `retrieval_sweep_hybrid_de.rs`); the BM25 sweep variants already use this defensive pattern. Acceptance: existing `--ignored` invocations produce identical output (the grid never hits NaN today, so the change is graceful-degradation polish, not a behavioural test). Could naturally bundle with issue #66.
- **Issue #66 — Dedupe retrieval-sweep harnesses across locales into `tests/common/sweep.rs`.** Now that EN+DE pair is in flight, consolidating before a hypothetical third locale (Hindi) avoids tripling the duplicated code. ~95% of each pair is bit-identical Rust; the locale-specific bits are `Locale::English` vs `Locale::German`, `QUERIES` vs `QUERIES_DE`, cluster array sizing (6 vs 5), and label strings. Concrete shape proposed in the issue: a `SweepConfig` struct + `run_bm25_sweep`/`run_hybrid_sweep` helpers; each per-locale file shrinks to ~30 lines. Acceptance: identical `--ignored` output before/after (diff by stripping timestamps); adding a new locale doesn't require touching the helper module.
- **Issue #90 — Voice asset path re-resolution server-side (security hardening).** `download_voice_assets` currently round-trips a full `MissingAsset` payload (path + suggested_url) through the webview; if the webview is ever compromised, a hostile payload could direct writes outside `~/.cache/primer/models/`. Concrete fix: have the frontend echo only `kind` strings, re-resolve `path`+`suggested_url` server-side via the existing `assets::resolve_voice_assets`. Surface from the PR #89 review.

### Larger queue items (carried forward)

- **Local llama.cpp inference (Phase 1.1).** `LlamaCppBackend` stub at `primer-inference` is the entry point.
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality, not production). The voice mode (Phase A) GUI work landed in PR #89 — production polish is the still-open piece. Issues #90/91/92 are the immediate review-fallout follow-ups.
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **Hindi (`hi`) locale pack rollout.** Schema + i18n boundary are already locale-keyed; the open work is the prompt-pack TOML, the seed corpus / wiki layer, and BGE-M3 retrieval validation in Devanagari. Check Wikipedia for an analogue of "Bal Vikipedia" (बाल विकिपीडिया) — the German pattern (Klexikon over de.wikipedia) confirms the heuristic: prefer the children's-vocabulary source whenever one exists. With the test-file pattern from PRs #63 / #65 / #93, adding a Hindi benchmark is a `tests/common/hi.rs` + parallel `retrieval_quality_hi.rs` + parallel `retrieval_sweep_hi.rs` plus a `WikiSource` preset in `wiki/source.py`. **Worth doing #66 first so this lands as one helper update + new locale data, not a third copy of the harness.**
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
- **#46** — explore post-RRF `min_score` as a fifth grid axis in the hybrid sweep.
- **#40** — aggregate per-source attribution row for Wikipedia (and Klexikon).
- **#41** — tighten disambiguation regex if false positives appear.
- **#22** — cache prepared statements for the corpus-bootstrap path (Phase 0.2; enhancement).
- **#21** — separate `--languages` preference list from bound `--language` locale.
- **#20** — i18n placeholder validator can false-fail on translator narrative text.
- **Failed-batch persistence sidecar (issue #38 optional).** Was deferred during brainstorming; file as new issue if scheduled ingest ever ships.
- **Network-error retry.** `requests.exceptions.ConnectionError` / `Timeout` still propagate unchanged.
- **Pre-commit fmt hook (workflow-level).** Drift accumulated 13 files across 5 PRs before CI noticed; a local `cargo fmt --check` pre-commit hook would prevent it. Defer until the next time drift happens.

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

- **The fmt-strict CI step is already wired but local workflow doesn't honour it.** `.github/workflows/ci.yml` runs `cargo fmt --all -- --check` as the first step after toolchain install; it had been failing for ~5 days before this session noticed. The cost of a red CI is real (every PR-author has to mentally distinguish "my CI red" from "pre-existing main red") but the local workflow has no equivalent gate. A pre-commit hook is the cheapest fix; adding it is a workflow-policy decision and deferred for now.
- **Rustfmt 2024-edition's alphabetical import sort is a frequent drift source.** Nine of the 13 reformatted hunks were `use foo::{Bar, Baz}` → `use foo::{Baz, Bar}` (or vice-versa) — drift triggered the moment an author adds or removes an import without re-running `cargo fmt`. Worth being aware of when reviewing GUI/voice diffs: a 1-line import re-sort isn't a semantic change.
- **NEXT_SESSION.md "follow-up issue" language can be deceptive.** The prior brief described the fmt drift as a "should be filed as a follow-up issue" item, which downgraded it from "real blocker" to "TODO" mentally. It was actually already a real CI blocker the entire time. When prior briefs flag a problem, verify whether it's still latent or has become active.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing, with no new patterns added this session.)

- **Pure functions in dedicated modules** for algorithmic cores — tested directly without I/O.
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level.
- **Carrier structs with `disabled()` no-op constructors** for parameter bundles that not all callers need to configure.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed.
- **Constants in `consts.rs` submodules** (Rust) or top-of-module `_DEFAULT_*` constants (Python). No magic numbers anywhere.
- **TDD discipline.** Tests first; watch them fail; implement to green. **Not applicable this session** — fmt is mechanical with no functional change to test.
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
- **Two-commit refactor: "set up the change" then "remove the old".**
- **Ship the follow-up issue body with explicit acceptance + per-file checklist when a PR ships a transitional state.**
- **Per-locale dataset modules under `tests/common/<pack>.rs`** for benchmark data.
- **Locale-specific cluster sizing via a named const** (`DE_CLUSTERS = 5`).
- **Required-loose-substrings drawn from the canonical article's lead, not the query phrasing.** EN-side convention from PR #45; re-confirmed standing in PR #93. Makes the loose check measure retrieval, not vocabulary alignment.

## Exact commands needed to resume

```bash
# Resume on main (after PR #94 merges):
cd /Users/hherb/src/primer
git status                       # confirm clean
git checkout main
git pull
git log --oneline -10            # the PR-94 squash-merge commit should be near the top

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 739 passed, 0 failed, 3 ignored.

~/.cargo/bin/cargo fmt --all -- --check
# Expected: clean (after PR #94 merges; was failing before).

~/.cargo/bin/cargo clippy --workspace --all-targets
# Expected: warnings only, exit 0.
```

To re-run the German regression benchmarks (both flavours):

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_de
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_hybrid_de
# Real BGE-M3 recall floor (downloads ~570 MB on first run; cached afterwards):
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_quality_hybrid_de
```

To re-run the German sweep diagnostics:

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
