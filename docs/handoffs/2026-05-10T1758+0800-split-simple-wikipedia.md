# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-10 (after the `simple_wikipedia.py` split into `wiki/` submodules; PR #60 open).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **605 Rust tests** under default features (unchanged this session — refactor was Python-only). Add `--features primer-kb-load/fastembed` for the embedding-backed sweep + recall test (downloads BGE-M3 ~570 MB on first run; cached afterwards). Plus **135 Python tests** in `data/ingest/` (unchanged this session — refactor preserved every test as-is via the back-compat shim).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.

## Branch status

`refactor/split-simple-wikipedia` carries 7 commits, pushed, and on **PR #60** (https://github.com/hherb/primer/pull/60). Built off `origin/main` after the PR #59 (Klexikon corpus 24 → 66) merge. Once #60 merges, the branch can be deleted.

## What we shipped this session

**`data/ingest/simple_wikipedia.py` split into focused submodules under `data/ingest/wiki/`.** Pure mechanical refactor — zero behaviour change. Cleared the deferred follow-up flagged in the previous handoff ("simple_wikipedia.py is 927 lines... defer until a third source comes in") because file-size hygiene is a standing project guideline, not a "wait for a third source" trigger.

**7 commits on `refactor/split-simple-wikipedia`:**

- `608386f` — `refactor(ingest): create empty wiki/ package for upcoming split`
- `51504d2` — `refactor(ingest): extract wikitext stripper to wiki/strip.py`
- `a947c5f` — `refactor(ingest): extract WikiSource + identity helpers to wiki/source.py`
- `de1e691` — `refactor(ingest): extract HTTP fetch dispatch to wiki/fetch.py`
- `34c98d8` — `refactor(ingest): rewrite simple_wikipedia.py docstring for new role`
- `2385ae8` — `docs: update CLAUDE.md + data/ingest/README.md for the wiki/ submodule layout`
- `a92ef69` — `docs(plans): record implementation plan for the simple_wikipedia.py split`

**Concrete deliverables:**

- New `data/ingest/wiki/` package with three submodules:
  - `wiki/source.py` (346 lines) — `WikiSource` dataclass + `_VALID_FETCH_STRATEGIES`, `SIMPLE_ENGLISH` and `KLEXIKON` presets, language-specific disambiguation patterns, `_SOURCES_BY_PACK_ID`, `slugify` + `_assert_unique_slugs` + `_assert_unique_passage_ids`, `read_whitelist`, `to_passage` + `_strip_math_artifacts`.
  - `wiki/strip.py` (163 lines) — Klexikon wikitext → plain text. `strip_klexikon_wikitext` + `_strip_balanced_drop_blocks` + all wikitext regexes. Pure functions, no I/O.
  - `wiki/fetch.py` (336 lines) — HTTP dispatch (`fetch_lead`, `fetch_leads`) + per-strategy fetchers (`_fetch_*_via_text_extracts`, `_fetch_*_via_klexikon`) + `_klexikon_canonical_url` + `_check_disambiguation` + `_DEFAULT_USER_AGENT` + `_RETRY_SETTINGS`.
- `simple_wikipedia.py` reduced from 970 → 242 lines: now `main` (pipeline orchestrator) + `_parse_args` + `_default_whitelist_path` + `_default_output_path` + `_SHORT_LEAD_WORD_THRESHOLD` + `__main__` block + back-compat re-export shims for every public/private name imported by the existing 135 Python tests.
- Doc updates: `CLAUDE.md` retry-bullet now points at `wiki/fetch.py` for `_RETRY_SETTINGS`; new bullet describes the submodule layout. `data/ingest/README.md` adds a "Submodule layout" section. Top-level `README.md` and `ROADMAP.md` left alone (internal cleanup, not user-facing news).
- Plan file at [`docs/superpowers/plans/2026-05-10-split-simple-wikipedia.md`](docs/superpowers/plans/2026-05-10-split-simple-wikipedia.md) — the full step-by-step implementation plan, including line-range maps for each extraction.
- Verification at every commit: `pytest data/ingest/tests/ -q` shows `135 passed`. Final smoke confirmed `from simple_wikipedia import main, KLEXIKON, SIMPLE_ENGLISH, slugify, strip_klexikon_wikitext, fetch_lead, _klexikon_canonical_url, read_whitelist, to_passage, _assert_unique_slugs, _assert_unique_passage_ids, WikiSource` resolves cleanly; `python simple_wikipedia.py --help` prints unchanged flags.

**Why it was worth doing now (vs. deferring "until a third source comes in"):**

- File size hygiene (≤500 lines) is a standing project guideline, not a "deferred until N" trigger.
- Per-commit reviewability is much higher with the smaller modules — a future German benchmark or a Hindi-source addition won't churn through a 970-line monolith.
- The back-compat shim makes the change risk-free for callers (135 tests stay green; CLI invocation unchanged; new code can opt into the cleaner submodule path).

## What's next

### Larger queue items (carried forward)

- **Local llama.cpp inference (Phase 1.1).** `LlamaCppBackend` stub at `primer-inference` is the entry point.
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality, not production).
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **Hindi (`hi`) locale pack rollout.** Schema + i18n boundary are already locale-keyed; the open work is the prompt-pack TOML, the seed corpus / wiki layer, and BGE-M3 retrieval validation in Devanagari. Open question: Wikipedia has a Hindi children's-wiki analog called "Bal Vikipedia" (बाल विकिपीडिया) — worth checking if it's API-accessible before defaulting to regular hi.wikipedia.org. The German session pattern (Klexikon over de.wikipedia) confirms the heuristic: prefer the children's-vocabulary source whenever one exists. With the new submodule layout, adding a Hindi source is a `WikiSource` preset in `wiki/source.py` plus (if needed) a new `fetch_strategy` branch in `wiki/fetch.py` — clean per-file change.
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default. The relevant flips: `default = ["embedding"]` in `primer-cli/Cargo.toml` and the `--embedder-backend` CLI default.

### Smaller-scope follow-ups still open

- **German retrieval-quality benchmark.** No equivalent of `tests/retrieval_quality.rs` exists for the German layer yet. Concrete acceptance: hand-author a German equivalent of `tests/common/mod.rs::QUERIES` (~25 child-style German questions targeting specific Klexikon ids), wire a parallel test that runs against the de KB. Higher value now that the corpus is 66 articles deep — 24 was at the cusp where regression-protection cost-benefit was weak. **Suggested target: ~25 BM25-only loose queries + ~12 strict canonical-id mappings; an `--ignored` BGE-M3 hybrid recall test as a follow-up.** This was the originally suggested task before Horst pivoted to the file split — still the highest-value smaller item in the queue.
- **Klexikon license claim spot-check.** Site-wide CC BY-SA 4.0 is the canonical claim and is what the 66 passages now uniformly carry. Concrete acceptance: WebFetch a sample of ~5 article footers (e.g. `Sonne`, `Atom`/`Atome und Moleküle`, `Skelett`, `Bienen`, `Mondfinsternis`) and verify the footer license matches. If a per-page divergence appears, document the override path in `WikiSource` (probably needs a per-passage license override field). Low-priority.
- **Klexikon corpus expansion past 66.** Klexikon has ~3000 articles. Concrete next-batch acceptance: pick 30-50 more titles in still-thin clusters — body could use Atmung-related topics; life could use Ökosystem, animal-specific (Affe, Pferd, Fledermaus); how-things-work could use Computer, Internet, Telefon, Maschine. Re-run pipeline. No code change required (the structural duplicate-id check from PR #59 makes redirect-collision risk a hard fail rather than a silent dup).
- **#46** — explore post-RRF `min_score` as a fifth grid axis in the hybrid sweep.
- **#40** — aggregate per-source attribution row for Wikipedia (and Klexikon).
- **#41** — tighten disambiguation regex if false positives appear.
- **#22** — cache prepared statements for the corpus-bootstrap path (Phase 0.2; enhancement).
- **#21** — separate `--languages` preference list from bound `--language` locale.
- **#20** — i18n placeholder validator can false-fail on translator narrative text.
- **Failed-batch persistence sidecar (issue #38 optional).** Was deferred during brainstorming; file as new issue if scheduled ingest ever ships.
- **Network-error retry.** `requests.exceptions.ConnectionError` / `Timeout` still propagate unchanged.

### Migrate test imports to the new submodule paths (minor, low-priority)

The existing 135 tests import everything from `simple_wikipedia` via the back-compat shim. New code is encouraged to import from the submodule directly (`from wiki.fetch import fetch_lead`) per the docs we updated. As a low-stakes follow-up, the existing tests could be migrated one file at a time to the cleaner submodule paths. Concrete acceptance: each test file's imports updated; 135 tests still pass; the back-compat re-exports in `simple_wikipedia.py` can then be slimmed down (but leave `WikiSource`, `SIMPLE_ENGLISH`, `KLEXIKON`, and a handful of public names because external callers — if any — may still reach for them). Not worth doing in isolation; bundle with the next test edit in this area.

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

**New observations from this session (not risks per se):**

- **Back-compat re-export shims are a safe refactoring tool when the test surface is wide.** 135 tests imported a mix of public and private names directly from `simple_wikipedia`. Bulk-editing all those imports in the same PR would have made the diff much harder to review and would have coupled the structural change with a test migration. The shim isolates the move and lets the import migration happen as a separate, optional follow-up.
- **The "defer until a third source comes in" trigger was wrong framing.** A 970-line file already violates the standing 500-line guideline; the fact that two sources fit within it isn't a counter-argument. File-size hygiene operates on the file, not on the number of users it serves. When you find that framing in a future handoff, push back.
- **Plan-then-execute inline still beats subagent-driven for mechanical refactors.** Eight tasks, each well-defined, with the test suite as the safety net — subagent-driven would have spent overhead on context handoffs without adding review value. Inline executing-plans was the right call. Subagents pay off when tasks are independent enough to run in parallel or when the work has design surface that benefits from a fresh view.
- **A linter diagnostic from earlier work is not always on you.** The mypy `Library stubs not installed for "requests"` diagnostic that fired on every Edit is on the pre-existing `import requests` line in `main`, not anything I touched. Worth flagging but not worth fixing inside an unrelated PR.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing.)

- **Pure functions in dedicated modules** for algorithmic cores — tested directly without I/O. (Confirmed standing this session: `wiki/strip.py` is a textbook example; the wikitext stripper is purely textual transformation with no module-level state.)
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level.
- **Carrier structs with `disabled()` no-op constructors** for parameter bundles that not all callers need to configure.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed.
- **Constants in `consts.rs` submodules** (Rust) or top-of-module `_DEFAULT_*` constants (Python). No magic numbers anywhere.
- **TDD discipline.** Tests first; watch them fail; implement to green. (Not exercised this session — pure mechanical refactor with zero behaviour change. The 135 existing tests served as the safety net at every commit.)
- **File-size hygiene.** Keep modules under 500 lines. ✅ This session: `simple_wikipedia.py` 970 → 242; new submodules 163, 336, 346.
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
- **Back-compat re-export shims when bulk-editing test imports would dilute a structural-refactor PR.** New this session.
- **Plan-then-execute inline for mechanical refactors with a strong test safety net.** New this session.

## Exact commands needed to resume

```bash
# Resume on main (after PR #60 merges):
cd /Users/hherb/src/primer
git status                       # confirm clean
git checkout main
git pull
git log --oneline -10            # the file-split merge commit should be near the top

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 605 passed, 0 failed, 2 ignored (as of 2026-05-10).

~/.cargo/bin/cargo clippy --workspace --all-targets
~/.cargo/bin/cargo fmt --all -- --check
```

For the Python ingestion pipeline tests:

```bash
cd /Users/hherb/src/primer/data/ingest
# (venv was set up per data/ingest/README.md)
.venv/bin/pytest tests/
# Expected: 135 passed.
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
# CLI invocation unchanged after the file split.
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
# First run downloads BGE-M3 (~570 MB) into ~/.cache/fastembed/.
```

For running the BM25 sweep diagnostic:

```bash
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_sweep \
    -- --ignored sweep_retrieval_params --nocapture 2>&1 | tee /tmp/sweep.txt
```

For running the BM25 floor tripwire:

```bash
~/.cargo/bin/cargo test -p primer-kb-load --test bm25_floor_tripwire \
    -- --ignored bm25_score_floor_tripwire --nocapture
```

For running the hybrid sweep:

```bash
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed \
    --test retrieval_sweep_hybrid \
    -- --ignored sweep_retrieval_params_hybrid --nocapture 2>&1 | tee /tmp/sweep_hybrid.txt
```

For running the hybrid regression test:

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
