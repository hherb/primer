# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-10 (after the Klexikon corpus expansion 24 → 66 + a follow-up commit on the same PR addressing review feedback; PR #59 open).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **605 Rust tests** under default features (unchanged this session — corpus expansion was data-only on the Rust side). Add `--features primer-kb-load/fastembed` for the embedding-backed sweep + recall test (downloads BGE-M3 ~570 MB on first run; cached afterwards). Plus **135 Python tests** in `data/ingest/` (one new test was added during PR #59 review for the post-resolution duplicate-id check).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes.

## Branch status

`feature/klexikon-corpus-expansion-24-to-65` carries 2 commits (initial expansion + review-feedback follow-up), pushed, and on **PR #59** (https://github.com/hherb/primer/pull/59). Built off `origin/main` after the PR #58 (issue #45 full closure) merge. Branch name is fixed at "24-to-65"; after the follow-up commit the corpus is actually 66 (Jahreszeit was missed in the first pass and added during review). Once #59 merges, the branch can be deleted.

## What we shipped this session

**Klexikon corpus expansion: 24 → 66 articles.** Hand-curated 42 additional German children's-curriculum titles spanning the same five clusters as the English seed corpus, lifting de-locale coverage from a starter set toward parity with the English seed (56 hand-drafted + 35 Simple English Wikipedia = 91).

**2 commits on `feature/klexikon-corpus-expansion-24-to-65`:**

- `62faff5` — `feat(ingest): expand Klexikon corpus from 24 to 65 articles` (initial expansion: 41 articles)
- follow-up — `fix(ingest): address PR #59 review feedback` (added Jahreszeit; structural duplicate-id check; whitelist comment; doc count corrections)

**Concrete deliverables:**

- 42 new entries appended to `data/ingest/klexikon_whitelist.txt`, organised under the existing five cluster headers (Space, Earth & weather, Life, Body, How things work).
- Regenerated `data/seed/wiki_passages.de.jsonl` (66 passages; 24 → 66) via the existing pipeline. Live HTTP to `klexikon.zum.de`, 1-per-second pacing (no batching for the `parse` strategy). The PR #57 retry path (HTTP 429 / 5xx backoff) was active but not exercised — runs completed with no transient failures.
- Pre-validation: every candidate was probed against Klexikon's `action=query` API before being staged in the whitelist. Results: **0 missing**, **14 redirects** to a canonical form (most often plural — `Säugetier` → `Säugetiere`, `Hunde`, `Bienen` etc. — sometimes a synonym/broader topic such as `Berg` → `Gebirge`, `Universum` → `Weltall`, `Atom` → `Atome und Moleküle`, `Jahreszeit` → `Jahreszeiten`). The fetch path's `redirects=1` flag follows redirects transparently and writes the canonical title.
- `Molekül` was deliberately omitted: it shares the canonical `Atome und Moleküle` redirect target with `Atom` and would have caused an id collision in the JSONL output. The ingest path now hard-fails on post-resolution duplicate ids — the manual probe is no longer the only line of defence.
- **Structural duplicate-id check** in `simple_wikipedia.py::_emit_passages` (review-feedback follow-up): after redirect resolution, a `dict[id -> title]` accumulator detects collisions and raises `RuntimeError` listing the colliding input titles. Replaces the manual probe-first habit; documented in the whitelist comment.
- Doc updates: `README.md` (status line + Phase 0.2 bootstrapping bullet, 24 → 66), `ROADMAP.md` (new ✅ bullet documenting the expansion + annotation on the existing Klexikon-rollout bullet, 24 → 66), `CLAUDE.md` (`auto_seed_if_empty` bullet, 24 → 66), whitelist comment header rewritten to describe the redirect shapes accurately.
- Code change: 1 (`_assert_unique_passage_ids` + wiring in `main`). Test changes: +3 Python tests (132 → 135) covering the passing case, the Atom/Molekül collision shape with synthetic redirects, and the actionable error-message contract.
- Verification: 66 unique ids, all schema-valid (`id`, `source`, `license`, `attribution`, `source_url`, `text`, `topics`), all `text` fields ≥50 chars. Python tests at 135/135. Rust workspace at 605/0/2.

**Cluster breakdown of the additions:**

- **Space (5):** Galaxie, Komet, Universum, Sternbild, Mondfinsternis
- **Body (10):** Skelett, Knochen, Muskel, Haut, Zahn, Ohr, Nase, Magen, Blut, Verdauung
- **Life (12):** Säugetier, Reptil, Fisch, Dinosaurier, Hund, Katze, Biene, Schmetterling, Baum, Blume, Wald, Pilz
- **Earth & weather (8):** Berg, Fluss, Meer, See, Wüste, Schnee, Wind, Jahreszeit
- **How things work (7):** Atom, Schwerkraft, Energie, Elektrizität, Feuer, Luft, Temperatur

**Course corrections during execution:**

- The handoff I started from (NEXT_SESSION.md before this rewrite) was stale — written after PR #57 (#38 backoff) but before PR #58 (issue #45 full closure). Horst flagged it; I verified PR #58 was merged (commit `5f447aa` on origin/main) and `KNOWN_FAILING_QUERIES_HYBRID` was already empty. Took #45 off the candidate task list before re-asking direction.
- Initial probe surfaced the `Atom`/`Molekül` collision risk before any HTTP for the actual ingest. The whitelist's `_assert_unique_slugs` check is on the input slugs (per-title, before resolution), so post-redirect collisions slip past it — caught only by the probe. This justified the probe-first pattern for future expansions; the review-feedback follow-up turned it into a structural check inside the ingest so future contributors don't need the manual habit.
- During code review, the cluster-header counts were spotted as inaccurate ("(8)" for Earth & weather but only 7 articles in whitelist; Jahreszeit was listed in the description but missing from whitelist). The fix was to actually add Jahreszeit, not edit the description down — it was a real intent slip from the first pass. Total corpus is now 66, not 65, after the follow-up.

**Out of scope for this PR (intentional, deferred):**

- License-claim spot-check on individual page footers (only the site-wide CC-BY-SA-4.0 has been verified; per-page divergence remains a low-priority audit item).
- German retrieval-quality benchmark (`tests/retrieval_quality.rs` analog) — still not implemented for the de layer; expanded corpus makes it more valuable.
- BGE-M3 retrieval validation in German (multilingual model covers de, but no smoke test exists).
- Klexikon corpus expansion past 66 (the wiki has ~3000 articles; 66 is "rounded curriculum starter", not "comprehensive").

## What's next

### Larger queue items (carried forward)

- **Local llama.cpp inference (Phase 1.1).** `LlamaCppBackend` stub at `primer-inference` is the entry point.
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality, not production).
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **Hindi (`hi`) locale pack rollout.** Schema + i18n boundary are already locale-keyed; the open work is the prompt-pack TOML, the seed corpus / wiki layer, and BGE-M3 retrieval validation in Devanagari. Open question: Wikipedia has a Hindi children's-wiki analog called "Bal Vikipedia" (बाल विकिपीडिया) — worth checking if it's API-accessible before defaulting to regular hi.wikipedia.org. The German session pattern (Klexikon over de.wikipedia) confirms the heuristic: prefer the children's-vocabulary source whenever one exists.
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default. The relevant flips: `default = ["embedding"]` in `primer-cli/Cargo.toml` and the `--embedder-backend` CLI default.

### Smaller-scope follow-ups still open

- **German retrieval-quality benchmark.** No equivalent of `tests/retrieval_quality.rs` exists for the German layer yet. Concrete acceptance: hand-author a German equivalent of `tests/common/mod.rs::QUERIES` (~25-30 child-style German questions targeting specific Klexikon ids), wire a parallel test that runs against the de KB. Higher value now that the corpus is 66 articles deep — 24 was at the cusp where regression-protection cost-benefit was weak. **Suggested target: ~25 BM25-only loose queries + ~12 strict canonical-id mappings; an `--ignored` BGE-M3 hybrid recall test as a follow-up.**
- **Klexikon license claim spot-check.** Site-wide CC BY-SA 4.0 is the canonical claim and is what the 66 passages now uniformly carry. Concrete acceptance: WebFetch a sample of ~5 article footers (e.g. `Sonne`, `Atom`/`Atome und Moleküle`, `Skelett`, `Bienen`, `Mondfinsternis`) and verify the footer license matches. If a per-page divergence appears, document the override path in `WikiSource` (probably needs a per-passage license override field). Low-priority.
- **Klexikon corpus expansion past 66.** Klexikon has ~3000 articles. Concrete next-batch acceptance: pick 30-50 more titles in still-thin clusters — body could use Atmung-related topics; life could use Ökosystem, animal-specific (Affe, Pferd, Fledermaus); how-things-work could use Computer, Internet, Telefon, Maschine. Re-run pipeline. No code change required (the structural duplicate-id check landed on PR #59 makes future redirect-collision risk a hard fail rather than a silent dup).
- **#46** — explore post-RRF `min_score` as a fifth grid axis in the hybrid sweep.
- **#40** — aggregate per-source attribution row for Wikipedia (and Klexikon).
- **#41** — tighten disambiguation regex if false positives appear.
- **#22** — cache prepared statements for the corpus-bootstrap path (Phase 0.2; enhancement).
- **#21** — separate `--languages` preference list from bound `--language` locale.
- **#20** — i18n placeholder validator can false-fail on translator narrative text.
- **`simple_wikipedia.py` is 927 lines** (well past the 500-line guideline). Split candidates carried forward: `wiki/source.py` (dataclass + presets), `wiki/strip.py` (wikitext stripper), `wiki/fetch.py` (strategy dispatch + retry). Defer until a third source comes in.
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

- **Manual probes are good but a structural ingest-time check is better.** The `Atom` / `Molekül` collision was caught at probe time on the first pass, but PR #59 review noted that without the structural defence, the next contributor adding a whitelist entry without manually probing could silently reintroduce the same bug. The fix landed in `simple_wikipedia.py::_emit_passages`: a `dict[id -> input_title]` accumulates as passages are emitted; a duplicate raises `RuntimeError` listing the colliding inputs. Manual probing is still cheap and catches the issue earlier in the workflow, but it's no longer load-bearing.
- **The handoff is only as fresh as the moment it was written.** PR #58 merged hours after PR #57's brief was authored, so the brief described an open follow-up that was already closed. Recommendation: at session start, always verify the open-issue claims in the brief against `gh issue list --state open` and against a `git log origin/main` since the brief's "last updated" timestamp. Took 1 round-trip with Horst this session.
- **Auto-seed loads everything that matches `*.<pack>.jsonl` — no schema bump or test change is needed when the corpus grows.** The 42 new passages picked up automatically on the next `--language de` session; this is the runtime contract `discover_seed_files` provides. Useful to remember when sizing future expansions: corpus growth is a data-only change as long as the JSONL schema doesn't move.
- **Cluster-count typos in PR descriptions are cheap to introduce and worth catching at code-review time.** First-pass description claimed "Earth & weather (8)" but only 7 articles were in the whitelist (Jahreszeit slipped); "How things work (6)" but listed 7 names. These were spotted only because the reviewer cross-counted. Pattern: when a description enumerates additions, the count next to the cluster header should be a count of the items in the bullet list immediately after, not a vibe-check.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing.)

- **Pure functions in dedicated modules** for algorithmic cores — tested directly without I/O.
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level.
- **Carrier structs with `disabled()` no-op constructors** for parameter bundles that not all callers need to configure.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed.
- **Constants in `consts.rs` submodules** (Rust) or top-of-module `_DEFAULT_*` constants (Python). No magic numbers anywhere.
- **TDD discipline.** Tests first; watch them fail; implement to green. (Not exercised this session — pure data update with structural verification.)
- **File-size hygiene.** Keep modules under 500 lines. `simple_wikipedia.py` is 927 lines; split candidates listed above.
- **Network-injection test seam** for any data-ingest pipeline (`http_client` parameter, substitute `FakeHttpClient`/`KlexikonFakeHttpClient` in tests).
- **Subagent-driven development workflow** for plan execution (or inline executing-plans for small TDD-shaped plans).
- **Defensive sanity tests at the data layer.** Confirmed standing.
- **Always pin `Default` impls of public structs to `consts::*`, with a drift-prevention test.** Mirrored on the Python side as `RetrySettings.default()` ↔ module-level `DEFAULT_*` consts.
- **Pure helpers + their own unit tests, even in `#[ignore]`'d integration tests.**
- **Frozen dataclasses for process-wide configuration** (the `WikiSource` pattern; `RetrySettings` follows the same shape).
- **String discriminators for strategy selection, with allow-list validation.**
- **Re-run the live ingest after changing any fetch-path helper.** Tiny expected diff = success, anything bigger = a flag worth investigating.
- **Kwarg-injected side-effect functions for TDD seams in Python.**
- **Structural ingest-time defences beat manual probing habits.** Confirmed via PR #59 review feedback — see "New observations" above.

## Exact commands needed to resume

```bash
# Resume on main (after PR #59 merges):
cd /Users/hherb/src/primer
git status                       # confirm clean
git checkout main
git pull
git log --oneline -10            # the corpus-expansion merge commit should be near the top

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

For real-LLM smoke testing in German (now 66 Klexikon passages):

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
- If you discover that Horst did interim work that changes the plan, flag it. **(This session was a textbook example — the previous handoff was 1 PR stale and Horst caught it before I started work.)**
