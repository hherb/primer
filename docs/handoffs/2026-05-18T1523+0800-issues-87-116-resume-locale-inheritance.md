# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-18T1523+0800 (PR #117 in flight closing #87 + #116 — regression guards for PR #115's locale-inheritance contract. PR #115 merged earlier in this session as commit 5717e0b.)

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **839 Rust tests** under default features once PR #117 lands. 3 ignored. Add `--features primer-gui/speech` for **129 primer-gui tests**. Add `--features primer-kb-load/fastembed` for the embedding-backed sweeps + the real-BGE-M3 recall tests. Plus 135 Python tests in `data/ingest/` (unchanged this session — tooling-only work).
3. **Check PR #117's CI status.** If green and unmerged, merge it. If red, the diff is tiny (4 files; +184 / -2 lines; pure additive tests + one new `#[doc(hidden)]` cross-crate test seam) so any failure is localised.
4. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.

## What we shipped this session

### PR #117 (in flight, closes #87 + #116) — regression guards for resume locale inheritance

Two narrow tests that pin the on-disk consequence of PR #115's `SqliteSessionStore::set_locale` contract.

- **#116** ([`primer-storage/src/store/tests/session_tests.rs::set_locale_changes_subsequent_concept_tags`](src/crates/primer-storage/src/store/tests/session_tests.rs)): opens a store under English, inserts a concept via `update_turn_concepts` (tagged `en`), calls `set_locale(German)`, inserts a different concept (tagged `de`), and asserts both rows carry the expected tag. The PR-#115 tests covered the inputs — `store.locale()` returns the new value, the file isn't re-opened — but the longitudinal effect on `concept_language_tag` wasn't directly asserted.
- **#87** ([`primer-gui/src/commands/session.rs::resume_inherits_persisted_locale_end_to_end`](src/crates/primer-gui/src/commands/session.rs)): builds + saves an English session, drops the active session, calls `build_active_session_for_resume` with `cfg.learner.locale = "de"`, and asserts (a) the resumed `DialogueManager.learner.profile.locale` is English (not cfg's German), and (b) a concept inserted post-resume via `session_store.update_turn_concepts` lands tagged `en` in the session DB.
- **New `#[doc(hidden)]` cross-crate test seam** [`primer_storage::__concept_language_tag_for_tests(path, name) -> Option<String>`](src/crates/primer-storage/src/store/mod.rs) — opens a transient read-only `rusqlite::Connection` on the file at `path` and returns the named concept's tag. Mirrors the existing `__session_store_open_count_for_tests` pattern. Lets `primer-gui` inspect on-disk artefacts without adding `rusqlite` as a dev-dep.
- **Branch:** `tests/resume-locale-inheritance-issues-87-116`; head SHA `0126813`.
- **Tests:** **839 passed / 0 failed / 3 ignored** (default features; was 837 / 0 / 3). primer-gui with `--features speech`: **129 / 0 / 0** (was 128). fmt + clippy `-D warnings` clean on both default and `--features primer-gui/speech` builds.
- **No README.md or ROADMAP.md update needed** — internal regression guards; no user-visible behaviour change.

### Earlier in this session (already merged before #117 opened)

- **PR #115** — collapse `resume_session` into a single session-DB open (closes #86). Merged as commit `5717e0b`. The pre-#86 path opened the DB twice (`probe_learner_locale` + `build_active_session`); the new path reorders construction so the session DB opens first, the learner row is read inline, and (on locale mismatch) the store's locale field is re-tagged in place via `SqliteSessionStore::set_locale`. New public API: `build_active_session_for_resume` in `primer-gui/src/wiring.rs`. New `SqliteSessionStore::set_locale` method. Thread-local open counter `primer_storage::session_store_open_count` for behavioural pinning.

## What's next

### Immediate (this session's in-flight)

- **PR #117 (closes #87 + #116)** — verify CI green and merge. The diff is tiny (4 files; +184 / -2 lines; pure test additions + one new `#[doc(hidden)]` test seam) so nothing user-visible should drift. If CI surfaces an unexpected failure, the most likely culprits are a stale clippy lint or some test-only seam discoverability issue — both fix-locally-and-push fast.

### macOS-native speech follow-ups (opened during the #110 / #111 PRs; not touched this session)

- **#114** — speech(macos-native): stream PCM chunks to speaker as `AVSpeechSynthesizer` emits them (cut time-to-first-audio). Larger; touches the synthesis path. The current path buffers the full utterance before pushing to cpal; streaming would let the user hear the start of the response sooner.
- **#112** — cli(macos-native): `--speech` with `--speech-backend macos-native` still requires dummy `--whisper-model`/`--voice-onnx`/`--voice-config` paths. Clap-level UX fix — make those flags conditional on the speech-backend selection.

### Hindi locale follow-ups (carried forward — not touched this session)

- **Native-speaker review of `prompts/hi.toml`.** Grep `# REVIEW:` for the blocks flagged for review. Critical items: tense register (तुम vs. आप), age-band vocabulary markers (तत्सम / Sanskrit-rooted vocabulary), factual-prefix list (Hindi syntax places question words at the end so prefix-matching is weak — consider setting `factual_prefixes = []` and relying entirely on the LLM-engagement-classifier path), `[voice_state]` UI copy (cramped in Devanagari).
- **Hindi children's-vocabulary corpus.** Three candidate sources documented in `docs/localisation/hi/README.md`:
  - **NCERT textbooks** ([ncert.nic.in](https://ncert.nic.in/)) — Indian government textbooks; "free to use for educational purposes" claim needs spot-checking.
  - **Pratham Books StoryWeaver** ([storyweaver.org.in](https://storyweaver.org.in/)) — CC-BY on most books but varies per book; ingest needs per-book license check.
  - **Wikisource Hindi** ([hi.wikisource.org](https://hi.wikisource.org/)) — children's literature; mostly literary, not encyclopedic.
- **`tests/common/hi.rs`** + retrieval-quality / sweep tests for `hi` once a corpus lands.
- **Real-LLM smoke** against `--backend cloud --language hi` and at least three local Ollama models. Populate `docs/locale/models/HINDI.md`.
- **The flip-to-stable PR** when the above are ready: edit `[meta] status = "stable"` in `hi.toml` + add `Self::Hindi` to `Locale::ALL` + remove `# REVIEW:` markers + drop the preview-banner section from `hi/README.md`. Single commit.

### OpenAI-compat backend follow-ups (carried forward)

- **Real-server smoke testing.** Spin up oMLX (Apple Silicon MLX-native server) and one of {LM Studio, vLLM, llama.cpp `--server`}; run `--backend openai-compat --openai-compat-url http://localhost:8000 --model <model>` against each; confirm SSE streaming, error classification, and embedder round-trip. Particularly check the Apple-Silicon throughput claim (the spec cites 20–40% gains via MLX vs. Ollama on the same hardware).
- **GUI wiring.** The spec scopes GUI wiring as a deferred follow-up; today the OpenAI-compat backend is reachable only via the CLI. A future PR should mirror the existing `--backend ollama` / `--backend cloud` GUI surface (settings modal + backend dispatch in `primer-engine`'s GUI consumer) for the new backend.
- **Model evaluation page.** A `docs/openai-compat-models.md` or extension to existing per-locale model pages could track which models behave well behind which servers.

### Carried-forward larger items

- **Branch-protection-on-main remains the structural fix** that PR #109 set up the local-hook layer for. To close the gap at the merge boundary, the repo owner needs to flip a GitHub setting: Settings → Branches → Add rule for `main` → require status check `cargo test (default features)` → require branches up to date before merge. One-time UI click; not a code change.
- **Local llama.cpp inference (Phase 1.1).** `LlamaCppBackend` stub at `primer-inference` is still the entry point. The OpenAI-compat path partially obviates this since llama.cpp's `--server` is already reachable via the new backend, but a direct llama.cpp embedding (without the HTTP hop) remains the long-term Phase 1 goal.
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality). Voice mode (Phase A) GUI work landed in PR #89; #91 closed with PR #101; #92 closed with PR #104; #102 closed with PR #110. The remaining Phase 2 polish is the still-open piece — and #114 / #112 expand that area.
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default.

### Klexikon corpus follow-ups (carried forward)

- **Klexikon corpus expansion past 66.** Klexikon has ~3000 articles. The 2 corpus gaps from PR #93 (`gänsehaut` reflex; tides on the `mond` article) would need either expanded articles or additional Klexikon titles. Concrete next-batch acceptance: pick 30-50 more titles in still-thin clusters. Re-run pipeline. No code change required. Once corpus grows past ~150 passages, re-run the sweep harnesses to verify production defaults still flatline at 100% strict from `top_k=5` onward on non-paraphrase queries.
- **Klexikon license claim spot-check.** WebFetch a sample of ~5 article footers and verify each shows CC-BY-SA-4.0. Low-priority.

### Smaller-scope follow-ups still open

Verified against `gh issue list` 2026-05-18 (#86 closed by PR #115; #87 + #116 closure pending PR #117 merge):

- **#114** — voice(macos-native): stream PCM chunks to speaker as AVSpeechSynthesizer emits them.
- **#112** — cli(macos-native): `--speech` still requires dummy `--whisper-model`/`--voice-onnx`/`--voice-config` paths.
- **#103** — voice: cancel-and-retry path drops the first half of the transcript (bug, voice-loop hardening territory).
- **#98** — refactor(tests): split `tests/common/sweep.rs` into `bm25`/`hybrid` submodules (enhancement). **Defer until Hindi or another third locale lands** — issue body explicitly recommends this.
- **#81** — GUI: settings modal needs a focus trap (enhancement).
- **#71** — GUI: tighten CSP before ship (remove `'unsafe-inline'`).
- **#46** — Hybrid sweep: explore post-RRF `min_score` as a fifth grid axis.
- **#41** — data/ingest: consider scoping disambiguation regex to lead-sentence patterns.
- **#40** — data/ingest: aggregate per-source attribution for the Wikipedia layer.
- **#22** — primer-knowledge: cache prepared statements for the corpus-bootstrap path (Phase 0.2; enhancement).
- **#21** — CLI: separate `--languages` preference list from bound `--language` locale.
- **#20** — i18n: placeholder validator can false-fail on translator narrative text.

### Out-of-issue-tracker follow-ups still standing

- **Failed-batch persistence sidecar (issue #38 optional follow-up).**
- **Network-error retry on Python ingest side.**
- **Probe-function duplication between CLI and GUI.** `primer-cli/src/main.rs::probe_espeak_ng_data` and `primer-gui/src/lib.rs::probe_espeak_ng_data` carry byte-identical logic except for the log channel. Low-priority — move shared impl to `primer-speech` if either side needs to diverge.

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

Carried over from PR #115's brief, still pending:

- **Open-counter is thread-local, not global.** The `session_store_open_count` test seam relies on `#[tokio::test]`'s default `current_thread` flavour: all `open_for_locale` calls within one test happen on the same OS thread. A future test that opts into `flavor = "multi_thread"` and opens session DBs from `spawn_blocking` will see the counter reset to 0 on the other thread. If you hit this, either pin the test to current_thread, or replace the thread-local with `serial_test` and a process-wide counter.
- **`SqliteSessionStore::set_locale` is mutable by reference.** Future code that holds the store as `Arc<dyn LearnerStore>` (most consumers do) cannot call `set_locale` — the mutation happens BEFORE the `Arc::new` wrap in `build_with_strategy`, which is the only intended caller. If a future refactor moves the Arc wrap earlier, the locale-inheritance path will silently break. **PR #117 now pins this with the end-to-end test** — `resume_inherits_persisted_locale_end_to_end` would fail loudly if the wrap moved.

**New for this session — minor risks:**

- **`__concept_language_tag_for_tests` opens a sibling rusqlite::Connection** on the file. If a test calls it while another connection holds an EXCLUSIVE lock (e.g. mid-write transaction), the open could fail or block. In practice this is fine because all current call sites drop the writing store first; if a future caller queries DURING a live session, a `SHARED` open would work but the read-uncommitted concern would apply. The function silently returns `None` on open failure rather than panicking, which preserves the "use only after drop" contract.
- **Manual real-LLM smoke for Hindi and OpenAI-compat has not run.** Recommended:
  - Hindi: `~/.cargo/bin/cargo run --bin primer -- --backend cloud --language hi --name Aarav --age 9 --no-persist --verbose`. A few child-style Hindi prompts via stdin. Document any obvious translation register issues by appending to `docs/localisation/hi/README.md`'s "Open items" or to `docs/locale/models/HINDI.md`.
  - OpenAI-compat: spin up `oMLX --serve` or `llama-server --port 8000 --model <gguf>`; `~/.cargo/bin/cargo run --bin primer -- --backend openai-compat --openai-compat-url http://localhost:8000 --model <model> --no-persist --verbose`. Confirm streaming works, error path handles a deliberately-bad URL, embedder round-trip via `--embedder-backend openai-compat`.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing; new entries from this session at the top.)

- **`#[doc(hidden)] pub` cross-crate test seams in `primer-storage`** to avoid pulling rusqlite into consumer dev-deps (issues #87 + #116). When a downstream test crate needs to inspect on-disk SQLite state, prefer adding a narrow `__name_for_tests` free function in `primer-storage` — which already owns the schema and the rusqlite dep — over adding rusqlite as a dev-dep of the consumer. The `__` prefix and `#[doc(hidden)]` attribute keep it out of the public surface. `__session_store_open_count_for_tests` (counter) and `__concept_language_tag_for_tests` (column query) are the two existing instances; both gate access to internal state for cross-crate behavioural pins.
- **Pin the on-disk consequence, not just the in-memory inputs** (issue #116). When a method's purpose is to influence a future-state on-disk artefact (here: `set_locale` → next `update_turn_concepts` insert), a test that asserts the future-state directly (query the column value) is more durable than a test that only asserts the inputs to that future state (the locale field). Both layers are valuable; the on-disk pin catches a refactor that breaks the chain anywhere between input and output.
- **Thread-local counters as test seams for behavioural pin tests** (issue #86). When a test needs to count side-effectful operations (file opens, network calls) and `cargo test` runs in parallel, a thread-local counter beats a process-wide atomic. `#[tokio::test]` defaults to `current_thread` so all in-test calls share the OS thread. Production code never consults the counter. The trade-off: tests that opt into `flavor = "multi_thread"` plus `spawn_blocking` see a reset; pin the discipline in the counter's doc comment so the next reader knows.
- **Reorder construction to fold redundant probes into the build path** (issue #86). When a code path opens a resource just to read one field, then opens it again to build the real object, the cleanest fix is reordering the build: open the resource first, read the field, then continue construction conditional on what the field says. This pattern beats both "cache the probe" and "extract a shared opener" because there's no caching invariant to maintain and no shared mutable state.
- **`set_locale`-style re-tag methods when the resource itself is locale-neutral** (issue #86). The session-DB's `concepts` table carries locale as a column tag, not a per-locale table. So the in-memory store's locale field can be re-tagged without re-opening the connection. Discipline: name the method `set_locale` (not `with_locale`) and take `&mut self`, so consumers immediately see it's a state mutation. Distinct from the KB side where re-tag is impossible because `passages_<pack>` are separate tables.
- **Opt-in version-controlled git hooks under `.githooks/`.** When adding a pre-commit / pre-push check, put it under `.githooks/` and document `git config core.hooksPath .githooks` in CLAUDE.md. Two reasons: hooks become version-controlled and reviewable; opt-in keeps the install path explicit for contributors who don't want hooks.
- **CI as source of truth; local hooks as early-warning copies.** When duplicating a check across CI and local hooks, the CI step is the canonical enforcer (it runs unconditionally on every push). The hook is an early-warning copy — if it ever drifts from CI, fix the hook to match, not vice versa.
- **Resolve binary tools via $ENVVAR → known install path → PATH.** Mirrors CLAUDE.md's "always invoke as `~/.cargo/bin/cargo`" guidance.
- **Single source of truth at the IPC trust boundary** (PR #108). When the GUI mirrors a Rust enum's contents in JS, prefer a server-side metadata command (return the data) over hand-mirroring.
- **Verify before claiming closed.** When a prior PR's commit message says "closes #X, #Y" but the PR was only scoped to close #X, audit #Y's acceptance criteria against current `main` before closing.
- **Co-locate workflow-level policies with the steps that enforce them.** A `RUSTFLAGS: -D warnings` at the top of `ci.yml` is invisible at the failure point.
- **TDD-driven validator extension.** Add the failing test → watch it fail → land the validator change → land the consumer (data file or producer site).
- **Subagent-driven development with two-stage review (spec + code-quality) per task.**
- **Promote modules that have outgrown their original location.** `locale_defaults` is the model — when a module's deps are narrower than its host module's, promotion is a net positive.
- **Two-firewall preview gates for safety-critical opt-outs.** `Locale::ALL` exclusion + `[meta] status = "preview"` is overkill for low-stakes flags but exactly right for "this could reach a child and they wouldn't know it's machine-translated".
- **In-process `tokio::net::TcpListener` for HTTP behavior tests.**
- **Borrowed client / `FnMut` callback test seam for async streaming.**
- **Pack-side i18n for any locale-keyed display string the GUI surfaces.**
- **Server-side re-resolution at IPC trust boundaries.**
- **Shared test harness with `*Config` carrier struct + locale-specific shim.**
- **Pure functions in dedicated modules** for algorithmic cores — tested directly without I/O.
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level.
- **Carrier structs with `disabled()` no-op constructors** for parameter bundles that not all callers need to configure.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed.
- **Constants in `consts.rs` submodules** (Rust) or top-of-module `_DEFAULT_*` constants (Python).
- **TDD discipline.** Tests first; watch them fail; implement to green.
- **File-size hygiene.** Keep modules under 500 lines where reasonable.
- **Network-injection test seam** for any data-ingest pipeline.
- **Defensive sanity tests at the data layer.**
- **Always pin `Default` impls of public structs to `consts::*`, with a drift-prevention test.**
- **Pure helpers + their own unit tests, even in `#[ignore]`'d integration tests.**
- **Frozen dataclasses for process-wide configuration.**
- **String discriminators for strategy selection, with allow-list validation.**
- **Re-run the live ingest after changing any fetch-path helper.**
- **Kwarg-injected side-effect functions for TDD seams in Python.**
- **Structural ingest-time defences beat manual probing habits.**
- **Plan-then-execute inline for mechanical refactors with a strong test safety net.**
- **Two-commit refactor: "set up the change" then "remove the old".**
- **Ship the follow-up issue body with explicit acceptance + per-file checklist when a PR ships a transitional state.**
- **Per-locale dataset modules under `tests/common/<pack>.rs`** for benchmark data.
- **Locale-specific cluster sizing via a named const** (`DE_CLUSTERS = 5`).
- **Required-loose-substrings drawn from the canonical article's lead, not the query phrasing.**

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer
git status                       # confirm clean (or on the `tests/resume-locale-inheritance-issues-87-116` branch if reviewing PR #117)
git checkout main
git pull
git log --oneline -10            # 5717e0b (PR #115) at top until #117 merges

# Check PR #117 status; merge if green.
gh pr checks 117
gh pr view 117

# Opt-in to the local pre-commit hook (one-time per clone; from PR #109):
git config core.hooksPath .githooks

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 839 passed, 0 failed, 3 ignored (default features).

~/.cargo/bin/cargo test -p primer-gui --features speech
# Expected: 129 passed, 0 failed, 0 ignored.

~/.cargo/bin/cargo fmt --all -- --check
# Expected: clean.

~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
# Expected: clean exit 0.

RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo test --workspace --no-fail-fast
# Expected: 839 passed.

RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech
# Expected: clean exit 0 (the speech-features build is not yet on CI; verify locally).
```

To exercise the Hindi preview locale manually:

```bash
cd /Users/hherb/src/primer/src
RUST_LOG=primer::prompt_pack=warn,info ~/.cargo/bin/cargo run --bin primer -- \
    --backend stub --name Aarav --age 8 --language hi --no-persist
# Expected: one WARN line "prompt pack is in preview status — machine-translated content
# awaiting native-speaker review ... locale=hi" before the first turn. Session runs;
# type "bye" to end. Stub backend gives an English canned response (it doesn't read the
# system prompt) — this is correct; the Hindi pack-loading path is what we're verifying.
```

For a manual GUI smoke (locale picker):

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run -p primer-gui --features speech
# Open Settings; confirm:
#   - Locale dropdown lists "English (en)" and "Deutsch (de)" (in that order)
#   - Speech section lists override cards for both EN and DE
#   - Hindi is absent from both
```

For a real-LLM Hindi smoke (recommended before flipping to stable):

```bash
cd /Users/hherb/src/primer/src
ANTHROPIC_API_KEY=... RUST_LOG=info ~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name Aarav --age 9 --language hi --no-persist --verbose 2>&1 | tee /tmp/smoke_hi.log
```

For an OpenAI-compat smoke (spin up a local server first, e.g. llama-server):

```bash
# In one terminal:
llama-server --port 8000 --model /path/to/some.gguf

# In another:
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run --bin primer -- \
    --backend openai-compat --openai-compat-url http://localhost:8000 \
    --model <model-id-from-server> \
    --name SmokeTester --age 9 --no-persist --verbose
```

To re-run the German regression benchmarks (both flavours; unchanged this session):

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_de
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_hybrid_de
# Real BGE-M3 recall floor (downloads ~570 MB on first run; cached afterwards):
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_quality_hybrid_de
```

To re-run the sweep diagnostics:

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
.venv/bin/pytest tests/
# Expected: 135 passed.
```

For the speech build path: `~/.cargo/bin/cargo build --workspace --features primer-cli/speech`.

For the embedding feature build path:

```bash
~/.cargo/bin/cargo build --workspace --features primer-cli/embedding
~/.cargo/bin/cargo run --bin primer -- --embedder-backend fastembed ...
# First run downloads BGE-M3 (~570 MB) into the fastembed cache.
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If you discover that Horst did interim work that changes the plan, flag it. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.
