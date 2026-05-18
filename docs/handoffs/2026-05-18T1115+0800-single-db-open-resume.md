# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-18T1115+0800 (PR #115 in flight closing #86 — collapse `resume_session` into a single session-DB open. PRs #109, #110, #111 merged earlier under prior sessions.)

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **837 Rust tests** under default features. 3 ignored. Add `--features primer-gui/speech` for **128 primer-gui tests** including the voice-mode integration coverage. Add `--features primer-kb-load/fastembed` for the embedding-backed sweeps + the real-BGE-M3 recall tests. Plus 135 Python tests in `data/ingest/` (unchanged this session — tooling-only work).
3. **Check PR #115's CI status.** If green and unmerged, merge it. If red, inspect the failing step — the change is narrow (4 files; 1 refactor pass + 2 new public symbols + 1 internal removal) so diagnosis is localised.
4. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.

## What we shipped this session

### PR #115 (in flight, closes #86) — single session-DB open on resume

- **Collapses the pre-#86 resume_session path** from two `SqliteSessionStore::open_for_locale` calls into one. The pre-#86 path called `probe_learner_locale` (open DB → load_learner → drop) and then `wiring::build_active_session` (open same DB again to build the real `ActiveSession`); both opens ran the idempotent schema-migration chain + lookup-table validation. The new path reorders construction so the session DB opens first, the learner row is read inline, and (on locale mismatch) the store's locale field is re-tagged in place via a new `SqliteSessionStore::set_locale` method.
- **New public API in `primer-gui/src/wiring.rs`:**
  - `build_active_session_for_resume(home, cfg) -> Result<ActiveSession, String>` — the inheritance-on-mismatch variant; used by `commands::session::resume_session`.
  - `build_active_session(home, cfg)` keeps its strict `LocaleStrategy::UseCfg` semantics (start-session path; hard-error on cfg/persisted-locale mismatch to preserve the longitudinal `concept_language_tag` discipline from CLAUDE.md's locale-is-per-learner gotcha).
  - Internal `LocaleStrategy` enum + private `build_with_strategy` helper hold the shared body — one place to maintain.
- **New `SqliteSessionStore::set_locale`** at [crates/primer-storage/src/store/mod.rs:174](src/crates/primer-storage/src/store/mod.rs#L174) — re-tag the in-memory store without re-opening the SQLite file. The session-side schema is locale-neutral (single `concepts` table; locale lives in a column), so re-tag is safe; the KB side stays at re-open-required because per-locale `passages_<pack>` tables make re-opening mandatory.
- **Thread-local open counter** `primer_storage::session_store_open_count()` for behavioural pinning. Thread-local — not a process-wide `AtomicUsize` — because `cargo test` runs in parallel across OS threads and a global counter races; `#[tokio::test]` defaults to a `current_thread` runtime so all opens within one test happen on the same OS thread. Production code does not consult the counter.
- **Removed:** `commands/session.rs::probe_learner_locale` (40 lines) + its dedicated test (replaced by `resume_helper_inherits_persisted_locale_on_mismatch` at the public-API level).
- **Branch:** `gui/resume-single-db-open-issue-86`; head SHA `06276a8`.
- **Tests:** **837 passed / 0 failed / 3 ignored** (default features; was 830 / 0 / 3 — +5 in `primer-gui`, +2 in `primer-storage`). primer-gui with `--features speech`: **128 / 0 / 0**. Fmt + clippy `-D warnings` clean on both default and `--features primer-gui/speech` builds.
- **No README.md or ROADMAP.md update needed** — internal refactor; no user-visible behaviour change.

## What's next

### Immediate (this session's in-flight)

- **PR #115 (closes #86)** — verify CI green and merge. The diff is narrow (4 files; +324 / -138 lines); nothing user-visible should drift. If CI surfaces an unexpected failure, the most likely culprits are a stale clippy lint on the new code or a timing-sensitive test elsewhere — fix locally and push.

### Newly-opened post-brief issues to consider

These were opened in the macOS-native speech work (PRs #110, #111) and have not been touched:

- **#114** — speech(macos-native): stream PCM chunks to speaker as AVSpeechSynthesizer emits them (cut time-to-first-audio). Larger; touches the synthesis path. The current path buffers the full utterance before pushing to cpal; streaming would let the user hear the start of the response sooner.
- **#112** — cli(macos-native): `--speech` with `--speech-backend macos-native` still requires dummy `--whisper-model`/`--voice-onnx`/`--voice-config` paths. Clap-level UX fix — make those flags conditional on the speech-backend selection.

### Hindi locale follow-ups (carried forward — not touched this session)

- **Native-speaker review of `prompts/hi.toml`.** Grep `# REVIEW:` for the blocks flagged for review. Critical items: tense register (तुम vs. आप), age-band vocabulary markers (तत्सम / Sanskrit-rooted vocabulary), factual-prefix list (Hindi syntax places question words at the end so prefix-matching is weak — consider setting `factual_prefixes = []` and relying entirely on the LLM-engagement-classifier path), `[voice_state]` UI copy (cramped in Devanagari).
- **Hindi children's-vocabulary corpus.** Three candidate sources documented in `docs/localisation/hi/README.md`:
  - **NCERT textbooks** ([ncert.nic.in](https://ncert.nic.in/)) — Indian government textbooks; "free to use for educational purposes" claim needs spot-checking.
  - **Pratham Books StoryWeaver** ([storyweaver.org.in](https://storyweaver.org.in/)) — CC-BY on most books but varies per book; ingest needs per-book license check.
  - **Wikisource Hindi** ([hi.wikisource.org](https://hi.wikisource.org/)) — children's literature; mostly literary, not encyclopedic.
- **`tests/common/hi.rs`** + retrieval-quality / sweep tests for `hi` once a corpus lands.
- **Real-LLM smoke** against `--backend cloud --language hi` and at least three local Ollama models. Populate `docs/locale/models/HINDI.md`.
- **The flip-to-stable PR** when the above are ready: edit `[meta] status = "stable"` in `hi.toml` + add `Self::Hindi` to `Locale::ALL` + remove `# REVIEW:` markers + drop the preview-banner section from `hi/README.md`. Single commit. **Side benefit of PR #108:** the GUI dropdown picks up the new entry automatically — no JS edit needed.

### OpenAI-compat backend follow-ups (carried forward)

- **Real-server smoke testing.** Spin up oMLX (Apple Silicon MLX-native server) and one of {LM Studio, vLLM, llama.cpp `--server`}; run `--backend openai-compat --openai-compat-url http://localhost:8000 --model <model>` against each; confirm SSE streaming, error classification, and embedder round-trip. Particularly check the Apple-Silicon throughput claim (the spec cites 20–40% gains via MLX vs. Ollama on the same hardware).
- **GUI wiring.** The spec scopes GUI wiring as a deferred follow-up; today the OpenAI-compat backend is reachable only via the CLI. A future PR should mirror the existing `--backend ollama` / `--backend cloud` GUI surface (settings modal + backend dispatch in `primer-engine`'s GUI consumer) for the new backend.
- **Model evaluation page.** A `docs/openai-compat-models.md` or extension to existing per-locale model pages could track which models behave well behind which servers.

### Carried-forward larger items

- **Branch-protection-on-main remains the structural fix** that PR #109 set up the local-hook layer for. To close the gap at the merge boundary, the repo owner needs to flip a GitHub setting: Settings → Branches → Add rule for `main` → require status check `cargo test (default features)` → require branches up to date before merge. One-time UI click; not a code change.
- **Local llama.cpp inference (Phase 1.1).** `LlamaCppBackend` stub at `primer-inference` is still the entry point. The OpenAI-compat path partially obviates this since llama.cpp's `--server` is already reachable via the new backend, but a direct llama.cpp embedding (without the HTTP hop) remains the long-term Phase 1 goal.
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality). The voice mode (Phase A) GUI work landed in PR #89; #91 closed with PR #101; #92 closed with PR #104; #102 closed with PR #110. The remaining Phase 2 polish is the still-open piece — and #114 / #112 expand that area.
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default.

### Klexikon corpus follow-ups (carried forward)

- **Klexikon corpus expansion past 66.** Klexikon has ~3000 articles. The 2 corpus gaps from PR #93 (`gänsehaut` reflex; tides on the `mond` article) would need either expanded articles or additional Klexikon titles. Concrete next-batch acceptance: pick 30-50 more titles in still-thin clusters. Re-run pipeline. No code change required. Once corpus grows past ~150 passages, re-run the sweep harnesses to verify production defaults still flatline at 100% strict from `top_k=5` onward on non-paraphrase queries.
- **Klexikon license claim spot-check.** WebFetch a sample of ~5 article footers and verify each shows CC-BY-SA-4.0. Low-priority.

### Smaller-scope follow-ups still open

Verified against `gh issue list` 2026-05-18 (#86 closure pending PR #115 merge; #96, #102 closed earlier):

- **#114** (new) — voice(macos-native): stream PCM chunks to speaker as AVSpeechSynthesizer emits them.
- **#112** (new) — cli(macos-native): `--speech` still requires dummy `--whisper-model`/`--voice-onnx`/`--voice-config` paths.
- **#103** — voice: cancel-and-retry path drops the first half of the transcript (bug, voice-loop hardening territory).
- **#98** — refactor(tests): split `tests/common/sweep.rs` into `bm25`/`hybrid` submodules (enhancement). **Defer until Hindi or another third locale lands** — issue body explicitly recommends this.
- **#87** — primer-gui: end-to-end resume_session test for cross-locale inheritance (enhancement). **Worth a quick look now** — the public API surface this PR introduces (`build_active_session_for_resume`) is the natural integration test surface for #87. If you pick this up next, write the test against the new helper directly.
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

**New for this session — minor risks:**

- **Open-counter is thread-local, not global.** The `session_store_open_count` test seam relies on `#[tokio::test]`'s default `current_thread` flavour: all `open_for_locale` calls within one test happen on the same OS thread. A future test that opts into `flavor = "multi_thread"` and opens session DBs from a `tokio::task::spawn_blocking` (or any other tokio path that escapes the test's calling thread) will see the counter reset to 0 on the other thread. If you hit this, either pin the test to current_thread, or replace the thread-local with `serial_test` and a process-wide counter.
- **`SqliteSessionStore::set_locale` is mutable by reference.** Future code that holds the store as `Arc<dyn LearnerStore>` (most consumers do) cannot call `set_locale` — the mutation happens BEFORE the `Arc::new` wrap in `build_with_strategy`, which is the only intended caller. If a future refactor moves the Arc wrap earlier, the locale-inheritance path will silently break. Pin discipline: keep `Arc::new(session_store)` strictly AFTER any potential `set_locale` call.

**Carried over from the prior brief, still pending:**

- **Manual real-LLM smoke for Hindi and OpenAI-compat has not run.** Recommended:
  - Hindi: `~/.cargo/bin/cargo run --bin primer -- --backend cloud --language hi --name Aarav --age 9 --no-persist --verbose`. A few child-style Hindi prompts via stdin. Document any obvious translation register issues by appending to `docs/localisation/hi/README.md`'s "Open items" or to `docs/locale/models/HINDI.md`.
  - OpenAI-compat: spin up `oMLX --serve` or `llama-server --port 8000 --model <gguf>`; `~/.cargo/bin/cargo run --bin primer -- --backend openai-compat --openai-compat-url http://localhost:8000 --model <model> --no-persist --verbose`. Confirm streaming works, error path handles a deliberately-bad URL, embedder round-trip via `--embedder-backend openai-compat`.
- **The preview-locale pattern is now established.** The `[meta] status = "preview"` field + `Locale::ALL` exclusion is the canonical way to land a new locale without exposing it to end users prematurely. Future locales (Spanish, Tamil, Bengali, …) should follow this two-firewall pattern, including the `# REVIEW:` markers in the prompt pack. Since PR #108, the GUI picker is now wired to `Locale::ALL` automatically — adding a stable locale is purely a Rust edit; the JS no longer needs to be touched.
- **`locale_defaults` is at the crate root.** Any future code that imports it should use `primer_speech::locale_defaults::*` directly. Grep `grep -rn "voice_loop::locale_defaults" src/crates --include="*.rs"` should always return zero.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing; new entries from this session at the top.)

- **Thread-local counters as test seams for behavioural pin tests** (issue #86). When a test needs to count side-effectful operations (file opens, network calls) and `cargo test` runs in parallel, a thread-local counter beats a process-wide atomic. `#[tokio::test]` defaults to `current_thread` so all in-test calls share the OS thread. Production code never consults the counter. The trade-off: tests that opt into `flavor = "multi_thread"` plus `spawn_blocking` see a reset; pin the discipline in the counter's doc comment so the next reader knows.
- **Reorder construction to fold redundant probes into the build path** (issue #86). When a code path opens a resource just to read one field, then opens it again to build the real object, the cleanest fix is reordering the build: open the resource first, read the field, then continue construction conditional on what the field says. This pattern beats both "cache the probe" and "extract a shared opener" because there's no caching invariant to maintain and no shared mutable state.
- **`set_locale`-style re-tag methods when the resource itself is locale-neutral** (issue #86). The session-DB's `concepts` table carries locale as a column tag, not a per-locale table. So the in-memory store's locale field can be re-tagged without re-opening the connection. Discipline: name the method `set_locale` (not `with_locale`) and take `&mut self`, so consumers immediately see it's a state mutation. Distinct from the KB side where re-tag is impossible because `passages_<pack>` are separate tables.
- **Opt-in version-controlled git hooks under `.githooks/`.** When adding a pre-commit / pre-push check, put it under `.githooks/` and document `git config core.hooksPath .githooks` in CLAUDE.md. Two reasons: hooks become version-controlled and reviewable; opt-in keeps the install path explicit for contributors who don't want hooks. Don't auto-install via a setup script; intrusive auto-installation defeats the trust model. Pair it with the structural fix (branch protection / CI required-status-check) that closes the gap regardless of contributor discipline.
- **CI as source of truth; local hooks as early-warning copies.** When duplicating a check across CI and local hooks, the CI step is the canonical enforcer (it runs unconditionally on every push). The hook is an early-warning copy — if it ever drifts from CI, fix the hook to match, not vice versa. Stating this explicitly in the hook's header comment keeps the relationship clear for future readers.
- **Resolve binary tools via $ENVVAR → known install path → PATH.** Mirrors CLAUDE.md's "always invoke as `~/.cargo/bin/cargo`" guidance. Scripts that shell out to toolchain binaries should follow the same order so a Homebrew shim can't silently win.
- **Single source of truth at the IPC trust boundary.** (PR #108) When the GUI mirrors a Rust enum's contents in JS, prefer a server-side metadata command (return the data) over hand-mirroring. PR #108 is the canonical example: `list_locales` returns `Locale::ALL`, and the JS no longer encodes the locale list at all.
- **Verify before claiming closed.** When a prior PR's commit message says "closes #X, #Y" but the PR was only scoped to close #X, audit #Y's acceptance criteria against current `main` before closing.
- **Co-locate workflow-level policies with the steps that enforce them.** A `RUSTFLAGS: -D warnings` at the top of `ci.yml` is invisible at the failure point. The same flag as a step-local env (or as `cargo ... -- -D warnings`) makes the failure self-describing without changing behaviour.
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
git status                       # confirm clean (or on the `gui/resume-single-db-open-issue-86` branch if reviewing PR #115)
git checkout main
git pull
git log --oneline -10            # 8daa116 (PR #111) at top until #115 merges

# Check PR #115 status; merge if green.
gh pr checks 115
gh pr view 115

# Opt-in to the local pre-commit hook (one-time per clone; from PR #109):
git config core.hooksPath .githooks

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 837 passed, 0 failed, 3 ignored (default features).

~/.cargo/bin/cargo test -p primer-gui --features speech
# Expected: 128 passed, 0 failed, 0 ignored.

~/.cargo/bin/cargo fmt --all -- --check
# Expected: clean.

~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
# Expected: clean exit 0.

RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo test --workspace --no-fail-fast
# Expected: 837 passed.

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
