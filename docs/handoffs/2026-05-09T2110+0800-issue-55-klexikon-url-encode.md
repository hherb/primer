# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-09 (after issue #55 — Klexikon URL percent-encoding — landing).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **605 Rust tests** under default features (unchanged from prior session — the URL-encoding fix was Python-side only). Add `--features primer-kb-load/fastembed` for the embedding-backed sweep + recall test (downloads BGE-M3 ~570 MB on first run; cached afterwards). Plus **87 Python tests** in `data/ingest/` (was 82 — 5 new on `_klexikon_canonical_url`; pytest from a venv set up per `data/ingest/README.md`).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes.

## Branch status

`feature/issue-55-klexikon-url-encode` carries 1 commit, pushed and on **PR #56** (open). Built off `origin/main` after the German Klexikon merge (`33e7ba0`). Once merged, the branch can be deleted from the remote at the user's discretion.

## What we shipped this session

**Closed issue #55: `_klexikon_canonical_url` now percent-encodes non-ASCII titles per RFC 3986.** Surfaced during the PR #54 review; the concern was the stored `source_url` field that flows into the `sources` attribution table and any future attribution UI. Browsers handle either form, so retrieval was never affected; this was about getting the canonical shape right for the long-term form.

**One commit on `feature/issue-55-klexikon-url-encode`:**

- `fe1c8c7` — `fix(ingest): percent-encode non-ASCII titles in Klexikon canonical URLs (#55)`

**Concrete deliverables:**

- `_klexikon_canonical_url` switched from `source.web_base_url + title.replace(" ", "_")` to `source.web_base_url + urllib.parse.quote(title.replace(" ", "_"), safe="/:")`. The `safe="/:"` keeps namespace separators unencoded (e.g. `Datei:Foo`); `_` is in the unreserved set so `quote` leaves it alone.
- 5 new unit tests on `_klexikon_canonical_url` at `data/ingest/tests/test_fetch_lead.py`:
  - `test_klexikon_canonical_url_percent_encodes_diacritics` (Vögel → V%C3%B6gel)
  - `test_klexikon_canonical_url_percent_encodes_eszett` (Größe → Gr%C3%B6%C3%9Fe)
  - `test_klexikon_canonical_url_replaces_spaces_with_underscores_first` (Roter Riese → Roter_Riese)
  - `test_klexikon_canonical_url_underscore_is_preserved_unencoded` (Schwarzes Loch → Schwarzes_Loch — `_` not encoded)
  - `test_klexikon_canonical_url_ascii_titles_unchanged` (Klima → Klima — round-trip)
  - The 2 non-ASCII tests were watched failing RED before implementing.
- `data/seed/wiki_passages.de.jsonl` regenerated via live Klexikon ingest. Diff is exactly **one line** — the `Vögel` entry's `source_url` (`/wiki/Vögel` → `/wiki/V%C3%B6gel`); the other 23 passages are byte-identical, confirming the deterministic-output invariant from CLAUDE.md.
- CLAUDE.md gained a one-line invariant bullet right next to the `slugify` casefold one, so future readers see the URL-encoding contract alongside the related slugify gotcha.
- Python tests grew **82 → 87** (5 new). Rust tests unchanged at **605 / 0 / 2** (no Rust code touched). `cargo clippy` + `cargo fmt --check` clean.

**TDD discipline (re-applied):**

- RED first: wrote 5 unit tests asserting the new percent-encoded shape AND the unchanged-shape invariants for ASCII titles. Confirmed 2 failures (the diacritic + ß cases) before implementing. After GREEN, ran the full Python suite to confirm nothing else broke.
- Note: no existing tests asserted on a literal-Unicode URL form, so no test edits were required for the existing suite. The only test file that passed a literal-Unicode `canonical_url` (`test_passage_emit.py:147`) does so as a `to_passage` input fixture, not as a behaviour assertion.

**Branch hygiene aside:**

The previous brief mentioned an orphan commit `8304df4` on the merged Klexikon branch (`test(ingest): drop unused page_url parameter`). When I tried to cherry-pick it onto the new branch, the cherry-pick was empty — the squash merge of PR #54 had already absorbed the same change. So that commit is effectively redundant and the local `feature/german-klexikon-ingest` branch can be deleted at the user's discretion.

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
- **#38** — add 429/5xx backoff to `fetch_leads` (Python ingestion). Now also relevant to the per-page Klexikon strategy. Not blocking; only one-time runs need it today.
- **#40** — aggregate per-source attribution row for Wikipedia (and Klexikon).
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
- Tripwire `min top-K` margin is now narrower (1.84x above the 0.5 floor).
- Hybrid does not always win on paraphrases — observation from issue #45.

**New observations from this session (not risks per se):**

- **`urllib.parse.quote` defaults are usually right; the `safe` parameter is the lever.** The default `safe='/'` keeps slashes unencoded — perfect for full URL paths but slightly aggressive for namespace separators (`:`). Using `safe="/:"` is the common pattern for MediaWiki-shaped URLs where namespace prefixes (`Datei:`, `Kategorie:`, `Template:`) appear in canonical paths.
- **Re-ingestion as smoke test.** When changing a fetch-path helper, re-running the live ingest end-to-end is a cheap full-stack confirmation: the change either produces a tiny expected diff or no diff. A larger-than-expected diff would have flagged either an upstream content change OR an unintended side effect of the helper change. In this session it produced exactly the expected one-line diff. Reuse this pattern when changing any fetch/transform helper.
- **PR-review issues are the cheapest follow-ups to land.** Issue #55 was filed during the review of PR #54 with explicit acceptance criteria; turning that into a 4-file ~80-line PR took under an hour because the work was well-scoped and the test design was already implicit in the issue body. Encourage future reviewers (Claude or human) to file PR-comment-style issues with explicit acceptance criteria — they convert directly into well-shaped follow-up PRs.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing.)

- **Pure functions in dedicated modules** for algorithmic cores — tested directly without I/O. `_klexikon_canonical_url` is a textbook example: pure (`(WikiSource, str) → str`), no I/O, easy to test exhaustively with 5 unit tests covering the full input space we care about.
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level.
- **Carrier structs with `disabled()` no-op constructors** for parameter bundles that not all callers need to configure.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed.
- **Constants in `consts.rs` submodules.** No magic numbers anywhere.
- **TDD discipline.** Tests first; watch them fail; implement to green. Re-applied for every new behaviour this session — only the 2 tests asserting the *new* behaviour failed RED; the 3 invariant tests passed immediately because they were pinning behaviour the existing code already produced (good — those tests defend against future regressions, not the current change).
- **File-size hygiene.** Keep modules under 500 lines. `simple_wikipedia.py` is now 778 lines (+8 from the URL-encoding helper docstring + import); future split candidates: `wiki/source.py` (dataclass + presets), `wiki/strip.py` (wikitext stripper), `wiki/fetch.py` (strategy dispatch). Defer until a third source comes in.
- **Network-injection test seam** for any data-ingest pipeline (`http_client` parameter, substitute `FakeHttpClient`/`KlexikonFakeHttpClient` in tests).
- **Subagent-driven development workflow** for plan execution.
- **Defensive sanity tests at the data layer.** Confirmed standing.
- **Always pin `Default` impls of public structs to `consts::*`, with a drift-prevention test.**
- **Pure helpers + their own unit tests, even in `#[ignore]`'d integration tests.**
- **Frozen dataclasses for process-wide configuration** (the `WikiSource` pattern).
- **String discriminators for strategy selection, with allow-list validation.**
- **Re-run the live ingest after changing any fetch-path helper.** New pattern this session — a tiny expected diff = success, anything bigger = a flag worth investigating.

## Exact commands needed to resume

```bash
# Resume on main (after PR #56 merges):
cd /Users/hherb/src/primer
git status                       # confirm clean
git checkout main
git pull
git log --oneline -10            # the issue-55 merge commit should be near the top

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
# Expected: 87 passed.
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
```

For re-running the Simple English Wikipedia ingest:

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/python simple_wikipedia.py --language en
# Writes ../seed/wiki_passages.en.jsonl. Commit any diff.
# Live HTTP traffic to simple.wikipedia.org; ~2 batched requests of 20 titles.
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
