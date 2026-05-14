# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-14T1612+0800 (after pushing one additional fix to PR #101 — decoupling the GUI's voice-toggle availability from session state; commit `e33f0d4` on `i18n/voice-state-copy-91` on top of `3fd1903`; closes #91 plus the toggle-disabled smoke-test fallout).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **773 Rust tests** under default features (up from the previous 767 once PR #101 merges; the +6 is from the 2 GUI regression witnesses, 3 pack-side tests covering English + German `voice_state` lookup and the empty-field error path, plus the `voice_mode_available_matches_cfg_feature_speech` test pinning the new capability command's output to the cfg flag). 3 ignored. With `--features primer-gui/speech` an additional 5 unit tests in `voice/assets.rs` run for a total of 93 primer-gui tests on that feature (up from 89; +4 from this PR's GUI-side additions). Add `--features primer-kb-load/fastembed` for the embedding-backed sweeps + the real-BGE-M3 recall tests (downloads BGE-M3 ~570 MB on first run; cached afterwards). Plus **135 Python tests** in `data/ingest/` (unchanged this session — Rust-only work).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.

## Branch status

`i18n/voice-state-copy-91` carries 3 commits, pushed, and on **PR #101** (https://github.com/hherb/primer/pull/101). Built off `origin/main` at `b7b4a5a` (PR #99 squash — voice IPC path re-resolution hardening). Once #101 merges, the branch can be deleted.

## What we shipped this session

**Primary work:** moved the six voice-mode UI display strings the GUI's `get_voice_state_copy` Tauri command serves out of a hardcoded `match` in `commands/voice.rs` and into the `[voice_state]` table of each `primer-pedagogy/prompts/<pack>.toml`. Adding a new locale (e.g. Hindi) now requires no Rust change for voice-state copy — just the new pack's `[voice_state]` table.

**Smoke-test fallout fix (same PR):** the GUI's voice toggle was permanently disabled on the session-picker screen because `restoreOnLaunch` read the `voice_mode_available` flag off `current_session_info`, which returns `null` when no session is active. Added a dedicated `voice_mode_available` Tauri command that returns `cfg!(feature = "speech")` (compile-time constant, no session needed) and switched the frontend to call it.

**Commits on `i18n/voice-state-copy-91`:**

- `3fd1903` — `i18n(voice): move VoiceStateCopy display strings into prompt packs (closes #91)`
- `f647cd3` — `docs: update NEXT_SESSION.md + handoff for PR #101`
- `e33f0d4` — `fix(gui): decouple voice-toggle availability from session state`

**Concrete deliverables:**

- **New `PromptPack` accessor `voice_state_labels() -> &VoiceStateLabels`** at [src/crates/primer-pedagogy/src/prompt_pack.rs:78](src/crates/primer-pedagogy/src/prompt_pack.rs#L78) plus the `VoiceStateLabels` data struct (six `String` fields: `listen_label`, `listen_hint`, `thinking_label`, `thinking_hint`, `speak_label`, `speak_hint`).
- **`[voice_state]` table added to both packs** at [src/crates/primer-pedagogy/prompts/en.toml](src/crates/primer-pedagogy/prompts/en.toml#L194-L209) (CC0 English copy) and [src/crates/primer-pedagogy/prompts/de.toml](src/crates/primer-pedagogy/prompts/de.toml#L212-L227) (German copy; byte-identical to the previously-hardcoded strings). Pack-style comments instruct future translators to keep the label short and the hint a soft reassurance (not an instruction).
- **`validate_voice_state_section`** at [src/crates/primer-pedagogy/src/prompt_pack.rs](src/crates/primer-pedagogy/src/prompt_pack.rs) rejects empty values in any of the six fields at load time. Consumers render the strings unconditionally — a silent empty would produce a blank UI label rather than a clear pack-shape error.
- **`VoiceStateCopy::for_locale` rewritten** at [src/crates/primer-gui/src/commands/voice.rs:405-426](src/crates/primer-gui/src/commands/voice.rs#L405-L426) to call `primer_pedagogy::prompt_pack::load_cached(*locale).expect(...)` then clone the six strings. Mirrors the `.expect()` pattern at `dialogue_manager::lifecycle::DialogueManager::new` — embedded packs are validated at build time so a load failure here would be a structural codebase bug, not a user-recoverable condition.
- **CLAUDE.md gotcha entry** added at [CLAUDE.md:111](CLAUDE.md#L111) documenting the new i18n location and the no-Rust-change rule for new locales.
- **6 new tests:** 2 GUI regression witnesses (`voice_state_copy_english_strings_pinned`, `voice_state_copy_german_strings_pinned` in `commands/voice.rs`) pinning the pre-refactor strings byte-identically — these stay green before AND after the pack switchover; 3 pack-side tests (`english_pack_exposes_voice_state_labels`, `german_pack_exposes_voice_state_labels`, `empty_voice_state_field_returns_err` in `prompt_pack.rs`) covering the new accessor and the empty-field error path; 1 capability-command test (`voice_mode_available_matches_cfg_feature_speech`) pinning the new `voice_mode_available` Tauri command's output to the `cfg!(feature = "speech")` compile-time constant. Future drift now fails at the pack layer first (rather than only at the GUI bridge) AND the toggle-availability path has explicit cfg-flag coverage.

**Smoke-fix deliverables (commit `e33f0d4`):**

- **New `voice_mode_available` Tauri command** at [src/crates/primer-gui/src/commands/voice.rs](src/crates/primer-gui/src/commands/voice.rs) returning `cfg!(feature = "speech")`. Independent of session state so the frontend can decide toggle availability at launch without waiting for a session.
- **Registered in the builder** at [src/crates/primer-gui/src/commands/mod.rs](src/crates/primer-gui/src/commands/mod.rs).
- **Frontend update** at [src/crates/primer-gui/ui/voice.js](src/crates/primer-gui/ui/voice.js#L273-L289): `restoreOnLaunch` now calls `voice_mode_available` instead of pulling the flag off `current_session_info`. The previously-misleading "Voice mode is not built into this binary" tooltip now only fires when the binary genuinely lacks the speech feature.
- The `SessionInfo.voice_mode_available` field is kept (widely consumed by sidebar/refresh paths) but the frontend stops using its value in the launch path.

**Verification:**

- `~/.cargo/bin/cargo test --workspace` → 773 passed / 0 failed / 3 ignored (default features; +6 from baseline 767)
- `~/.cargo/bin/cargo test -p primer-gui --features speech` → 93 passed / 0 failed / 0 ignored (+4 from baseline 89)
- `~/.cargo/bin/cargo fmt --all -- --check` clean
- `RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets` clean (default features)
- `RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech` clean

**Net diff (across both commits):** 9 files changed, 294 insertions(+), 20 deletions(-).

**Design choices that may be relevant later:**

- **One method returning a struct (`&VoiceStateLabels`) rather than six per-field methods.** The existing `PromptPack` trait has both styles — `child_label()/primer_label()` are per-field whereas `engagement_note(state)` is key-based. Six methods would have been consistent with the per-field style but would balloon the trait surface for what is genuinely a single group of related strings. The struct is locale-agnostic data; the GUI's `VoiceStateCopy` is the Tauri-`Serialize`-flavoured equivalent that wraps it.
- **`primer-pedagogy` rather than a new crate for voice-state copy.** The pedagogy pack is already the per-locale i18n single source of truth (prompts, intent text, labels, factual prefixes, vocab/break templates). Adding a sibling crate would split that surface and force `primer-gui` to depend on two crates for what is one concept. The `primer-pedagogy` crate name is historic — it has long since become the i18n-pack crate.
- **`.expect()` rather than fallback-on-error.** Mirrors the existing pattern at `DialogueManager::new` (line 49 of `dialogue_manager/lifecycle.rs`). Embedded packs are validated at build time so a runtime load failure is a structural bug; falling back to English would mask a regression rather than surface it loudly.
- **Empty values are a pack-shape error caught at load, not at render.** Pattern carried forward from how `validate_placeholders` works for the rest of the pack. Consumers render the strings unconditionally — a `match s.is_empty() { ... }` at the render site would invite silent UI bugs.
- **`[voice_state]` is the only new pack section that has BOTH a section-level structural validator AND per-field placeholder validation.** The placeholder validators are kept (despite no placeholders being allowed) because the existing pattern is "every text field goes through `validate_placeholders` with its allowlist" — keeping it consistent costs nothing and means a translator who accidentally drops a `{name}` into a label gets an immediate error.

## What's next

### The most defensible smaller-scope follow-ups

- **Hindi (`hi`) locale pack rollout.** Even more attractive after this PR — `[voice_state]` is now data-only, so adding Hindi means: `tests/common/hi.rs` (define `QUERIES_HI` mirroring the EN/DE shape), parallel `retrieval_quality_hi.rs` (mirror `retrieval_quality_de.rs`), parallel `retrieval_sweep_hi.rs` + `retrieval_sweep_hybrid_hi.rs` (~50-line shims each via `run_bm25_sweep` / `run_hybrid_sweep`), a `WikiSource` preset in `data/ingest/wiki/source.py`, and a children's-vocabulary corpus source. Wikipedia's "बाल विकिपीडिया" (Bal Vikipedia) is the obvious analogue of Klexikon and Simple English — confirm it's actually live and CC-licensed before commitment. Schema + i18n boundary are already locale-keyed; no Rust core changes expected. Voice-state copy is now a six-line TOML append.
- **#92** — voice: `download_voice_assets` needs timeout, resume, and max-size cap. Current implementation has no upper bound on download size and no per-asset timeout; a network hang or hostile-CDN response could lock the consent modal indefinitely. Concrete fix: wrap `reqwest::Client` in a timeout-bearing builder; track `bytes_done` against `approx_size_mb * 1.2 * 1_048_576` as a soft cap; on partial-file resume, send a `Range:` header if the `.partial` exists.

### Larger queue items (carried forward)

- **Local llama.cpp inference (Phase 1.1).** `LlamaCppBackend` stub at `primer-inference` is the entry point.
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality, not production). The voice mode (Phase A) GUI work landed in PR #89 — production polish is the still-open piece. Issue #92 above is the immediate review-fallout follow-up; #91 closes with PR #101.
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default. The relevant flips: `default = ["embedding"]` in `primer-cli/Cargo.toml` and the `--embedder-backend` CLI default.

### Klexikon corpus follow-ups (carried forward)

- **Klexikon corpus expansion past 66.** Klexikon has ~3000 articles. The 2 corpus gaps from the PR #93 session (gänsehaut reflex; tides on the mond article) would need either expanded articles or additional Klexikon titles to lift. Concrete next-batch acceptance: pick 30-50 more titles in still-thin clusters — body could use Atmung-related topics; life could use Ökosystem, Affe/Pferd/Fledermaus; how-things-work could use Computer/Internet/Telefon/Maschine. Re-run pipeline. No code change required. Once corpus grows past ~150 passages, re-run the sweep harnesses to verify production defaults still flatline at 100% strict from `top_k=5` onward on non-paraphrase queries (and that the BM25 floor's defensive 0.5 hasn't been challenged by the larger statistic).
- **Klexikon license claim spot-check.** Concrete acceptance: WebFetch a sample of ~5 article footers (e.g. `Sonne`, `Atome und Moleküle`, `Skelett`, `Bienen`, `Mondfinsternis`) and verify each footer shows CC-BY-SA-4.0. If a per-page divergence appears, document a per-passage license override field in `WikiSource`. Low-priority.

### Smaller-scope follow-ups still open

- **#86** — primer-gui: avoid double session-DB open on resume (enhancement).
- **#87** — primer-gui: end-to-end resume_session test for cross-locale inheritance (enhancement).
- **#80** — GUI: expose Locale::ALL via a Tauri command instead of hand-mirroring it in settings.js (enhancement).
- **#81** — GUI: settings modal needs a focus trap (enhancement).
- **#71** — GUI: tighten CSP before ship (remove `'unsafe-inline'`).
- **#69** — primer-engine: embedder helpers should return Result, not `std::process::exit`.
- **#46** — explore post-RRF `min_score` as a fifth grid axis in the hybrid sweep. (Now lives in the shared helper at `tests/common/sweep.rs`; the change is a one-axis addition to `HybridSweepConfig` + the loop body.)
- **#40** — aggregate per-source attribution row for Wikipedia (and Klexikon).
- **#41** — tighten disambiguation regex if false positives appear.
- **#22** — cache prepared statements for the corpus-bootstrap path (Phase 0.2; enhancement).
- **#21** — separate `--languages` preference list from bound `--language` locale.
- **#20** — i18n placeholder validator can false-fail on translator narrative text.
- **Failed-batch persistence sidecar (issue #38 optional).** Was deferred during brainstorming; file as new issue if scheduled ingest ever ships.
- **Network-error retry.** `requests.exceptions.ConnectionError` / `Timeout` still propagate unchanged.
- **Pre-commit fmt hook (workflow-level).** Carried forward from PR #94. Drift accumulated 13 files across 5 PRs before CI noticed; a local `cargo fmt --check` pre-commit hook would prevent it. Defer until the next time drift happens.
- **`prompt_pack.rs` is now 1313 lines.** Up from 1141 pre-PR. Bulk of additions are tests + the `voice_state` data shape. Still single-purpose but candidate for a future split into `prompt_pack/{mod,sections,validation,tests}.rs` if it grows past ~1500. No action this PR.

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
- IPC trust-boundary covers path + URL attack surface but not download size / timeout (issue #92 follow-up).

**New observations from this session:**

- **`VoiceStateCopy::for_locale` is now a thin clone over `voice_state_labels()`** — six `.clone()` calls of `String`s. The struct could in principle hold `Arc<str>` slices into the pack to skip the clones, but the call site is `get_voice_state_copy`, which fires only on Settings modal open and voice-mode toggle — both off the hot path, with frequencies measured in user-actions-per-minute. Premature.
- **Manual smoke not run yet.** The PR description checklist marks the in-app smoke as `[ ]` deliberately — the changeset is small, well-tested, and pure refactor with byte-identical regression witnesses on both ends of the bridge, but voice mode needs a clean GUI launch under `--features speech` and a toggle to confirm the LISTEN / THINKING / SPEAK indicator strings render byte-identically under both `--language en` and `--language de`. Quick eyeball confirmation; should be done before merge.
- **The pack-pattern is now well-defended.** Three sibling sections (`labels`, `sections`, now `voice_state`) all carry locale-keyed display strings. Adding a fourth — for example, settings-modal strings (a natural #91 follow-on if the settings panel grows GUI-side i18n needs) — would follow the same pattern: data struct in `primer-pedagogy`, accessor on `PromptPack`, validator at load, GUI-side thin clone. No structural changes to the pack-pattern needed; just data adds.
- **Toggle-availability bug was caught only by the manual smoke test, not by any automated suite.** No Rust test could have caught it because the bug lived in `restoreOnLaunch` (JS) reading the wrong field off a JSON IPC payload — both ends of the call were structurally fine. The defensive fix at the Rust layer (a dedicated capability command) makes the right test possible (cfg-flag mirror, now in place), but the regression class — "frontend pulls the right value off the wrong shape" — is uncovered by either Rust unit tests or pack-side cfg-flag tests. A future Playwright-style integration suite would close this; not in scope this PR.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing, with one new pattern reinforced this session.)

- **REINFORCED: Pack-side i18n for any locale-keyed display string the GUI surfaces.** Pattern shape: data struct in `primer-pedagogy::prompt_pack` (e.g. `VoiceStateLabels`), single accessor on `PromptPack` returning `&Struct`, `[section]` table in each `prompts/<pack>.toml`, structural validator at load (no-empty / no-placeholder), GUI-side `Serialize`-flavoured equivalent that calls `prompt_pack::load_cached(locale).expect(...)` and clones the strings. Adding a new locale is a TOML-table append.
- **Server-side re-resolution at IPC trust boundaries.** Pattern shape: when the webview echoes back a payload that includes both *identity* (kind, id, slug) and *resource locators* (path, URL, fd), have the command take only the identity strings and re-resolve the locators server-side via the same resolver that produced them originally. Pair with a `Serialize`-only output type (no `Deserialize`) to make the trust direction structural. Applies wherever IPC has any chance of being compromised — Tauri webviews, web-socket bridges, IPC to renderer processes.
- **Shared test harness with `*Config` carrier struct + locale-specific shim.** Pattern shape: one helper module (`tests/common/sweep.rs`), one config carrier struct per algorithmic shape (`Bm25SweepConfig`, `HybridSweepConfig`), one entrypoint per shape (`run_bm25_sweep`, `run_hybrid_sweep`), thin per-locale shims that fill the config. Output is the public contract — verify with byte-identical diff against pre-refactor baselines, not arbitrary assertions. The same pattern can be applied to the `retrieval_quality{,_de}{,_hybrid,_hybrid_de}` regression tests if they grow a third locale.
- **Pure functions in dedicated modules** for algorithmic cores — tested directly without I/O.
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level.
- **Carrier structs with `disabled()` no-op constructors** for parameter bundles that not all callers need to configure.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed.
- **Constants in `consts.rs` submodules** (Rust) or top-of-module `_DEFAULT_*` constants (Python). No magic numbers anywhere.
- **TDD discipline.** Tests first; watch them fail; implement to green. **Reinforced this session** — the regression witnesses for the pre-refactor strings were written BEFORE touching the production code and stayed green throughout the switchover. The empty-field error-path test was the failing-then-green TDD cycle for the new validator.
- **File-size hygiene.** Keep modules under 500 lines.
- **Network-injection test seam** for any data-ingest pipeline (`http_client` parameter, substitute `FakeHttpClient`/`KlexikonFakeHttpClient` in tests).
- **Subagent-driven development workflow** for plan execution (or inline executing-plans for small TDD-shaped plans).
- **Defensive sanity tests at the data layer.**
- **Always pin `Default` impls of public structs to `consts::*`, with a drift-prevention test.** Mirrored on the Python side as `RetrySettings.default()` ↔ module-level `DEFAULT_*` consts.
- **Pure helpers + their own unit tests, even in `#[ignore]`'d integration tests.**
- **Frozen dataclasses for process-wide configuration** (the `WikiSource` pattern; `RetrySettings` follows the same shape).
- **String discriminators for strategy selection, with allow-list validation.**
- **Re-run the live ingest after changing any fetch-path helper.** Tiny expected diff = success, anything bigger = a flag worth investigating.
- **Kwarg-injected side-effect functions for TDD seams in Python.**
- **Structural ingest-time defences beat manual probing habits.**
- **Back-compat re-export shims when bulk-editing test imports would dilute a structural-refactor PR.**
- **Plan-then-execute inline for mechanical refactors with a strong test safety net.**
- **Two-commit refactor: "set up the change" then "remove the old".** (Most session refactors are small enough for one commit; the two-commit form is for larger PRs.)
- **Ship the follow-up issue body with explicit acceptance + per-file checklist when a PR ships a transitional state.**
- **Per-locale dataset modules under `tests/common/<pack>.rs`** for benchmark data.
- **Locale-specific cluster sizing via a named const** (`DE_CLUSTERS = 5`).
- **Required-loose-substrings drawn from the canonical article's lead, not the query phrasing.** EN-side convention from PR #45; re-confirmed standing in PR #93. Makes the loose check measure retrieval, not vocabulary alignment.

## Exact commands needed to resume

```bash
# Resume on main (after PR #101 merges):
cd /Users/hherb/src/primer
git status                       # confirm clean
git checkout main
git pull
git log --oneline -10            # the PR-101 squash-merge commit should be near the top

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 773 passed, 0 failed, 3 ignored (default features).

~/.cargo/bin/cargo test -p primer-gui --features speech
# Expected: 93 passed, 0 failed, 0 ignored.

~/.cargo/bin/cargo fmt --all -- --check
# Expected: clean.

RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets
# Expected: clean exit 0 (mirrors CI).

# Speech-feature clippy (slower; downloads Tauri's macro deps on first run):
RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech
# Expected: clean exit 0.
```

To exercise the voice-state pack lookup manually:

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run --bin primer-gui --features speech
# In the GUI: toggle voice mode on. The LISTEN / THINKING / SPEAK indicator
# strings should render as before for the configured locale.
# Switch locale via the Settings modal and toggle voice mode again to
# confirm the German strings render correctly.
```

To translator-iterate on the pack copy without recompiling:

```bash
cd /Users/hherb/src/primer/src
PRIMER_PROMPTS_DIR=$PWD/crates/primer-pedagogy/prompts \
    ~/.cargo/bin/cargo run --bin primer-gui --features speech
# Edits to crates/primer-pedagogy/prompts/<pack>.toml are reflected on the
# next `load_cached` call (cache is bypassed when PRIMER_PROMPTS_DIR is set).
```

To re-run the German regression benchmarks (both flavours):

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_de
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_hybrid_de
# Real BGE-M3 recall floor (downloads ~570 MB on first run; cached afterwards):
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_quality_hybrid_de
```

To re-run the German sweep diagnostics (via the shared helper at `tests/common/sweep.rs`):

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
