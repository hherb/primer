# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-06 (after PR #36 merged: Phase 0.2 MVP — Simple English Wikipedia ingestion).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **550 Rust tests** across the workspace plus **43 Python tests** in `data/ingest/` (pytest from a venv set up per `data/ingest/README.md`).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes.

## Branch status

`main`, clean working tree. PR #36 merged at commit `44a70b8` (Phase 0.2 MVP — Simple English Wikipedia ingestion); the post-review cleanup commit `6435960` was folded into the same merge. The `feature/wikipedia-ingestion` branch can be deleted from the remote at the user's discretion.

## What we shipped this session

Phase 0.2 MVP — Wikipedia ingestion + retrieval expansion + post-review cleanups.

**New code (Python):**

- `data/ingest/simple_wikipedia.py` — pure-functional Python pipeline that ingests Simple English Wikipedia Science articles via the MediaWiki extracts API. Lead-only chunking; batched 20-titles-per-request fetch (1-2 API calls instead of N); disambiguation-page detection; `_strip_math_artifacts` sanitiser for MathJax fallback blocks; `_assert_unique_slugs` guards against post-slugify collisions before any HTTP traffic.
- `data/ingest/build_whitelist.py` — one-time helper that scrapes Wikipedia Vital Articles Level 4 sub-lists (follows redirects; aggregates from Physical sciences + Biological/health sciences) and prints candidate titles for hand-curation.
- `data/ingest/simple_wikipedia_whitelist.txt` — 34-entry curated list seeded from `build_whitelist.py` and trimmed by topic relevance to the children's-curriculum audience.
- `data/ingest/tests/` — 43 pytest cases covering slugify, the whitelist parser, `to_passage`, `fetch_lead`/`fetch_leads` (with injected fake HTTP client), `build_whitelist`, and the byte-exact end-to-end pipeline.

**New data:**

- `data/seed/wiki_passages.en.jsonl` — 35 lead passages (CC-BY-SA-3.0; per-passage `source_url` carrying the canonical Wikipedia URL). Auto-loads on a fresh KB alongside the existing 55-passage CC0 hand-drafted seed corpus, giving 90 passages total in the Phase 0.2 MVP corpus.

**New code (Rust):**

- `primer-kb-load::auto_seed_if_empty` was extended to load **all** `*.<pack>.jsonl` files in the seed dir (not just `seed_passages.<pack>.jsonl`). This is what wires the wiki layer through. New `discover_seed_files(locale) -> Vec<PathBuf>` is the general API; the existing `discover_seed_jsonl` is preserved as a backward-compat shim.
- The retrieval-quality integration test grew 8 wiki-targeted canonical queries (gravity, atom, virus, climate change, DNA, vaccines, friction, plants — concepts the seed corpus does not cover) and now switches its load step from `load_jsonl(seed_path())` to `auto_seed_if_empty` so attribution validation also exercises the wiki layer.

**Post-review cleanups (commit `6435960`):**

- `simple_wikipedia.py` mid-file imports hoisted to the top; `fetch_leads` switched to a single-batch contract (raises `ValueError` on > 20 titles, returns empty for empty input); slug-collision detection runs before any HTTP traffic; the attribution test now asserts both `CC0-1.0` and `CC-BY-SA-3.0` licences are present.

**Documentation:**

- `README.md`, `ROADMAP.md`, `CLAUDE.md` updated to reflect Phase 0.2 (MVP) done. Spec at [docs/superpowers/specs/2026-05-06-wikipedia-ingestion-design.md](docs/superpowers/specs/2026-05-06-wikipedia-ingestion-design.md). Plan at [docs/superpowers/plans/2026-05-06-wikipedia-ingestion.md](docs/superpowers/plans/2026-05-06-wikipedia-ingestion.md).

**Test counts after merge:** 550 Rust (no net new — the existing retrieval-quality test absorbed the wiki-layer assertions) + 43 Python (37 in PR + 6 added in the post-review cleanup: 4 slug-collision + 2 fetch_leads contract).

## What's next — Phase 0.2 final task + deferred follow-ups

### The remaining Phase 0.2 task: tune `RetrievalParams` / `HybridParams` defaults

This was gated on having a corpus broad enough to evaluate against; the wiki layer plus the hand-drafted seed now gives 90 passages spanning every cluster, which is the floor.

Concrete acceptance criteria:

1. Add benchmark queries that span the wider corpus (children's questions where the right answer is a wiki article rather than a hand-drafted seed passage). Many already exist in `primer-kb-load/tests/retrieval_quality.rs`; adding more targeted sweeps is welcome.
2. Sweep `top_k`, `min_score`, and `rrf_k`. Industry default for `rrf_k` is 60 (lower weighs the very top of each ranked list more, higher flattens). Spec at `docs/superpowers/specs/2026-05-05-hybrid-retrieval-design.md`.
3. Land the chosen defaults; update `primer_core::consts::retrieval` accordingly.
4. Optional but valuable: add a hybrid-leg version of the canonical-query test (filed as #39).

### Deferred follow-ups from PR #36 review (filed as GitHub issues)

These are real-but-not-blocking improvements to the ingest pipeline and the test surface:

- **#37** — replace placeholder User-Agent contact identifier (`PrimerSeedBuilder/0.1 (contact: see-repo-readme)`) with a real contact string before any further live runs.
- **#38** — add 429 / 5xx backoff (with `Retry-After` honouring) to `fetch_leads`. The Rust side already has `primer_core::retry::retry_with_backoff` as a conceptual template; the Python pipeline currently aborts on first non-2xx, which is fine for one-time runs but won't scale to scheduled or wider ingests.
- **#39** — add a hybrid-retrieval (BM25 + vector RRF) version of the canonical-query test. The current test is BM25-only; a hybrid leg using `StubEmbedder` (no model download in CI) plus an opt-in real-`fastembed` variant would surface vector-leg regressions.
- **#40** — aggregate per-source attribution. Every wiki passage today has its own `sources` row; an umbrella "Simple English Wikipedia" parent source would make UI-side attribution simpler. Not urgent for 35 passages; matters at scale.
- **#41** — consider scoping the disambiguation regex more tightly if false positives appear. Today's `\b(can mean|may refer to|...)\b` over `head[:300]` works for the curated 34-entry set; a wider whitelist might trip it on a real article.

### Larger things still in the queue (carried forward from prior sessions)

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

New in this session: items now tracked as #37, #38, #39, #40, #41 above. None of them are blocking the next Phase 0.2 task.

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
- **Network-injection test seam.** New in this session for the Python pipeline: `http_client` is a parameter on every fetch helper, so the unit tests substitute a `FakeHttpClient` and never touch the real API. Use the same shape if you add another data-ingest pipeline.

## Exact commands needed to resume

```bash
# Resume on main:
cd /Users/hherb/src/primer
git status                       # confirm clean
git log --oneline -10            # recent activity (44a70b8 should be at the top of the Phase 0.2 work)

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 550 passed, 0 failed (as of 2026-05-06).

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
# 9 unit tests + 2 integration tests should pass; the canonical-queries
# test exercises 58+ queries across all six layers (5 hand-drafted clusters
# + the wiki layer).
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
