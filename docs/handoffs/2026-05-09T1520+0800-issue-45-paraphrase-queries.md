# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-09 (after issue #45 paraphrase re-add — hybrid lifts 2 of 4).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **605 Rust tests** under default features (unchanged from prior session — the 4 new benchmark queries are skipped by `KNOWN_FAILING_QUERIES`, so the BM25 regression test still passes; the test count is identical). Add `--features primer-kb-load/fastembed` for the embedding-backed sweep + recall test (downloads BGE-M3 ~570 MB on first run; cached afterwards). Plus 43 Python tests in `data/ingest/` (pytest from a venv set up per `data/ingest/README.md`).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes.

## Branch status

`feature/issue-45-paraphrase-queries` carries 2 commits ready to land via PR (to be opened against `main`). Built off the latest `origin/main` (`b47c923`, the merge of PR #52 / BM25 floor tripwire). Once merged, the branch can be deleted from the remote at the user's discretion.

## What we shipped this session

**Reintroduced 4 paraphrase queries dropped during Phase 0.2 closeout. Hybrid retrieval lifts 2 of 4; the other 2 are corpus-coverage gaps tracked for future seed-corpus expansion.** Closes issue #45 partially.

The 4 queries had been dropped from `tests/common/mod.rs::QUERIES` because BM25-only at top_k=5 surfaced lexically adjacent passages instead of the semantically correct ones. The `KNOWN_FAILING_QUERIES` mechanism existed but wasn't applied to them. After re-adding them and running the hybrid recall test:

| Query                                                  | BM25-only top hit       | Hybrid top hit            | Hybrid lift?          |
|--------------------------------------------------------|-------------------------|---------------------------|-----------------------|
| what is inside a tiny bug                              | seed:en:cells           | seed:en:insects           | ✅ lifted (strict)    |
| why does the brain need oxygen from the lungs          | seed:en:lungs-breathing | seed:en:brain             | ✅ lifted (strict)    |
| what makes my tummy growl when I am hungry             | seed:en:brain           | seed:en:sleep-dreams      | ❌ corpus-coverage gap |
| why do flowers smell nice                              | seed:en:photosynthesis  | seed:en:photosynthesis    | ❌ corpus-coverage gap |

**Two commits on `feature/issue-45-paraphrase-queries`:**

- `0d0eb61` — `test(retrieval): re-add 4 paraphrase queries (#45); hybrid lifts 2 of 4`
- (handoff commit follows)

**Concrete deliverables:**

- 4 new `BenchQuery` entries in `src/crates/primer-kb-load/tests/common/mod.rs::QUERIES`, each with `canonical_id: Some(...)`. Strict-subset count grew from 20 → 24. Total benchmark grew from 87 → 91.
- All 4 added to `KNOWN_FAILING_QUERIES` (BM25-only path) so `seed_corpus_satisfies_canonical_queries` continues to pass.
- 2 added to `KNOWN_FAILING_QUERIES_HYBRID` (was empty) with detailed explanations of why hybrid still misses them and what the cure looks like (corpus expansion that names the symptom/scent vocabulary, not retrieval tuning).
- Hybrid recall test (`hybrid_retrieval_recall_with_fastembed`) re-run with `--features fastembed`: PASS at 100% loose / 100% strict on the 89-query / 22-mapping effective subset.
- Tripwire (`bm25_score_floor_tripwire`) re-run: still PASS. Worst-case top-1 dropped from 3.35 → 2.11 (the 2 hard paraphrases have weak BM25 top hits even at rank 1); min top-K stayed at 0.92; median 4.20. The 0.5 floor remains a no-op for recall.
- README.md, ROADMAP.md, CLAUDE.md all updated to reflect the new query counts and the partial-lift finding.
- `cargo fmt --all -- --check` clean. `cargo clippy --workspace --all-targets` clean. Full workspace test count: **605 pass / 0 fail / 2 ignored** (unchanged from before — the 4 new BM25-failing queries are skipped via KNOWN_FAILING; the 2 hybrid-failing queries are skipped via KNOWN_FAILING_HYBRID).

**TDD discipline:**

- RED: added the 4 queries to `QUERIES` without putting them in `KNOWN_FAILING_QUERIES` first. Ran `cargo test -p primer-kb-load --test retrieval_quality` — confirmed all 4 queries failed both loose and strict on BM25-only with the exact "got ids" diagnostic the issue described.
- GREEN: added the 4 to `KNOWN_FAILING_QUERIES`. Re-ran — pass.
- ADDITIONAL VERIFICATION: ran `--features fastembed --test retrieval_quality_hybrid` to discover hybrid only lifted 2 of 4. Added the remaining 2 to `KNOWN_FAILING_QUERIES_HYBRID` with detailed comments. Re-ran — pass.

## What's next

### Larger queue items (carried forward)

- **Local llama.cpp inference (Phase 1.1).** `LlamaCppBackend` stub at `primer-inference` is the entry point.
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality, not production).
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **Hindi (`hi`) and German (`de`) locale pack rollout** — the locale-keyed schema is in place; only the templates and per-locale wiki layers need to be added. Per-locale wiki layers would auto-load via `auto_seed_if_empty`'s multi-file discovery.
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default. The relevant flips: `default = ["embedding"]` in `primer-cli/Cargo.toml` and the `--embedder-backend` CLI default.

### Smaller-scope follow-ups still open

- **#45 (partial closure today)** — 2 paraphrases remain in `KNOWN_FAILING_QUERIES_HYBRID`: "what makes my tummy growl when I am hungry" and "why do flowers smell nice". Cure: a focused seed-corpus expansion that adds vocabulary like "growl" / "growls when empty" to `seed:en:digestion` and "scent" / "fragrant" / "smell" to `wiki-simple:en:plant` (or relax the canonical mapping for the second one — photosynthesis is also a plausible Socratic target). When that lands, remove from `KNOWN_FAILING_QUERIES_HYBRID` and use them as a permanent regression guarantee.
- **#46** — explore post-RRF `min_score` as a fifth grid axis in the hybrid sweep.
- **#38** — add 429/5xx backoff to `fetch_leads` (Python ingestion). Not blocking; only one-time runs need it today.
- **#40** — aggregate per-source attribution row for Wikipedia. Not urgent.
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
- Tripwire `min top-K` margin is now narrower (1.84x above the 0.5 floor; was the same number but the 91-query benchmark made the worst-case visible). Documented in CLAUDE.md.

**New observations from this session (not risks per se):**

- **Hybrid does not always win on paraphrases.** The intuition that "BGE-M3 will surface anything BM25 missed" is only ~50% true on this benchmark. The dense leg's vector geometry can route a paraphrase to an adjacent-but-wrong passage just as easily — e.g. "tummy growl when hungry" lands on sleep-dreams (because hunger-related concepts cluster there) instead of digestion. Treat hybrid recall as a useful tool but not a substitute for content coverage.
- **Canonical-mapping is sometimes ambiguous.** For "why do flowers smell nice", both `seed:en:photosynthesis` (top BM25 hit; talks about flowers in some way) and `wiki-simple:en:plant` (canonical assignment; mentions autotroph) are arguably the right answer to a Socratic conversation. The current strict mapping forces one choice. A future improvement: allow `canonical_id: Vec<&str>` for queries with multiple acceptable canonical answers.
- **The strict-subset floor (≥15) is comfortably met today (24).** This gives headroom to mark future paraphrases as strict without worrying about the dataset.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing.)

- **Pure functions in `primer-core`** for algorithmic cores — tested by injection of `now`, no async, no I/O.
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level.
- **Carrier structs with `disabled()` no-op constructors** for parameter bundles that not all callers need to configure.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed.
- **Constants in `consts.rs` submodules.** No magic numbers anywhere.
- **TDD discipline.** Tests first; watch them fail; implement to green. Re-applied this session: added queries to `QUERIES` first (no `KNOWN_FAILING` entry) → confirmed BM25 fails → THEN wired `KNOWN_FAILING`.
- **File-size hygiene.** Keep modules under 500 lines.
- **Network-injection test seam** for any data-ingest pipeline (`http_client` parameter, substitute `FakeHttpClient` in tests).
- **Subagent-driven development workflow** for plan execution.
- **Defensive sanity tests at the data layer.** When test data has multiple consumers, put cheap sanity tests in the shared `tests/common/mod.rs` that validate invariants. They run inside every test binary that imports the module. (Confirmed standing: `known_failing_queries_all_appear_in_queries` caught a stale entry in this kind of dataset edit during session #44; the session #45 edit benefited from the same machinery.)
- **Always pin `Default` impls of public structs to `consts::*`, with a drift-prevention test.**
- **Pure helpers + their own unit tests, even in `#[ignore]`'d integration tests.**

**New pattern from this session (re-applied, not invented):**

- **Use `KNOWN_FAILING_QUERIES_*` lists as a contract, not a dumping ground.** When a benchmark query fails, the choice is not "delete it" or "hide it forever" — it's "annotate why it fails, what the cure looks like, and what would unblock removing it from the list." Each entry in `KNOWN_FAILING_QUERIES_HYBRID` now carries a 4-line comment with the failure mode, the missing vocabulary, the cure, and the issue tracking it. Future contributors can act on this without re-running the benchmark.

## Exact commands needed to resume

```bash
# Resume on main (after the issue-45 PR merges):
cd /Users/hherb/src/primer
git status                       # confirm clean
git checkout main
git pull
git log --oneline -10            # the issue-45 merge commit should be near the top

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 605 passed, 0 failed, 2 ignored (as of 2026-05-09).

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
# Sub-second; prints min/median/floor; PASS today.
# After issue #45: min top-K 0.92, median 4.20, worst top-1 2.11.
# Run whenever the seed corpus expands.
```

For running the hybrid sweep:

```bash
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed \
    --test retrieval_sweep_hybrid \
    -- --ignored sweep_retrieval_params_hybrid --nocapture 2>&1 | tee /tmp/sweep_hybrid.txt
# 54-cell table; ~5–15 min runtime once the model is cached (570 MB on first run).
```

For running the hybrid regression test:

```bash
# Structural (always built):
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_hybrid

# Real-recall (--features fastembed, real BGE-M3):
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed \
    --test retrieval_quality_hybrid -- --nocapture
# After issue #45: passes the 89-query / 22-mapping effective subset.
# 2 KNOWN_FAILING_QUERIES_HYBRID entries are skipped.
```

For re-running the Wikipedia ingest (rare; only when the whitelist changes):

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/python simple_wikipedia.py
# Writes ../seed/wiki_passages.en.jsonl. Commit any diff.
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If you discover that Horst did interim work that changes the plan, flag it.
