# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-09 (after German Klexikon ingest landing).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **605 Rust tests** under default features (unchanged from prior session — German Klexikon work was Python-side only). Add `--features primer-kb-load/fastembed` for the embedding-backed sweep + recall test (downloads BGE-M3 ~570 MB on first run; cached afterwards). Plus **82 Python tests** in `data/ingest/` (was 43 — pytest from a venv set up per `data/ingest/README.md`).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes.

## Branch status

`feature/german-klexikon-ingest` carries 1 commit (work) + 1 commit (handoff) ready to land via PR (to be opened against `main`). Built off the latest `origin/main` (`522455d`, the merge of PR #53 / paraphrase re-add). Once merged, the branch can be deleted from the remote at the user's discretion.

## What we shipped this session

**Generalised the Phase 0.2 Wikipedia ingest pipeline around a frozen `WikiSource` dataclass; added a German Klexikon layer that auto-loads on `--language de`.** Closes the German half of the locale-rollout item.

The session began against the brief's "German (`de`) locale pack rollout" follow-up. Initial implementation targeted regular `de.wikipedia.org`, but mid-session the user pointed out [Klexikon](https://klexikon.zum.de/) — a hand-written German children's wiki for ages 8-13. Klexikon is the right vocabulary level for the Primer's audience the way Simple English is for English (regular de-Wikipedia is dense adult prose). We pivoted, expanding the scope to add a second fetch strategy because Klexikon's MediaWiki has no TextExtracts extension.

**Two commits on `feature/german-klexikon-ingest`:**

- `8eef82e` — `feat(ingest): add German Klexikon layer + WikiSource refactor`
- (handoff commit follows)

**Concrete deliverables:**

- New frozen `WikiSource` dataclass at `data/ingest/simple_wikipedia.py` bundling per-source ingest parameters: `pack_id`, `api_url`, `web_base_url`, `id_prefix`, `human_label`, `license`, `topic_tags`, `disambiguation_patterns`, `fetch_strategy`, `batch_size`. `__post_init__` validates `fetch_strategy` against an allow-list and rejects `batch_size < 1`. Two presets ship: `SIMPLE_ENGLISH` (Phase 0.2 path; preserved byte-exact in the existing test fixtures) and `KLEXIKON`.
- New `klexikon_wikitext` fetch strategy: `action=parse&prop=wikitext&section=0` (one HTTP call per article — no batching since the parse API takes one `page` per call) → raw wikitext → `strip_klexikon_wikitext` → plain text. The `text_extracts` strategy preserves the existing batched-20 path.
- New pure function `strip_klexikon_wikitext` and supporting helpers (`_strip_balanced_drop_blocks`, `_klexikon_canonical_url`). The stripper handles: HTML comments, `<ref>`/`<ref/>` tags, `<gallery>` blocks (with attribute support), drop-blocks `[[Datei:...]] [[File:...]] [[Bild:...]] [[Image:...]] [[Kategorie:...]] [[Category:...]]` via balanced-bracket scanning (image captions can contain nested links), plain wikilinks `[[a]]` / `[[a|b]]`, templates `{{...}}`, bold `'''x'''` and italic `''x''`, plus inline-whitespace and blank-line cleanup.
- `slugify` upgraded from `str.lower()` to `str.casefold()`. Critical for German: `lower()` left `ß` un-decomposed (NFD has no decomposition), and the ASCII-strip then removed it as a non-alphanumeric — silently corrupting `Straße` to `strae`. casefold folds ß → ss before NFD. Diacritics still NFD-decompose to base letter (ä→a, ö→o, ü→u, é→e). All English slugs unchanged (casefold == lower for ASCII).
- New `klexikon_whitelist.txt` with 24 children's-curriculum entries spanning the same five clusters as the English seed (space, body, how-things-work, life, earth/weather).
- New `data/seed/wiki_passages.de.jsonl` (24 passages, byte-exact reproducible — sorted by id, `ensure_ascii=True`). Auto-loads end-to-end on `--language de`; verified via stub-backend smoke test (`PRIMER_SEED_DIR=...path... cargo run -- --backend stub --language de --no-persist`).
- Refactored CLI: `__main__` now takes `--language en|de` (defaults `en` for back-compat); `--whitelist` and `--output` overrides honoured. The English path is unchanged byte-for-byte from the existing `wiki_passages.en.jsonl`.
- Tests grew **43 → 82** (39 new):
  - `tests/test_wiki_source.py` (new file, 7 tests) — pin every preset shape + frozen-dataclass invariant + `__post_init__` validation.
  - `tests/test_wikitext_strip.py` (new file, 18 tests) — 18 idioms covering links, image blocks, gallery blocks, German + English categories, templates, references, comments, bold/italic, whitespace, and a smoke test against a real Klexikon "Klima" page wikitext.
  - `tests/test_slugify.py` — 5 new tests for ß folding, capital ẞ, umlauts, compound words, and post-fold collisions.
  - `tests/test_passage_emit.py` — 2 new tests for the Klexikon source emit path.
  - `tests/test_fetch_lead.py` — 5 new tests for Klexikon URL routing, image-block stripping inside the fetch path, missing-page error handling, and German-style disambiguation detection.
  - `tests/test_pipeline.py` — 2 new tests for end-to-end Klexikon flow (via a per-strategy `KlexikonFakeHttpClient`) and request-routing assertions.
- Rust workspace tests unchanged at **605 / 0 / 2** (no Rust code touched).
- README.md, ROADMAP.md, CLAUDE.md, and `data/ingest/README.md` all updated to document the new shape.

**TDD discipline (re-applied throughout):**

- RED first for every behaviour: wrote tests against `WikiSource`, `KLEXIKON`, `strip_klexikon_wikitext`, the parse-strategy fetch path, and the pipeline-with-Klexikon-source — all before the implementation existed. Confirmed all 4 collection errors / 4 assertion failures, then implemented to GREEN.
- Mid-implementation, the live ingest exposed two markup classes (`<gallery>` blocks, `[[Kategorie:...]]` links) that the initial stripper missed. RED test for each → fix → GREEN. The wikitext-strip test file is now an explicit checklist of every markup idiom we've seen in real Klexikon articles.

## What's next

### Larger queue items (carried forward)

- **Local llama.cpp inference (Phase 1.1).** `LlamaCppBackend` stub at `primer-inference` is the entry point.
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality, not production).
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **Hindi (`hi`) locale pack rollout.** Schema + i18n boundary are already locale-keyed; the open work is the prompt-pack TOML, the seed corpus / wiki layer, and BGE-M3 retrieval validation in Devanagari. Open question: Wikipedia has a Hindi children's-wiki analog called "Bal Vikipedia" (बाल विकिपीडिया) — worth checking if it's API-accessible before defaulting to regular hi.wikipedia.org. The German session pattern (Klexikon over de.wikipedia) suggests the right move: prefer the children's-vocabulary source whenever one exists.
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default. The relevant flips: `default = ["embedding"]` in `primer-cli/Cargo.toml` and the `--embedder-backend` CLI default.

### Smaller-scope follow-ups still open

- **Klexikon corpus expansion.** 24 articles is a starter set; the Klexikon corpus is ~3000 articles total. Concrete acceptance: pick 30-50 more children's-curriculum titles (overlap with the English seed cluster categories where possible — heart/lungs/brain analogs are valuable for cross-language evaluation), append to `klexikon_whitelist.txt`, re-run `python3 simple_wikipedia.py --language de`. No code change needed.
- **German retrieval-quality benchmark.** No equivalent of `tests/retrieval_quality.rs` exists for the German layer yet. Acceptance: hand-author a German equivalent of `tests/common/mod.rs::QUERIES` (~20 child-style German questions), wire a parallel test that runs against the de KB. Unlocks regression protection on the German layer + a future German hybrid sweep.
- **Klexikon license claim spot-check.** The site's About page states CC BY-SA 4.0; we labelled all 24 passages with that. Worth a one-time verification that no individual page footers diverge (e.g. older imported content under a different license). Low-priority — the site-wide claim is the canonical one.
- **#45 (partial closure standing)** — 2 paraphrases remain in `KNOWN_FAILING_QUERIES_HYBRID`: "what makes my tummy growl when I am hungry" and "why do flowers smell nice". Cure: a focused English seed-corpus expansion adding "growl" / "growls when empty" to `seed:en:digestion` and "scent" / "fragrant" / "smell" to `wiki-simple:en:plant`. When that lands, remove from the list and use them as a permanent regression guarantee.
- **#46** — explore post-RRF `min_score` as a fifth grid axis in the hybrid sweep.
- **#38** — add 429/5xx backoff to `fetch_leads` (Python ingestion). Now also relevant to the per-page Klexikon strategy. Not blocking; only one-time runs need it today.
- **#40** — aggregate per-source attribution row for Wikipedia (and now Klexikon).
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

- **Mid-task user steering can change the right answer.** The session began on regular de-Wikipedia and would have produced functional but pedagogically wrong content. Horst's mid-stream Klexikon note redirected the work to a much better source. Lesson: explicitly surface "how should we proceed?" forks when the path is non-obvious, especially around content sources.
- **External APIs vary in capability.** TextExtracts is one of MediaWiki's most useful extensions but it's NOT installed everywhere — Klexikon doesn't have it. Future locale rollouts should probe API capabilities (`prop=siteinfo&siprop=extensions`) before assuming a fetch strategy.
- **Wikitext-stripping is a moving target.** Each new wiki dialect tends to introduce one or two markup idioms our stripper doesn't yet handle. The stripper tests serve double duty — they're a regression guard AND a checklist of "every markup idiom we've ever seen in real ingested content." Future contributors adding a new source should expect to add 1-3 new test cases the first time they ingest from a wiki they haven't touched before.
- **The `WikiSource` dataclass is the extension point for future sources.** Adding (e.g.) a hypothetical Hindi children's wiki is purely additive: declare a `HINDI` preset, hand-author a whitelist, run the pipeline. The fetch-strategy field is open-vocabulary in shape (just a string discriminator) but allow-listed via `__post_init__` validation, so adding a new strategy is a 3-line change (string + dispatch arm + helper).

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing.)

- **Pure functions in dedicated modules** for algorithmic cores — tested directly without I/O. New examples this session: `slugify`, `_assert_unique_slugs`, `strip_klexikon_wikitext`, `_strip_balanced_drop_blocks`, `_klexikon_canonical_url`, `_check_disambiguation`. All take simple inputs, produce simple outputs, raise on bad input.
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level.
- **Carrier structs with `disabled()` no-op constructors** for parameter bundles that not all callers need to configure.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed.
- **Constants in `consts.rs` submodules.** No magic numbers anywhere.
- **TDD discipline.** Tests first; watch them fail; implement to green. Re-applied for every new behaviour this session, including the post-ingest discovery of `<gallery>` and `Kategorie:` leaks.
- **File-size hygiene.** Keep modules under 500 lines. The refactored `simple_wikipedia.py` is now 770 lines (was 360); future split candidates: a `wiki/source.py` with the dataclass + presets, `wiki/strip.py` with the wikitext stripper, `wiki/fetch.py` with the strategy dispatch. Defer until a third source comes in.
- **Network-injection test seam** for any data-ingest pipeline (`http_client` parameter, substitute `FakeHttpClient`/`KlexikonFakeHttpClient` in tests). The new Klexikon path uses a different fake than the existing one because the `parse` action's request shape differs — keep them distinct rather than overloading one fake.
- **Subagent-driven development workflow** for plan execution.
- **Defensive sanity tests at the data layer.** When test data has multiple consumers, put cheap sanity tests in the shared `tests/common/mod.rs` that validate invariants. Confirmed standing.
- **Always pin `Default` impls of public structs to `consts::*`, with a drift-prevention test.**
- **Pure helpers + their own unit tests, even in `#[ignore]`'d integration tests.**
- **Frozen dataclasses for process-wide configuration.** New pattern this session, applied to `WikiSource`. Mutating a config object mid-run silently changes every record produced thereafter — frozen prevents that class of bug at runtime, and `__post_init__` validates the field invariants near the typo.
- **String discriminators for strategy selection, with allow-list validation.** New pattern this session, applied to `fetch_strategy`. Cleaner than a function-pointer field (still hashable / comparable / pickleable / inspectable in a debugger) and the validation gives loud errors on typos.

## Exact commands needed to resume

```bash
# Resume on main (after the german-klexikon PR merges):
cd /Users/hherb/src/primer
git status                       # confirm clean
git checkout main
git pull
git log --oneline -10            # the german-klexikon merge commit should be near the top

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
# Expected: 82 passed.
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
