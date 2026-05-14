# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-14T0818+0800 (after opening PR #93 — German stress-paraphrase queries; commit `e8e83ea` on `feat/de-stress-paraphrases-issue-64`).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **739 Rust tests** under default features after PR #93 merges. 3 ignored. Add `--features primer-kb-load/fastembed` for the embedding-backed sweeps + the real-BGE-M3 recall tests (downloads BGE-M3 ~570 MB on first run; cached afterwards). Plus **135 Python tests** in `data/ingest/` (unchanged this session — Rust-only work).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.

## Branch status

`feat/de-stress-paraphrases-issue-64` carries 1 commit, pushed, and on **PR #93** (https://github.com/hherb/primer/pull/93). Built off `origin/main` at `aaa87ae` (PR #89 squash — voice mode Phase A). Once #93 merges, the branch can be deleted.

**Note for next-session sanity check:** the prior NEXT_SESSION.md (2026-05-11T2206+0800) was significantly out of date by the time this session ran — PRs #66 through #89 had merged since (10+ commits, focused on the desktop GUI and voice mode). Test count grew from the brief's claimed 658 → 739. Always run `git log origin/main` since the brief's timestamp before trusting the "what's next" list.

## What we shipped this session

**Closed issue #64 — German stress-paraphrase queries.** Expanded the German benchmark from 26 → 31 queries with 5 deliberately mis-aligned child-style paraphrases that mirror the EN issue #45 paraphrase shape. Demonstrates the BM25-vs-hybrid lift the prior 26-query German benchmark didn't exercise (every initial query was authored against its target's lead text).

**Commits on `feat/de-stress-paraphrases-issue-64`:**

- `e8e83ea` — `test(retrieval): add German stress-paraphrase queries (closes #64)`

**Concrete deliverables:**

- 5 paraphrases added to `tests/common/de.rs::QUERIES_DE`, with `canonical_id` pinned to the targeted Klexikon article and `required` loose-substrings drawn from the canonical article's lead:
  - `"warum macht mein bauch komische geräusche wenn ich hunger habe"` → `wiki-klexikon:de:verdauung`
  - `"wieso wird es im sommer länger hell als im winter"` → `wiki-klexikon:de:jahreszeiten`
  - `"warum bekomme ich gänsehaut wenn mir kalt ist"` → `wiki-klexikon:de:haut`
  - `"warum gibt es ebbe und flut am meer"` → `wiki-klexikon:de:mond`
  - `"warum sind blätter im herbst bunt"` → `wiki-klexikon:de:baum`
- `KNOWN_FAILING_QUERIES_DE` populated with 3 BM25-only misses (verdauung, haut, mond) carrying a one-line rationale and the actual surfaced top-5 ids.
- `KNOWN_FAILING_QUERIES_DE_HYBRID` populated with 2 corpus gaps (haut, mond) carrying detailed rationales — both Klexikon articles lack the targeted content (haut doesn't describe the goosebumps reflex; mond doesn't discuss tides).
- Doc updates: `CLAUDE.md` (lines 133-134; numbers updated to 31/25; rationale for new KNOWN_FAILING entries; replaced "stress-paraphrase follow-up queued" with "stress-paraphrase expansion landed"), `README.md` (lines 46-47; explains the BM25/hybrid lift demonstration and the 2 documented corpus gaps), `ROADMAP.md` (new `✅ German stress-paraphrase queries` bullet directly under the prior `✅ German retrieval-sweep diagnostics` bullet).

**Findings against today's 66-passage Klexikon corpus + 31-query benchmark:**

| Path | Strict recall (25 canonicals) | Loose recall (31 queries) |
|---|---|---|
| BM25 at production defaults `(top_k=5, min_score=0.5)` | 22/25 = 88% | 28/31 = 90% |
| Hybrid at `HybridParams::default()` `(30, 30, 5, 60)` | 23/25 = 92% | 29/31 = 94% |
| Hybrid notional winner `(10, 10, 5, 60)` | 24/25 = 96% | 30/31 = 97% |

- **BM25 misses 3 paraphrases** (verdauung/haut/mond) because the dense-vocabulary articles don't share words with the child phrasings. Surfaced lexically adjacent: bauch-geräusche → energie/schall/saugetiere/planet/ohr; gänsehaut-kalt → energie/planet/klima/temperatur/schnee; ebbe-flut-meer → meer/fluss/wetter/insekten/fische.
- **Hybrid lifts 1 of those 3** (verdauung). The semantic bridge digestion ↔ stomach noises is exactly the kind of lift the dense leg is supposed to provide.
- **2 paraphrases (haut, mond) remain hybrid misses** because they're corpus-coverage gaps: the article doesn't have the content. No embedding can bridge to text that isn't there.
- The hybrid sweep's notional 96% winner cell `(bm25_top_k=10, vector_top_k=10)` is one query different from production defaults — does NOT motivate a default change at this corpus size.

No production defaults change. The diagnostic harnesses (sweep_de, sweep_hybrid_de) automatically pick up the new queries via the shared `common::de` module.

**Verification:**
- `~/.cargo/bin/cargo build --workspace` clean
- `~/.cargo/bin/cargo test --workspace` → 739 passed / 0 failed / 3 ignored (unchanged baseline; sanity tests don't grow when only data expands)
- `~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_de` → 12 passed (3 BM25 misses correctly excluded via KNOWN_FAILING_QUERIES_DE)
- `~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_quality_hybrid_de` → 12 passed (2 corpus gaps correctly excluded via KNOWN_FAILING_QUERIES_DE_HYBRID)
- `~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_sweep_de -- --ignored sweep_retrieval_params_de --nocapture` → 1 passed; winning cell `top_k=10, min_score=1.50` at 92% strict / 94% loose; paraphrases visible in fail list
- `~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_sweep_hybrid_de -- --ignored sweep_retrieval_params_hybrid_de --nocapture` → 1 passed; ~78s wallclock with BGE-M3 cached; notional winner `(10, 10, 5, 60)` at 96% strict
- `~/.cargo/bin/cargo clippy --workspace --all-targets` clean
- `~/.cargo/bin/cargo fmt -p primer-kb-load -- --check` clean (pre-existing fmt issues elsewhere on main — `primer-cli/src/speech_loop/mod.rs:113`, `primer-gui/build.rs:29,37`, `primer-gui/src/commands/session.rs:546` — not introduced here; file as separate issue if it bothers)

**Net diff:** 4 files changed, 89 insertions, 13 deletions.

**Design choices that may be relevant later:**

- **5 paraphrases (not 4 or 6) was a judgement call.** The issue requested 4-6; 5 gives a clear demonstration of all three BM25/hybrid outcome shapes (BM25 passes due to vocabulary overlap; BM25 misses but hybrid lifts; both miss because of a corpus gap) without padding.
- **The `required` loose-substring sets were drawn from the canonical article's lead, not from the query phrasing.** This is the EN-side convention from PR #45. It means the loose check measures "did retrieval surface the right article?" rather than "did the query happen to share vocabulary with the corpus?" — and is why the BM25 misses fail both loose AND strict (the canonical's lead lemmas don't appear in any top-K passage when the wrong articles are surfaced).
- **`canonical_id` is pinned for every paraphrase.** This makes both the loose check (any passage in top-K must contain the required lemmas) and the strict check (the exact canonical id must be in top-K) load-bearing. None of the new entries carry `canonical_id: None`.
- **The two corpus gaps are option (b) per the issue.** Adding a hand-drafted German seed layer alongside Klexikon would mirror the EN seed_passages / wiki_passages split, but the user's intent for `de` is that Klexikon IS the children's wiki for German — there's no parallel hand-drafted seed planned today. So the 2 gaps land in `KNOWN_FAILING_QUERIES_DE_HYBRID` with rationales until/unless that decision changes.
- **Reviewed cluster assignment:** verdauung→Body, jahreszeiten→Space (Earth's orbit/axis), haut→Body, mond→Space, baum→Life. All five clusters keep their ≥3 floor.
- **`KNOWN_FAILING_QUERIES_DE` and `KNOWN_FAILING_QUERIES_DE_HYBRID` overlap on 2 entries.** That's the documented contract: an entry in both means "BM25 misses AND hybrid can't bridge". An entry only in `KNOWN_FAILING_QUERIES_DE` means "BM25 misses but hybrid lifts" — the verdauung paraphrase is the first such entry on the DE side.

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

- **Klexikon corpus expansion past 66.** Klexikon has ~3000 articles. The 2 corpus gaps from this session (gänsehaut reflex; tides on the mond article) would need either expanded articles or additional Klexikon titles to lift. Concrete next-batch acceptance: pick 30-50 more titles in still-thin clusters — body could use Atmung-related topics; life could use Ökosystem, Affe/Pferd/Fledermaus; how-things-work could use Computer/Internet/Telefon/Maschine. Re-run pipeline. No code change required. Once corpus grows past ~150 passages, re-run the sweep harnesses to verify production defaults still flatline at 100% strict from `top_k=5` onward on non-paraphrase queries (and that the BM25 floor's defensive 0.5 hasn't been challenged by the larger statistic).
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

- **The 26-query baseline DE benchmark was too easy.** Every query was authored against its target article's lead text, so BM25 had the same vocabulary it was being scored against. Adding 5 paraphrases reduced BM25 strict recall from 100% to 88%, surfacing the natural BM25/hybrid contrast the EN side had documented since issue #45. The "no measurable hybrid lift on DE" claim from PR #65 was therefore an artifact of query authoring, not a property of the multilingual dense leg.
- **The dense leg lifts when content exists; semantic context isn't enough on its own.** For the gänsehaut paraphrase, BGE-M3 correctly understands the "kalt" context — it surfaces temperatur/klima/schnee/wüste/feuer. But none of those articles describe goosebumps. The dense leg can find topically-related content but can't synthesize content that isn't in the corpus. Same shape on the ebbe/flut paraphrase: hybrid surfaces fluss/meer/wetter/regen/fische (semantically near "ocean tides") but the mond article doesn't discuss tides. This is the load-bearing limit of any retrieval-only system; the only fixes are corpus expansion or a generative response that doesn't depend on retrieval.
- **The notional 96% hybrid winner cell is a 1-query difference.** Production defaults `(bm25_top_k=30, vector_top_k=30, final_top_k=5, rrf_k=60)` hit 23/25. The `(10, 10, 5, 60)` cell hits 24/25 — likely the verdauung paraphrase surfaces more reliably with a narrower BM25 leg pulling fewer unrelated lexical hits to the top before fusion. Not worth changing defaults for one query at this corpus size; revisit if the German corpus grows past ~150 passages.
- **Pre-existing fmt issues on main.** `primer-cli/src/speech_loop/mod.rs:113`, `primer-gui/build.rs:29,37`, `primer-gui/src/commands/session.rs:546` (and others) trigger `cargo fmt --check`. Not introduced this session — verified by `git stash && cargo fmt --check`. Should be filed as a follow-up issue; impacts CI cleanliness if/when a fmt-strict workflow is added.
- **NEXT_SESSION.md staleness.** The prior brief (2026-05-11) had test count 658; current is 739. That's 81 tests added across the merged GUI/voice PRs. The "first moves" step that says "verify open-issue claims and `git log origin/main` since this brief's timestamp" caught the gap before any time was wasted acting on stale guidance.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing, with no new patterns added this session.)

- **Pure functions in dedicated modules** for algorithmic cores — tested directly without I/O.
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level.
- **Carrier structs with `disabled()` no-op constructors** for parameter bundles that not all callers need to configure.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed.
- **Constants in `consts.rs` submodules** (Rust) or top-of-module `_DEFAULT_*` constants (Python). No magic numbers anywhere.
- **TDD discipline.** Tests first; watch them fail; implement to green. **Honoured this session** — wrote paraphrases, ran tests to observe BM25/hybrid behaviour, then classified into KNOWN_FAILING lists based on observed outcomes rather than guessing.
- **File-size hygiene.** Keep modules under 500 lines. ✅ This session: `common/de.rs` grew from 341 → 408 lines.
- **Network-injection test seam** for any data-ingest pipeline (`http_client` parameter, substitute `FakeHttpClient`/`KlexikonFakeHttpClient` in tests).
- **Subagent-driven development workflow** for plan execution (or inline executing-plans for small TDD-shaped plans).
- **Defensive sanity tests at the data layer.** ✅ Confirmed — `de_strict_subset_canonical_ids_exist_in_corpus` caught any typos in canonical_ids before runtime.
- **Always pin `Default` impls of public structs to `consts::*`, with a drift-prevention test.** Mirrored on the Python side as `RetrySettings.default()` ↔ module-level `DEFAULT_*` consts.
- **Pure helpers + their own unit tests, even in `#[ignore]`'d integration tests.**
- **Frozen dataclasses for process-wide configuration** (the `WikiSource` pattern; `RetrySettings` follows the same shape).
- **String discriminators for strategy selection, with allow-list validation.**
- **Re-run the live ingest after changing any fetch-path helper.** Tiny expected diff = success, anything bigger = a flag worth investigating.
- **Kwarg-injected side-effect functions for TDD seams in Python.**
- **Structural ingest-time defences beat manual probing habits.** Confirmed via PR #59 review feedback.
- **Back-compat re-export shims when bulk-editing test imports would dilute a structural-refactor PR.** Confirmed PR #60 / #61 — shim landed first, lived for one PR cycle, then went away as a 2-commit cleanup.
- **Plan-then-execute inline for mechanical refactors with a strong test safety net.** Re-confirmed.
- **Two-commit refactor: "set up the change" then "remove the old".** Splits a "migrate + delete" diff into a bisectable mid-state and a clean cleanup commit.
- **Ship the follow-up issue body with explicit acceptance + per-file checklist when a PR ships a transitional state.**
- **Per-locale dataset modules under `tests/common/<pack>.rs`** for benchmark data. Pattern from PR #63 — confirmed reusable in PR #93 (paraphrases) and PR #65 (sweep harnesses).
- **Locale-specific cluster sizing via a named const** (`DE_CLUSTERS = 5`). Pattern from PR #65 — still relevant for any future locale-specific test harnessing.
- **Required-loose-substrings drawn from the canonical article's lead, not the query phrasing.** EN-side convention from PR #45 — re-confirmed standing in PR #93. Makes the loose check measure retrieval, not vocabulary alignment.

## Exact commands needed to resume

```bash
# Resume on main (after PR #93 merges):
cd /Users/hherb/src/primer
git status                       # confirm clean
git checkout main
git pull
git log --oneline -10            # the PR-93 squash-merge commit should be near the top

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 739 passed, 0 failed, 3 ignored (after PR #93 merges).

~/.cargo/bin/cargo clippy --workspace --all-targets
# Note: cargo fmt --all -- --check has pre-existing issues on main that this PR did not touch.
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
