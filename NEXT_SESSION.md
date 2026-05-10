# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-10T1843+0800 (after closing #61 — back-compat shim dropped from `simple_wikipedia.py`; PR #62 open. Also bundled an inline mypy stub fix: `types-requests` added to `requirements.txt` and the README setup steps switched from `pip` to `uv`).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **605 Rust tests** under default features (unchanged this session — work was Python-only). Add `--features primer-kb-load/fastembed` for the embedding-backed sweep + recall test (downloads BGE-M3 ~570 MB on first run; cached afterwards). Plus **135 Python tests** in `data/ingest/` (down from 138 — three `test_shim_identity.py` tripwires went away with the shim they pinned).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.

## Branch status

`refactor/issue-61-drop-shim` carries 2 commits, pushed, and on **PR #62** (https://github.com/hherb/primer/pull/62). Built off `origin/main` after PR #60 merged. Once #62 merges, the branch can be deleted.

## What we shipped this session

**Closed issue #61** — dropped the `simple_wikipedia.py` back-compat re-export shim that was added in PR #60. The shim re-exported underscore-prefixed private names (`_RETRY_SETTINGS`, `_NON_ALNUM`, `_DROP_BLOCK_PREFIX_LOOKAHEAD`, …) across module boundaries (broke the "underscore = don't reach in" social contract) and shim-name monkeypatches didn't reach the live submodule globals (a quiet correctness trap). Both traps are now gone because every test imports directly from the `wiki/` submodule that owns the name.

**Commits on `refactor/issue-61-drop-shim`:**

- `4778c2a` — `refactor(ingest): migrate ingest tests off simple_wikipedia shim (#61)`
- `811732a` — `refactor(ingest): drop simple_wikipedia back-compat shim (closes #61)`
- `63c80a1` — `docs(handoff): refresh NEXT_SESSION + archive timestamped copy`
- (plus a follow-up commit for the inline mypy stub + uv-only README — sha-pending until committed)

**Concrete deliverables:**

- 7 ingest test files updated to import from `wiki.source` / `wiki.strip` / `wiki.fetch`:
  - `tests/test_slugify.py` → `from wiki.source import slugify, _assert_unique_passage_ids, _assert_unique_slugs`
  - `tests/test_wikitext_strip.py` → `from wiki.strip import strip_klexikon_wikitext`
  - `tests/test_fetch_lead.py` → `from wiki.fetch import _klexikon_canonical_url, fetch_lead, fetch_leads` + `from wiki.source import KLEXIKON, SIMPLE_ENGLISH`
  - `tests/test_wiki_source.py` → `from wiki.source import KLEXIKON, SIMPLE_ENGLISH, WikiSource`
  - `tests/test_whitelist_parser.py` → `from wiki.source import read_whitelist`
  - `tests/test_passage_emit.py` → `from wiki.source import KLEXIKON, SIMPLE_ENGLISH, to_passage`
  - `tests/test_pipeline.py` → `from simple_wikipedia import main` (legitimate — `main` lives in the CLI module) + `from wiki.source import KLEXIKON, SIMPLE_ENGLISH`
- `simple_wikipedia.py` shrank 242 → 186 lines. Three `# ── … (moved to wiki.* — re-exported for back-compat) ──` import blocks removed (lots of underscore-prefixed names plus the public ones). The surviving imports are functional usage by `main` and the CLI helpers (`_DEFAULT_USER_AGENT`, `fetch_leads`, `KLEXIKON`, `SIMPLE_ENGLISH`, `WikiSource`, `_SOURCES_BY_PACK_ID`, `_assert_unique_passage_ids`, `_assert_unique_slugs`, `read_whitelist`, `to_passage`). Module docstring rewritten to drop the shim and monkeypatch-gotcha language.
- `tests/test_shim_identity.py` deleted (the `is`-identity tripwire that pinned every shim re-export — no shim, no tripwire needed).
- Doc updates: `CLAUDE.md` bullet describing `simple_wikipedia.py` rewritten ("CLI entry point only", ~186 lines, imports are functional not shim); `data/ingest/README.md` drops the back-compat-shim paragraph and consolidates to "import from the specific submodule directly".
- Verification at every step: `pytest data/ingest/tests/ -q` showed `138 passed` after the import migration (shim still in place) and `135 passed` after the shim drop (acceptance criterion met). `python simple_wikipedia.py --help` prints unchanged flags. `from simple_wikipedia import main` still resolves (only legitimate consumer is `test_pipeline.py`).

**Net diff:** 11 files changed, 19 insertions, 180 deletions.

## What's next

### Larger queue items (carried forward)

- **Local llama.cpp inference (Phase 1.1).** `LlamaCppBackend` stub at `primer-inference` is the entry point.
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality, not production).
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **Hindi (`hi`) locale pack rollout.** Schema + i18n boundary are already locale-keyed; the open work is the prompt-pack TOML, the seed corpus / wiki layer, and BGE-M3 retrieval validation in Devanagari. Open question: Wikipedia has a Hindi children's-wiki analog called "Bal Vikipedia" (बाल विकिपीडिया) — worth checking if it's API-accessible before defaulting to regular hi.wikipedia.org. The German pattern (Klexikon over de.wikipedia) confirms the heuristic: prefer the children's-vocabulary source whenever one exists. With the submodule layout from PR #60, adding a Hindi source is a `WikiSource` preset in `wiki/source.py` plus (if needed) a new `fetch_strategy` branch in `wiki/fetch.py` — clean per-file change.
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default. The relevant flips: `default = ["embedding"]` in `primer-cli/Cargo.toml` and the `--embedder-backend` CLI default.

### Smaller-scope follow-ups still open

- **German retrieval-quality benchmark.** No equivalent of `tests/retrieval_quality.rs` exists for the German layer yet. Concrete acceptance: hand-author a German equivalent of `tests/common/mod.rs::QUERIES` (~25 child-style German questions targeting specific Klexikon ids), wire a parallel test that runs against the de KB. Higher value now that the corpus is 66 articles deep — 24 was at the cusp where regression-protection cost-benefit was weak. **Suggested target: ~25 BM25-only loose queries + ~12 strict canonical-id mappings; an `--ignored` BGE-M3 hybrid recall test as a follow-up.** This was the originally suggested task before Horst pivoted to the file split — and was offered again this session, but #61 was the cleaner completion of last session's work; still the highest-value smaller item in the queue.
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

- **The pre-existing mypy diagnostic on `import requests` is fixed.** `types-requests>=2.32,<3` added to `data/ingest/requirements.txt`; installed into the existing venv via `uv pip install --python .venv/bin/python -r requirements.txt`. `mypy --python-executable .venv/bin/python simple_wikipedia.py wiki/ retry.py build_whitelist.py` now reports `Success: no issues found in 7 source files`. The fix was bundled into PR #62 per the new "implement quick fixes inline, don't defer" memory.
- **`data/ingest/README.md` setup section now uses `uv` exclusively.** Replaced `python3 -m venv` + `pip install -r` with `uv venv .venv` + `uv pip install --python .venv/bin/python -r requirements.txt`, and added a one-line preface telling future readers that this project is uv-only (per the new memory). The existing venv was preserved (uv installs into it cleanly); only the documented commands changed.
- **Two-commit split was the right granularity for #61.** Commit 1 (test imports) leaves the shim in place and 138 tests green — bisectable as a no-behavior-change checkpoint. Commit 2 (shim drop + delete tripwire + doc updates) is cleanly the "cleanup" commit. Mirrors the per-step reviewability principle from PR #60.
- **The "leaving only `main` + argparse helpers + `__main__`" target lands at 186 lines, not the issue's aspirational ~70.** `main` and the 3 CLI helpers carry meaningful docstrings + body; trying to compress them further would be churn for no reader benefit. The 500-line guideline is the standing test, not "single-digit-percent of original."
- **Issue #61 was clearly defined in PR #60's review comments, with explicit acceptance criteria and per-file import targets.** Following that file-by-file checklist made the work mechanical. Pattern worth reusing for any "follow-up cleanup" PR: write the issue body with concrete acceptance + a per-file checklist, so the next session can execute without re-deriving the plan.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing.)

- **Pure functions in dedicated modules** for algorithmic cores — tested directly without I/O.
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level.
- **Carrier structs with `disabled()` no-op constructors** for parameter bundles that not all callers need to configure.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed.
- **Constants in `consts.rs` submodules** (Rust) or top-of-module `_DEFAULT_*` constants (Python). No magic numbers anywhere.
- **TDD discipline.** Tests first; watch them fail; implement to green. (Not exercised this session — pure mechanical refactor with zero behaviour change. The 138 → 135 tests served as the safety net at every step.)
- **File-size hygiene.** Keep modules under 500 lines. ✅ This session: `simple_wikipedia.py` 242 → 186; submodules unchanged at 163/336/346.
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
- **Plan-then-execute inline for mechanical refactors with a strong test safety net.** Re-confirmed. The 7-test-files migration + shim drop was 2 commits + 13-step todo list — no subagent overhead, just ordered work against pytest as the safety net.
- **Two-commit refactor: "set up the change" then "remove the old".** New this session. Splits a "migrate + delete" diff into a bisectable mid-state and a clean cleanup commit.
- **Ship the follow-up issue body with explicit acceptance + per-file checklist when a PR ships a transitional state.** New this session. Issue #61's body was pre-written during PR #60 review; this session's work was mechanical execution against that checklist.

## Exact commands needed to resume

```bash
# Resume on main (after PR #62 merges):
cd /Users/hherb/src/primer
git status                       # confirm clean
git checkout main
git pull
git log --oneline -10            # the issue-61 merge commit should be near the top

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 605 passed, 0 failed, 2 ignored (as of 2026-05-10).

~/.cargo/bin/cargo clippy --workspace --all-targets
~/.cargo/bin/cargo fmt --all -- --check
```

For the Python ingestion pipeline tests (uv-only — never invoke pip directly):

```bash
cd /Users/hherb/src/primer/data/ingest
# (venv was set up per data/ingest/README.md: `uv venv .venv` +
# `uv pip install --python .venv/bin/python -r requirements.txt`)
.venv/bin/pytest tests/
# Expected: 135 passed.
```

For mypy on the ingest tree (cleanly green after #62):

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
