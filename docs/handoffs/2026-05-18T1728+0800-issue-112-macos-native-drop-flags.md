# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-18T1728+0800 (PR-in-flight closing #112 — drop the dummy whisper/piper flag requirement on the macOS-native build. PR #117 closing #87 + #116 merged earlier in the day as commit `413a1df`.)

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **839 Rust tests** under default features (unchanged — the two new tests are gated to the speech feature, so they live behind `--features speech` or `--features speech,macos-native`). 3 ignored. With `--features primer-gui/speech`: **129 primer-gui tests**. With `--features primer-cli/speech`: primer-cli now has **12 tests** (+1 from the new `speech_alone_still_rejected_off_macos_native`); same count under `--features primer-cli/speech,primer-cli/macos-native` (the `speech_alone_parses_on_macos_native_without_whisper_piper_flags` test replaces the off-native variant under that build). Plus 135 Python tests in `data/ingest/` (unchanged this session).
3. **Check this PR's CI status.** If green and unmerged, merge it. If red, the diff is narrow (4 files; +130 / −39 lines; one cfg-gate pattern applied across the four whisper/piper flags + `SpeechLoopConfig` field set + `validate_speech_assets` function + the speech branch in `main.rs`). The failure mode is most likely a cfg-gate the patch missed in CI's feature combination — easy to localise.
4. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.

## What we shipped this session

### PR (in flight, closes #112) — drop dummy whisper/piper flags on the macOS-native build

Until this PR, `--features primer-cli/speech,primer-cli/macos-native` on macOS still forced users to pass `--whisper-model /dev/null --voice-onnx /dev/null --voice-config /dev/null --voice ignored` — clap's `requires_all` insisted on them, then `speech_loop/mod.rs::run` discarded every value at runtime and emitted a `dead_code` warning on the four corresponding `SpeechLoopConfig` fields. The PR cfg-gates the four CLI flags AND the matching `SpeechLoopConfig` fields under `not(all(target_os = "macos", feature = "macos-native"))`, so they disappear entirely from the CLI surface on the macOS-native build.

- **`Cli::speech`** now uses `#[cfg_attr]` to switch the clap `requires_all` between two payloads: on the macOS-native build it's just `#[arg(long)]`; on every other speech build the existing `requires_all = ["whisper_model", "voice_onnx", "voice_config"]` stays in force.
- **`Cli::{whisper_model, voice_onnx, voice_config, voice}`** are gated under `cfg(all(feature = "speech", not(all(target_os = "macos", feature = "macos-native"))))`, so the fields and their `#[arg(long)]` lines simply don't compile in on macOS-native.
- **`validate_speech_assets`** moves under the same cfg — it has no callers on the macOS-native build.
- **`speech_loop::SpeechLoopConfig`**: same cfg gating on `whisper_model`, `voice_onnx`, `voice_config`, `voice_id`. Field types switched from `&'a Path` / `&'a str` to owned `PathBuf` / `String`, so the struct can drop its lifetime parameter on the macOS-native build without a `PhantomData<&'a ()>` workaround. Public signature of `speech_loop::run` becomes `SpeechLoopConfig` (no lifetime).
- **Speech branch in `main.rs`**: cfg-gated `let cfg = …` blocks for the two paths. Non-macOS-native: unchanged validate-then-construct flow, with one extra `.clone()` per Path because `SpeechLoopConfig` now owns them. macOS-native: three-field `SpeechLoopConfig` with just `mic_silence_ms`, `verbose`, `locale`.
- **Two new tests** in `crates/primer-cli/src/main.rs::tests`, each gated to a complementary cfg so they cover both speech builds:
  - `speech_alone_parses_on_macos_native_without_whisper_piper_flags` (only under `cfg(all(feature = "speech", target_os = "macos", feature = "macos-native"))`).
  - `speech_alone_still_rejected_off_macos_native` (only under `cfg(all(feature = "speech", not(all(target_os = "macos", feature = "macos-native"))))`).
- **Pre-existing dead-code warning gone.** The "fields `whisper_model`, `voice_onnx`, `voice_config`, and `voice_id` are never read" warning that #111's PR description called out as a separate review finding no longer fires under `--features primer-cli/speech,primer-cli/macos-native` — the fields don't exist on that build.
- **`--mic-silence-ms` stays on both speech builds.** Silero remains the VAD on macOS-native; only whisper + piper are swapped out for SFSpeechRecognizer + AVSpeechSynthesizer.
- **README + CLAUDE.md updated.** README's voice-mode flag table now annotates each flag's per-build availability. CLAUDE.md's `--speech` mode bullet documents the cfg-gating + the lifetime drop on `SpeechLoopConfig`.
- **Branch:** `cli/macos-native-drop-whisper-piper-flags-issue-112`.
- **Tests:** **839 passed / 0 failed / 3 ignored** under default features (unchanged from pre-session baseline). `cargo test -p primer-cli --features speech`: **12 / 0 / 0** (was 11). `cargo test -p primer-cli --features speech,macos-native`: **12 / 0 / 0** (was 11; previously also emitted the dead-code warning). `cargo test -p primer-gui --features speech`: **129 / 0 / 0** (unchanged). fmt + clippy `-D warnings` clean on every relevant feature combination.

### Earlier in this session day (already merged before this PR opened)

- **PR #117** — pin resume-locale-inheritance contract end-to-end (closes #87 + #116). Merged as commit `413a1df`. Adds the `__concept_language_tag_for_tests` cross-crate test seam and two narrow regression tests that pin PR #115's `SqliteSessionStore::set_locale` contract on disk.

## What's next

### Immediate (this session's in-flight)

- **This PR (closes #112)** — verify CI green and merge. Diff is small and confined to `primer-cli`. If CI surfaces an unexpected failure, the most likely culprits are (a) a cfg-gate I missed in CI's feature combinations (e.g. a Linux build that pulls `macos-native` as a feature — though `macos-native` is a no-op on non-macOS targets so the not-macos-native arm should remain in effect), or (b) a clippy lint on the new owned-`PathBuf` clone path. Both fix-locally-and-push fast.

### macOS-native speech follow-ups (opened during the #110 / #111 PRs)

- **#114** — speech(macos-native): stream PCM chunks to speaker as `AVSpeechSynthesizer` emits them (cut time-to-first-audio). Larger; touches the synthesis path. The current path buffers the full utterance before pushing to cpal; streaming would let the user hear the start of the response sooner. This is the last open macOS-native follow-up after #112 lands.

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
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality). Voice mode (Phase A) GUI work landed in PR #89; #91 closed with PR #101; #92 closed with PR #104; #102 closed with PR #110; #112 closing with this PR. The remaining Phase 2 polish is the still-open piece — #114 expands that area.
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default.

### Klexikon corpus follow-ups (carried forward)

- **Klexikon corpus expansion past 66.** Klexikon has ~3000 articles. The 2 corpus gaps from PR #93 (`gänsehaut` reflex; tides on the `mond` article) would need either expanded articles or additional Klexikon titles. Concrete next-batch acceptance: pick 30-50 more titles in still-thin clusters. Re-run pipeline. No code change required. Once corpus grows past ~150 passages, re-run the sweep harnesses to verify production defaults still flatline at 100% strict from `top_k=5` onward on non-paraphrase queries.
- **Klexikon license claim spot-check.** WebFetch a sample of ~5 article footers and verify each shows CC-BY-SA-4.0. Low-priority.

### Smaller-scope follow-ups still open

Verified against `gh issue list` 2026-05-18 (#86 closed by PR #115; #87 + #116 closed by PR #117; #112 closure pending this PR's merge):

- **#114** — voice(macos-native): stream PCM chunks to speaker as AVSpeechSynthesizer emits them.
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

Carried over from PR #117's brief, still pending:

- **Open-counter is thread-local, not global.** The `session_store_open_count` test seam relies on `#[tokio::test]`'s default `current_thread` flavour. A future test that opts into `flavor = "multi_thread"` plus `spawn_blocking` will see the counter reset to 0 on the other thread.
- **`SqliteSessionStore::set_locale` is mutable by reference.** Future code that holds the store as `Arc<dyn LearnerStore>` cannot call it. PR #117 now pins this with the end-to-end test — it would fail loudly if the wrap moved.
- **`__concept_language_tag_for_tests` opens a sibling rusqlite::Connection.** Silently returns `None` on open failure rather than panicking; "use only after drop" contract.

**New for this session — minor risks:**

- **`SpeechLoopConfig` shape now differs between speech builds.** On macOS-native it has three fields; on every other speech build it has seven. Any future code that introspects this struct (serialization, debug-formatting, builder pattern, etc.) needs matching cfg gates. Today the only consumer is `speech_loop::run` in the same crate, so the blast radius is contained.
- **Owned `PathBuf` / `String` in `SpeechLoopConfig` means one extra clone per Path on the non-native build.** This runs once at session start; negligible. The alternative (keep `&'a Path` + add `PhantomData<&'a ()>` for the cfg-gated build) is also viable but uglier — if the clone ever becomes load-bearing, revisit.
- **`Cli` struct field set now varies by build.** The `Cli` struct itself doesn't change shape between the two speech builds — just which `#[arg]` fields are declared. Future tests that hardcode field counts via reflection would need cfg gates; current tests don't.
- **Manual real-LLM smoke for Hindi and OpenAI-compat has not run.** Same recommendation as the prior brief:
  - Hindi: `~/.cargo/bin/cargo run --bin primer -- --backend cloud --language hi --name Aarav --age 9 --no-persist --verbose`.
  - OpenAI-compat: spin up `oMLX --serve` or `llama-server --port 8000 --model <gguf>`; `~/.cargo/bin/cargo run --bin primer -- --backend openai-compat --openai-compat-url http://localhost:8000 --model <model> --no-persist --verbose`.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing; new entries from this session at the top.)

- **Cfg-gate CLI fields + the matching struct fields together, never just one side** (issue #112). When a CLI flag is meaningless on a build configuration and a downstream config struct mirrors that flag, gating only one side leaves a dead-code warning (consumer struct field never read) or a forced-dummy UX (CLI requires a value that gets discarded). Gate them in lockstep — flag declaration, `requires_all` list, asset-validation function, the consumer struct field, and the call-site construction — all under the same `cfg(...)` predicate. The two-test pattern (one test under each side of the cfg) keeps both behaviours pinned in CI without needing a feature-matrix workflow.
- **Drop lifetimes from cfg-gated structs by owning their references.** When all the borrowed fields in a struct are cfg-gated out on one build, the lifetime parameter becomes unused on that build. The two clean fixes: (a) `PhantomData<&'a ()>` under the inverse cfg, or (b) switch `&'a Path` → `PathBuf` and `&'a str` → `String` so the struct doesn't need the lifetime at all. (b) trades one clone for cleaner shape and tends to win when the struct is small and constructed once.
- **`#[cfg_attr]` to switch a single attribute payload, not just enable/disable an attribute.** When a clap `#[arg(...)]` attribute carries a `requires_all` whose contents depend on cfg, two `#[cfg_attr(cond, arg(long, ...))]` lines with mutually exclusive conditions is cleaner than splitting the field into two cfg-gated declarations. The field name appears once; the attribute payload switches.
- **`#[doc(hidden)] pub` cross-crate test seams in `primer-storage`** to avoid pulling rusqlite into consumer dev-deps (issues #87 + #116).
- **Pin the on-disk consequence, not just the in-memory inputs** (issue #116).
- **Thread-local counters as test seams for behavioural pin tests** (issue #86).
- **Reorder construction to fold redundant probes into the build path** (issue #86).
- **`set_locale`-style re-tag methods when the resource itself is locale-neutral** (issue #86).
- **Opt-in version-controlled git hooks under `.githooks/`.**
- **CI as source of truth; local hooks as early-warning copies.**
- **Resolve binary tools via $ENVVAR → known install path → PATH.**
- **Single source of truth at the IPC trust boundary** (PR #108).
- **Verify before claiming closed.**
- **Co-locate workflow-level policies with the steps that enforce them.**
- **TDD-driven validator extension.** Add the failing test → watch it fail → land the validator change → land the consumer (data file or producer site).
- **Subagent-driven development with two-stage review (spec + code-quality) per task.**
- **Promote modules that have outgrown their original location.**
- **Two-firewall preview gates for safety-critical opt-outs.**
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
git status                       # confirm clean (or on the `cli/macos-native-drop-whisper-piper-flags-issue-112` branch if reviewing this PR)
git checkout main
git pull
git log --oneline -10            # 413a1df (PR #117) at top until this PR merges

# Check the PR's status; merge if green.
gh pr list --state open
gh pr checks <number>            # this session's PR number once `gh pr create` returns it

# Opt-in to the local pre-commit hook (one-time per clone; from PR #109):
git config core.hooksPath .githooks

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 839 passed, 0 failed, 3 ignored (default features).

~/.cargo/bin/cargo test -p primer-cli --features speech
# Expected: 12 passed, 0 failed, 0 ignored (includes the new
# speech_alone_still_rejected_off_macos_native test).

# On macOS only:
~/.cargo/bin/cargo test -p primer-cli --features speech,macos-native
# Expected: 12 passed, 0 failed, 0 ignored (includes the new
# speech_alone_parses_on_macos_native_without_whisper_piper_flags test).

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

To exercise the macOS-native build manually after this PR lands (verifies #112's fix):

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run --features primer-cli/speech,primer-cli/macos-native --bin primer -- \
    --speech --name SmokeTester --age 9 --no-persist --verbose
# Expected: no clap MissingRequiredArgument error — the four whisper/piper
# flags are no longer required (or even declared). SFSpeechRecognizer +
# AVSpeechSynthesizer carry STT and TTS; Silero stays as the VAD.
# Note: this still requires the macOS Speech framework to be available;
# the loop will error if SFSpeechRecognizer's on-device English assets
# are missing (System Settings → Keyboard → Dictation).
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
