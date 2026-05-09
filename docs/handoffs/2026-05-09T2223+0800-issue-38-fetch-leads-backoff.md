# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-09 (after issue #38 — `fetch_leads` 429/5xx backoff — landing on PR #57).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **605 Rust tests** under default features (unchanged from prior session — issue #38 was Python-side only). Add `--features primer-kb-load/fastembed` for the embedding-backed sweep + recall test (downloads BGE-M3 ~570 MB on first run; cached afterwards). Plus **131 Python tests** in `data/ingest/` (was 87 — 43 new on `retry.py` + 1 integration test in `test_fetch_lead.py`; pytest from a venv set up per `data/ingest/README.md`).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes.

## Branch status

`feature/issue-38-fetch-leads-backoff` carries 12 commits, pushed and on **PR #57** (open at https://github.com/hherb/primer/pull/57). Built off `origin/main` after the PR #56 (Klexikon URL encode) merge. Once #57 merges, the branch can be deleted from the remote at the user's discretion.

## What we shipped this session

**Closed issue #38: HTTP 429 / 5xx backoff for `data/ingest/simple_wikipedia.py::fetch_leads`.** The pre-Phase-0.3 ingest path called `resp.raise_for_status()` and let any HTTP error abort the whole pipeline — fine for the one-shot Phase 0.2 use case, wasteful if the ingest ever becomes scheduled or much larger. This PR makes the ingest tolerant of transient 429 / 5xx without changing scope of what it actually retries.

**12 commits on `feature/issue-38-fetch-leads-backoff`:**

- `8b724a4` — `docs(spec): backoff for fetch_leads (issue #38)`
- `30968e3` — `docs(plan): fetch_leads backoff implementation plan (issue #38)`
- `8a91136` — `feat(ingest): add retry.py skeleton + consts + RetrySettings + RetryCapExceeded (#38)`
- `3896043` — `feat(ingest): add is_retryable_status pure helper (#38)`
- `6bae2e8` — `feat(ingest): add parse_retry_after pure helper (#38)`
- `6381850` — `feat(ingest): add compute_delay pure helper (#38)`
- `97526b9` — `feat(ingest): add retry_http_get happy-path (#38)`
- `d7bfbad` — `test(ingest): pin retry_http_get retry-path behaviour (#38)`
- `ee76268` — `test(ingest): add headers default to FakeResponse for retry path (#38)`
- `b34da97` — `feat(ingest): wire retry_http_get into all 3 strategy fetchers (#38)`
- `814421a` — `docs(claude.md): record retry_http_get invariant for data/ingest (#38)`
- `61c8056` — `docs(roadmap): close #38 (fetch_leads backoff) + add bullet`

**Concrete deliverables:**

- New module `data/ingest/retry.py` (~180 lines) with the full retry surface:
  - **Constants** (`DEFAULT_MAX_ATTEMPTS=3`, `DEFAULT_BASE_DELAY_S=0.5`, `DEFAULT_BACKOFF_FACTOR=2`, `DEFAULT_JITTER_FRACTION=0.1`, `DEFAULT_RETRY_AFTER_BUDGET_S=30.0`).
  - **`@dataclass(frozen=True) RetrySettings`** with a `.default()` classmethod that mirrors the consts (drift-guard test pins both ends).
  - **`RetryCapExceeded(RuntimeError)`** carrying `attempts` / `last_status` / `retry_after` for developer-facing diagnostics.
  - **Three pure helpers** (`is_retryable_status`, `parse_retry_after`, `compute_delay`) — no I/O, fully unit-tested.
  - **`retry_http_get(http_client, url, *, params, timeout, settings, sleep, jitter_fn)`** — composes the above. `sleep` and `jitter_fn` are kwarg-injected so tests pass deterministic no-ops.
- All three strategy fetchers in `simple_wikipedia.py` route their `http_client.get(...)` through `retry_http_get`:
  - `_fetch_lead_via_text_extracts` (Simple English single-title)
  - `_fetch_leads_via_text_extracts` (Simple English batch of up to 20)
  - `_fetch_lead_via_klexikon` (per-title, Klexikon parse strategy)
- Module-level `_RETRY_SETTINGS = RetrySettings.default()` in `simple_wikipedia.py` is the single source of truth — tune there, not at call sites.
- `FakeResponse` in `tests/test_fetch_lead.py` gained a `headers={}` default (back-compat-additive — no existing test changed shape).
- One new integration test `test_fetch_lead_retries_on_429_then_succeeds` proves the wiring through the public `fetch_lead` API. Uses `monkeypatch.setattr("time.sleep", lambda _: None)` to keep the test fast.
- CLAUDE.md gained one new bullet under the data/ingest section recording the retry-helper invariant.
- ROADMAP.md gained a new ✅ bullet plus inline closure note next to the Phase 0.2 ingest history.
- Python tests grew from 87 → 131 (43 new on `retry.py` + 1 integration on `test_fetch_lead.py`). Rust tests unchanged at 605/0/2. `cargo clippy` + `cargo fmt --check` clean.

**TDD discipline (re-applied):**

- Every behavioural task started with a failing test. RED was confirmed before GREEN for: `RetrySettings`/`RetryCapExceeded`, `is_retryable_status`, `parse_retry_after`, `compute_delay`, `retry_http_get` happy-path, and the integration test in `test_fetch_lead.py`.
- Tasks 6–9 (retry-path regression pins) tested behaviour the Task 5 implementation already covered — they're permanent regression guarantees, not new behaviour, so they pass immediately. Plan explicitly called this out.

**Course corrections during execution:**

- The plan called for one commit per regression-pin test (Tasks 6–9). I consolidated those into a single commit `d7bfbad` since they all pinned the same function with the same shape — per-test commits would have been churn. The work itself was unchanged.
- Plan's "17 passed" expectation after Task 2 was an off-by-one (my counting error). Actual was 18; immaterial.

**Out of scope for this PR (intentional, deferred):**

- Failed-batch persistence sidecar (issue #38's optional bullet).
- Network-error retry (`requests.exceptions.ConnectionError` / `Timeout` propagate unchanged).
- HTTP-date form `Retry-After` parsing (carries the same trade-off as `primer_core::retry`).
- Splitting `simple_wikipedia.py` past 500 lines (it's at ~927 now, already over the guideline; split candidates are listed below; deferred until a third source ships).

## What's next

### Larger queue items (carried forward)

- **Local llama.cpp inference (Phase 1.1).** `LlamaCppBackend` stub at `primer-inference` is the entry point.
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality, not production).
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **Hindi (`hi`) locale pack rollout.** Schema + i18n boundary are already locale-keyed; the open work is the prompt-pack TOML, the seed corpus / wiki layer, and BGE-M3 retrieval validation in Devanagari. Open question: Wikipedia has a Hindi children's-wiki analog called "Bal Vikipedia" (बाल विकिपीडिया) — worth checking if it's API-accessible before defaulting to regular hi.wikipedia.org. The German session pattern (Klexikon over de.wikipedia) confirms the heuristic: prefer the children's-vocabulary source whenever one exists.
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default. The relevant flips: `default = ["embedding"]` in `primer-cli/Cargo.toml` and the `--embedder-backend` CLI default.

### Smaller-scope follow-ups still open

- **Klexikon corpus expansion.** 24 articles is a starter set; the Klexikon corpus is ~3000 articles total. Concrete acceptance: pick 30-50 more children's-curriculum titles (overlap with the English seed cluster categories where possible — heart/lungs/brain analogs are valuable for cross-language evaluation), append to `klexikon_whitelist.txt`, re-run `python3 simple_wikipedia.py --language de`. No code change needed.
- **German retrieval-quality benchmark.** No equivalent of `tests/retrieval_quality.rs` exists for the German layer yet. Acceptance: hand-author a German equivalent of `tests/common/mod.rs::QUERIES` (~20 child-style German questions), wire a parallel test that runs against the de KB. Unlocks regression protection on the German layer + a future German hybrid sweep.
- **Klexikon license claim spot-check.** The site's About page states CC BY-SA 4.0; we labelled all 24 passages with that. Worth a one-time verification that no individual page footers diverge (e.g. older imported content under a different license). Low-priority — the site-wide claim is the canonical one.
- **#45 (partial closure standing)** — 2 paraphrases remain in `KNOWN_FAILING_QUERIES_HYBRID`: "what makes my tummy growl when I am hungry" and "why do flowers smell nice". Cure: a focused English seed-corpus expansion adding "growl" / "growls when empty" to `seed:en:digestion` and "scent" / "fragrant" / "smell" to `wiki-simple:en:plant`. When that lands, remove from the list and use them as a permanent regression guarantee.
- **#46** — explore post-RRF `min_score` as a fifth grid axis in the hybrid sweep.
- **#40** — aggregate per-source attribution row for Wikipedia (and Klexikon).
- **#41** — tighten disambiguation regex if false positives appear.
- **`simple_wikipedia.py` is now 927 lines** (well past the 500-line guideline). Split candidates carried forward from prior sessions, still deferred until a third source comes in: `wiki/source.py` (dataclass + presets), `wiki/strip.py` (wikitext stripper), `wiki/fetch.py` (strategy dispatch + retry). Now also a candidate: factor `retry.py` to live next to that triad if the split lands.
- **Failed-batch persistence sidecar (issue #38 optional).** Was deferred during brainstorming; file as new issue if scheduled ingest ever ships.
- **Network-error retry.** `requests.exceptions.ConnectionError` / `Timeout` still propagate unchanged. If scheduled ingest ships, this becomes the obvious next layer; for one-shot dev runs it's a feature, not a bug.

## Open decisions / risks

Carried-forward open items (still relevant from prior sessions):

- Smoke #6 (Ollama daemon down) deferred verification.
- `is_request()` heuristic conflates config errors with network errors.
- HTTP-date form `Retry-After` silently dropped (now true on both Rust and Python sides).
- Pre-flight auth check at startup deferred.
- Voice-mode bridging hook is data-only.
- `close_session` may drop the final classifier task's `turn_classifications` row on a short conversation.
- Identifier-format divergence across structured-output crates (`llm:{model}` vs `llm:{backend}:{model}`).
- `apply_comprehension` insertion policy (only updates concepts already in `learner.concepts`).
- Concepts are monotonic on `learner_concepts` — saved-once stays saved.
- No cancellation token on streaming-task spawns.
- Background-task spawn order is load-bearing on serialized backends.
- "I just don't want to see this word" escape valve for vocab not yet implemented.
- Tripwire `min top-K` margin is now narrower (1.84x above the 0.5 floor).
- Hybrid does not always win on paraphrases — observation from issue #45.

**New observations from this session (not risks per se):**

- **Plan vs. execution: bundle micro-commits when they're testing the same surface.** The plan called for 4 separate commits across Tasks 6–9 (one per retry-path test); during execution I bundled them because the tests all pinned the same function and the per-test commits added no value over a single thematic commit. The skill ("Don't force through blockers") handles real blockers, but minor commit-granularity decisions during execution are routine and don't need re-planning.
- **Off-by-one in expected test counts is harmless.** My plan's "17 passed" after Task 2 was actually 18 (16 parametrize cases + 2 prior, not 15 + 2). Counts in plans are best-effort guidance, not contracts; what matters is RED before GREEN and zero failures at end-of-task.
- **Kwarg-injection of side-effect functions is the cleanest TDD seam in Python.** `sleep=time.sleep, jitter_fn=random.random` as kwargs (with deterministic test substitutes) made the entire retry surface unit-testable without any `monkeypatch` calls in the pure-helper tests. The integration test does use `monkeypatch.setattr("time.sleep", ...)` but only because plumbing the kwarg through `fetch_lead` would change the public API for one test. Reuse this pattern for any future Python helper that has a real-world side effect.
- **A test that pins behaviour-already-implemented is still TDD-valuable.** Tasks 6–9 (the four retry-path regression pins) didn't drive new code — they pin behaviour the Task 5 implementation already produced. They protect against future "simplify the retry loop" changes that would inadvertently drop a sub-behaviour. Don't skip these even when the impl already passes.
- **The default `time.sleep(0)` is a no-op.** Useful for keeping integration tests fast without monkeypatching when `Retry-After: 0` covers the test path. Saved one monkeypatch call this session.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing.)

- **Pure functions in dedicated modules** for algorithmic cores — tested directly without I/O. `retry.py`'s three pure helpers are a textbook example: each has a closed input domain and a deterministic output; all three are exhaustively unit-tested with pytest parametrize.
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level.
- **Carrier structs with `disabled()` no-op constructors** for parameter bundles that not all callers need to configure.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed.
- **Constants in `consts.rs` submodules** (Rust) or top-of-module `_DEFAULT_*` constants (Python). No magic numbers anywhere.
- **TDD discipline.** Tests first; watch them fail; implement to green. Re-applied for every new behaviour this session.
- **File-size hygiene.** Keep modules under 500 lines. `simple_wikipedia.py` is now 927 lines (~+30 from the retry-wiring); split candidates listed above. Defer until a third source comes in.
- **Network-injection test seam** for any data-ingest pipeline (`http_client` parameter, substitute `FakeHttpClient`/`KlexikonFakeHttpClient` in tests).
- **Subagent-driven development workflow** for plan execution (or inline executing-plans for small TDD-shaped plans).
- **Defensive sanity tests at the data layer.** Confirmed standing.
- **Always pin `Default` impls of public structs to `consts::*`, with a drift-prevention test.** Mirrored on the Python side as `RetrySettings.default()` ↔ module-level `DEFAULT_*` consts.
- **Pure helpers + their own unit tests, even in `#[ignore]`'d integration tests.**
- **Frozen dataclasses for process-wide configuration** (the `WikiSource` pattern; `RetrySettings` follows the same shape).
- **String discriminators for strategy selection, with allow-list validation.**
- **Re-run the live ingest after changing any fetch-path helper.** Tiny expected diff = success, anything bigger = a flag worth investigating. (Skipped this session because the change was retry-only and would only manifest under upstream-failure conditions; skip is justified when the change is observable only via a code path we can't trigger from a working ingest.)
- **Kwarg-injected side-effect functions for TDD seams in Python.** New pattern this session — see "New observations" above.

## Exact commands needed to resume

```bash
# Resume on main (after PR #57 merges):
cd /Users/hherb/src/primer
git status                       # confirm clean
git checkout main
git pull
git log --oneline -10            # the issue-38 merge commit should be near the top

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 605 passed, 0 failed, 2 ignored (as of 2026-05-09).

~/.cargo/bin/cargo clippy --workspace --all-targets
~/.cargo/bin/cargo fmt --all -- --check
```

For the Python ingestion pipeline tests:

```bash
cd /Users/hherb/src/primer/data/ingest
# (venv was set up per data/ingest/README.md)
.venv/bin/pytest tests/
# Expected: 131 passed.
```

For real-LLM smoke testing (Anthropic):

```bash
cd /Users/hherb/src/primer/src
RUST_LOG=debug ~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name SmokeTester --age 9 --no-persist --verbose 2>&1 | tee /tmp/smoke.log
```

For real-LLM smoke testing in German:

```bash
cd /Users/hherb/src/primer/src
RUST_LOG=debug ~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name Lukas --age 9 --language de --no-persist --verbose 2>&1 | tee /tmp/smoke_de.log
# Expected: KB auto-loads 24 Klexikon passages on locale=de.
```

For re-running the Klexikon ingest (rare; only when the whitelist changes or articles drift):

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/python simple_wikipedia.py --language de
# Writes ../seed/wiki_passages.de.jsonl. Commit any diff.
# Live HTTP traffic to klexikon.zum.de; ~24 sequential requests with 1s pacing
# (per-page parse strategy = no batching; takes ~30s on a warm network).
# 429/5xx now retried 3× with backoff (issue #38).
```

For re-running the Simple English Wikipedia ingest:

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/python simple_wikipedia.py --language en
# Writes ../seed/wiki_passages.en.jsonl. Commit any diff.
# Live HTTP traffic to simple.wikipedia.org; ~2 batched requests of 20 titles.
# 429/5xx now retried 3× with backoff (issue #38).
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
- If you discover that Horst did interim work that changes the plan, flag it.
