# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-10 (after issue #45 — full closure via seed-corpus expansion — landing on PR #58).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **605 Rust tests** under default features (unchanged from prior session — issue #45 partial closure is corpus-only). Add `--features primer-kb-load/fastembed` for the embedding-backed sweep + recall test (downloads BGE-M3 ~570 MB on first run; cached afterwards). Plus **131 Python tests** in `data/ingest/` (unchanged this session; pytest from a venv set up per `data/ingest/README.md`).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes.

## Branch status

`feature/issue-45-paraphrase-corpus-expansion` carries 2 commits, pushed and on **PR #58** (open at https://github.com/hherb/primer/pull/58). Built off `origin/main` after the PR #57 (issue #38 backoff) merge. Once #58 merges, the branch can be deleted from the remote at the user's discretion.

## What we shipped this session

**Closed issue #45 in full.** The hybrid retrieval test had 2 paraphrase queries skipped via `KNOWN_FAILING_QUERIES_HYBRID` after the initial #45 partial closure: `"what makes my tummy growl when I am hungry"` → `seed:en:digestion` (no growl vocabulary in the passage) and `"why do flowers smell nice"` → `wiki-simple:en:plant` (the wiki passage discusses photosynthesis and plant anatomy, not scent). Both are now closed via a focused seed-corpus expansion.

**2 commits on `feature/issue-45-paraphrase-corpus-expansion`:**

- `9cec0df` — `feat(corpus): close #45 — add seed:en:flowers + stomach-growl sentence`
- `7c6d017` — `docs: record issue #45 full closure (CLAUDE/README/ROADMAP)`

**Concrete deliverables:**

- **`seed:en:digestion`** in `data/seed/seed_passages.en.jsonl` gains a paragraph naming the growling/rumbling symptom in child-friendly terms ("Have you ever heard your tummy growl when you are hungry? When the stomach and small intestine are mostly empty, the muscles in their walls keep squeezing anyway, pushing gas and a little leftover liquid through. That mixing makes the gurgling, growling, rumbling sound you hear — your gut is just keeping busy, waiting for the next meal."). Topics list adds `hunger`, `growling`.
- **New `seed:en:flowers`** passage in `data/seed/seed_passages.en.jsonl` (~360 words, hand-drafted CC0): pollination, petals, scent, fragrant, nectar, pollinator attraction. Inserted after `seed:en:seeds-germination` in the life cluster. Topics: `life`, `flowers`, `pollination`, `scent`, `reproduction`, `plants`.
- **Benchmark canonical re-target** in `src/crates/primer-kb-load/tests/common/mod.rs`: `"why do flowers smell nice"` now points at `seed:en:flowers` (cluster `Cluster::Life`, required terms `["flower", "scent"]`). The previous `wiki-simple:en:plant` target was about photosynthesis/anatomy, not scent. Inline rationale comment added.
- **`KNOWN_FAILING_QUERIES`** loses the 2 newly-passing entries (still keeps "what is inside a tiny bug" and "why does the brain need oxygen from the lungs" as the BM25-vs-hybrid-lift demonstration the brief calls out).
- **`KNOWN_FAILING_QUERIES_HYBRID`** is now an empty list with an explanatory comment. New entries there should be investigated before adding — they represent either a real semantic regression or a corpus-coverage gap the dense leg cannot bridge.
- CLAUDE.md, README.md, ROADMAP.md updated with new corpus counts (55 → 56 hand-drafted, 90 → 91 total English), new hybrid recall claim ("100% loose / 100% strict on all 91 queries / 24 mappings; `KNOWN_FAILING_QUERIES_HYBRID` is empty"), and new tripwire numbers (worst-case top-1 was 2.11, now 2.06; min top-K was 0.92, now 0.95; median 4.20 → 4.25). New ✅ ROADMAP bullet records the partial-closure event.

**Verified:**

- `cargo test -p primer-kb-load --test retrieval_quality` → 7/7 pass.
- `cargo test -p primer-kb-load --test retrieval_quality_hybrid` → 6/6 pass (structural `StubEmbedder`).
- `cargo test -p primer-kb-load --features fastembed --test retrieval_quality_hybrid hybrid_retrieval_recall_with_fastembed -- --nocapture` → 1/1 pass (real BGE-M3, 100% loose / 100% strict on all 91 queries / 24 mappings; ran in 42.5 s including model load).
- `cargo test -p primer-kb-load --test bm25_floor_tripwire -- --ignored --nocapture` → min top-K 0.95, median 4.25, worst top-1 2.06; PASS at the 0.5 floor.
- `cargo test --workspace` → 605 passed, 0 failed, 2 ignored. Unchanged from prior session.
- `cargo clippy --workspace --all-targets`, `cargo fmt --all -- --check` → clean.

**TDD discipline (re-applied):**

- Followed RED → GREEN per the standard cycle: removed the 2 queries from `KNOWN_FAILING_QUERIES` first (no corpus changes yet) → BM25-only test failed loose+strict on both, as expected → added the corpus content + canonical re-target → all 7 BM25-only tests pass. Then verified hybrid recall against real BGE-M3.
- The "RED → GREEN" discipline saved ~30 seconds of doubt: confirming the original behaviour failed the new assertion (instead of just adding code and hoping) means the test we now have is actually load-bearing.

**Course corrections during execution:**

- The first draft of `seed:en:flowers` used `"drink the nectar"` and `"sugary drink"`. The word `"drink"` is uncommon enough that BM25 ranked the new passage 4th on the unrelated query `"why does ice cool a drink"`, knocking the previous 5th-place winner out and breaking that query's loose-recall assertion (`required: ["density"]`). Switched to `"sip the nectar"` / `"sugary liquid"` — same meaning, no BM25 collision. Lesson: when adding new corpus content, watch for low-frequency tokens that overlap with off-topic benchmark queries.
- The brief's wording was "expand the plant passage to mention flowers + scent" (referring to `wiki-simple:en:plant`). I deviated: editing the auto-generated wiki JSONL would be clobbered on the next `python3 simple_wikipedia.py --language en` run. Instead, I added a hand-drafted seed passage (`seed:en:flowers`) and re-targeted the benchmark canonical_id. This is durable across re-ingests AND aligns with the architectural distinction "seed corpus is hand-drafted CC0; wiki is bulk import". The brief is directional, not literal — what matters is making the query findable and keeping the change durable.

**Out of scope for this PR (intentional, deferred):**

- Klexikon corpus expansion (24 → 50+ articles) — concrete, no code change needed.
- German retrieval-quality benchmark — would unlock regression protection on the German layer.
- `simple_wikipedia.py` split (still 927 lines).
- Network-error retry in the Python ingest pipeline (issue #38 was status-code only).
- The other 2 #45 paraphrases ("what is inside a tiny bug", "why does the brain need oxygen from the lungs") — these continue to live in `KNOWN_FAILING_QUERIES` (BM25-only) but pass under hybrid; they serve as the ongoing BM25-vs-hybrid-lift demonstration. Not part of the closure — they were already passing under hybrid.

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
- **#46** — explore post-RRF `min_score` as a fifth grid axis in the hybrid sweep.
- **#40** — aggregate per-source attribution row for Wikipedia (and Klexikon).
- **#41** — tighten disambiguation regex if false positives appear.
- **`simple_wikipedia.py` is still 927 lines** (well past the 500-line guideline). Split candidates carried forward from prior sessions, still deferred until a third source comes in: `wiki/source.py` (dataclass + presets), `wiki/strip.py` (wikitext stripper), `wiki/fetch.py` (strategy dispatch + retry), `wiki/retry.py` (factor next to that triad).
- **Failed-batch persistence sidecar (issue #38 optional).** Was deferred during brainstorming; file as new issue if scheduled ingest ever ships.
- **Network-error retry.** `requests.exceptions.ConnectionError` / `Timeout` still propagate unchanged. If scheduled ingest ships, this becomes the obvious next layer; for one-shot dev runs it's a feature, not a bug.
- **The other 2 #45 paraphrases** ("what is inside a tiny bug", "why does the brain need oxygen from the lungs") still sit in `KNOWN_FAILING_QUERIES` as BM25-vs-hybrid-lift exhibits. If we ever want them to pass under BM25-only as well, the cure is a similar corpus expansion: name "thorax" / "six legs" inside the insect passage in child-friendly terms, name the brain-oxygen dependency in `seed:en:brain` directly. Low priority — they pass under hybrid (the production path).

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
- Tripwire `min top-K` margin tightened slightly (was 0.92, now 0.95 — still 1.9× above the 0.5 floor; still well-defended).
- Hybrid does not always win on paraphrases — observation from issue #45.

**New observations from this session (not risks per se):**

- **BM25 sensitivity to low-frequency tokens.** Adding a new passage to a 91-passage corpus is a small change in the indexing population, but a single uncommon token in the new passage can compete with off-topic queries. The "drink the nectar" → "sip the nectar" course correction is an instance of this. Pattern: when adding new corpus content, scan the draft for tokens that look uncommon and check whether they appear in any benchmark query (a quick `grep` against `tests/common/mod.rs`).
- **Wiki-source canonical re-targeting is a valid cure for paraphrase gaps.** The brief suggested expanding the wiki passage; in practice, the durable answer was to add a hand-drafted seed passage and re-target the benchmark. The principle: if the question is genuinely outside what a wiki source covers (scent in a photosynthesis-focused article), re-targeting the canonical mapping to a topic-appropriate seed is more honest than padding the wiki article with off-topic prose. Plus the wiki article would get clobbered on next ingest.
- **The TDD RED → GREEN cycle still pays off on data-only changes.** A first-draft expansion that passes the test by accident is indistinguishable from corpus content that doesn't actually drive the assertion. Confirming RED first guarantees the test is load-bearing.
- **Test counts unchanged across this session.** No new Rust tests added; the 7 BM25 tests, 6 hybrid structural tests, and 1 hybrid recall test all pre-existed — they're now exercising 91 assertable queries instead of 89. The corpus expansion repurposes existing assertion infrastructure, and that's the cheap, sustainable shape.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing.)

- **Hand-drafted CC0 seed corpus is the durable home for child-friendly content.** Wiki-imported passages are bulk imports that get regenerated on each re-ingest; manual edits there are fragile. When a benchmark canonical mapping needs a topic-appropriate target the wiki source doesn't naturally provide, prefer adding a seed passage over editing wiki output.
- **TDD discipline applies to data-only changes too.** RED → GREEN even when the "code" is corpus content.
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level.
- **Carrier structs with `disabled()` no-op constructors** for parameter bundles that not all callers need to configure.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed.
- **Constants in `consts.rs` submodules** (Rust) or top-of-module `_DEFAULT_*` constants (Python). No magic numbers anywhere.
- **File-size hygiene.** Keep modules under 500 lines. `simple_wikipedia.py` is at 927 lines; split candidates listed above. Defer until a third source comes in.
- **Network-injection test seam** for any data-ingest pipeline (`http_client` parameter, substitute `FakeHttpClient`/`KlexikonFakeHttpClient` in tests).
- **Subagent-driven development workflow** for plan execution (or inline executing-plans for small TDD-shaped plans).
- **Defensive sanity tests at the data layer.** `known_failing_queries_all_appear_in_queries`, `strict_subset_canonical_ids_exist_in_corpus`, etc. — they catch typos/stale entries before retrieval logic runs.
- **Always pin `Default` impls of public structs to `consts::*`, with a drift-prevention test.**
- **Pure helpers + their own unit tests, even in `#[ignore]`'d integration tests.**
- **Frozen dataclasses for process-wide configuration** (the `WikiSource` pattern; `RetrySettings` follows the same shape).
- **String discriminators for strategy selection, with allow-list validation.**
- **Re-run the live ingest after changing any fetch-path helper.** (Skipped this session — the change was seed-corpus only and didn't touch the ingest pipeline.)
- **Kwarg-injected side-effect functions for TDD seams in Python.**
- **Watch for low-frequency-token BM25 collisions** when adding new corpus content. (New this session.)

## Exact commands needed to resume

```bash
# Resume on main (after PR #58 merges):
cd /Users/hherb/src/primer
git status                       # confirm clean
git checkout main
git pull
git log --oneline -10            # the issue-45 corpus-expansion merge commit should be near the top

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
# After issue #45 partial closure: min top-K 0.95, median 4.25, worst top-1 2.06; PASS at the 0.5 floor.
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
# Expected: 100% loose / 100% strict on all 91 queries / 24 mappings.
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If you discover that Horst did interim work that changes the plan, flag it.
