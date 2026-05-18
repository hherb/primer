# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-18T1747+0800 (PR #118 — drop dummy whisper/piper flag requirements on the macOS-native build — **merged** to `main` as commit `1d8a1d7`, closing issue #112. No PR currently in flight.)

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **839 Rust tests** under default features. 3 ignored. With `--features primer-gui/speech`: **129 primer-gui tests**. With `--features primer-cli/speech`: primer-cli has **12 tests**; same count under `--features primer-cli/speech,primer-cli/macos-native` (the `speech_alone_parses_on_macos_native_without_whisper_piper_flags` test replaces the off-native variant under that build). Plus 135 Python tests in `data/ingest/`.
3. **Pick the next item from "What's next" below.** No PR is in flight today. The remaining open follow-ups are the eleven issues listed under "Smaller-scope follow-ups still open" plus the carried-forward larger items.
4. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.

## What we shipped this session

This session opened with PR #118 already pushed to GitHub and pending CI green. The session's actual work was confined to closing it out:

- **CI verification.** PR #118 came back **green** on all four required checks (`cargo test (default features)`; CodeQL × 3 for actions / javascript-typescript / python / rust; CodeQL summary). Merged to `main` as commit `1d8a1d7` (squash merge from branch `cli/macos-native-drop-whisper-piper-flags-issue-112`).
- **Issue #112 closed.** GitHub auto-closed the issue at 2026-05-18T09:44:16Z when the merge landed. No follow-up issues were spawned by the PR.
- **Local cleanup.** Switched local clone to `main`, fast-forwarded over `413a1df..1d8a1d7`, deleted the merged feature branch.
- **Sanity verification on main.** `cargo build --workspace` clean; `cargo test --workspace` green at **839 passed / 0 failed / 3 ignored**; `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean.
- **No code change this session.** All work was confined to this brief (and its handoff snapshot in `docs/handoffs/`). README.md and CLAUDE.md were updated as part of PR #118 itself, before merge.

### PR #118 recap (merged as `1d8a1d7`, closes #112)

Under `--features primer-cli/speech,primer-cli/macos-native` on macOS the SFSpeechRecognizer + AVSpeechSynthesizer path replaces whisper + piper, yet clap still forced users to pass dummy `--whisper-model /dev/null --voice-onnx /dev/null --voice-config /dev/null` because of `requires_all`, and `speech_loop::run` discarded every value at runtime (emitting a dead-code warning on the matching `SpeechLoopConfig` fields, called out as a separate review finding in PR #111).

The fix cfg-gates the four CLI flags + their `SpeechLoopConfig` mirrors under `not(all(target_os = "macos", feature = "macos-native"))` so they disappear entirely on the macOS-native build:

- `Cli::speech` uses `#[cfg_attr]` to switch the `requires_all` payload.
- `Cli::{whisper_model, voice_onnx, voice_config, voice}` fields are declared only on the whisper+piper build.
- `validate_speech_assets` moves under the same cfg.
- `SpeechLoopConfig` drops its lifetime parameter; the four asset fields are now owned (`PathBuf` / `String`) so the macOS-native arm doesn't need a `PhantomData<&'a ()>` workaround.
- `speech_loop::run` takes `SpeechLoopConfig` (no lifetime).
- Speech branch in `main.rs` cfg-gates the validate-then-construct path on the non-native arm; the macOS-native arm constructs the three-field config directly.

Two new tests, each gated to a complementary cfg so both speech builds are pinned in CI:

- `speech_alone_parses_on_macos_native_without_whisper_piper_flags`
- `speech_alone_still_rejected_off_macos_native`

`cargo test -p primer-cli --features speech`: 12 / 0 / 0 (was 11). `cargo test -p primer-cli --features speech,macos-native`: 12 / 0 / 0 (was 11; the pre-existing dead-code warning is also gone). Workspace default features: 839 / 0 / 3 (unchanged). fmt + clippy `-D warnings` clean on every relevant feature combination.

## What's next

### macOS-native speech follow-ups (open after #112 landed)

- **#114** — speech(macos-native): stream PCM chunks to speaker as `AVSpeechSynthesizer` emits them (cut time-to-first-audio). Larger; touches the synthesis path. The current path buffers the full utterance before pushing to cpal; streaming would let the user hear the start of the response sooner. **This is the only macOS-native speech follow-up still open** after #112 landed.

Acceptance criteria for #114 (sketch — refine before implementing):
- `AVSpeechSynthesizer.writeUtterance:toBufferCallback:` is already chunk-by-chunk; the change is plumbing the callback's `AVAudioPCMBuffer` into the speaker ringbuf directly instead of accumulating into a single `Vec<f32>` and pushing post-synthesis.
- Time-to-first-audio (TTFA) measured from end of LLM streaming to first speaker sample should drop substantially. Pick a smoke phrase and pin a TTFA budget in a manual smoke (no test; the smoke binary `examples/tts_macos_pcm_smoke.rs` is the right home for an instrumented variant).
- The PCM-callback chunk-size assumption that `examples/tts_macos_pcm_smoke.rs` validates today must still hold under the streaming path.
- The `is_speaking` echo-suppression invariant (mic muted while speaker is producing audio) must not regress — extend the drain-hook logic so it waits for the *last* chunk to drain, not the first.

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
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality). Voice mode (Phase A) GUI work landed in PR #89; #91 closed with PR #101; #92 closed with PR #104; #102 closed with PR #110; #112 closed with PR #118. The remaining Phase 2 polish is the still-open piece — #114 expands that area.
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default.

### Klexikon corpus follow-ups (carried forward)

- **Klexikon corpus expansion past 66.** Klexikon has ~3000 articles. The 2 corpus gaps from PR #93 (`gänsehaut` reflex; tides on the `mond` article) would need either expanded articles or additional Klexikon titles. Concrete next-batch acceptance: pick 30-50 more titles in still-thin clusters. Re-run pipeline. No code change required. Once corpus grows past ~150 passages, re-run the sweep harnesses to verify production defaults still flatline at 100% strict from `top_k=5` onward on non-paraphrase queries.
- **Klexikon license claim spot-check.** WebFetch a sample of ~5 article footers and verify each shows CC-BY-SA-4.0. Low-priority.

### Smaller-scope follow-ups still open

Verified against `gh issue list` 2026-05-18T1747+0800 (no new issues opened since the prior brief; #112 closed by PR #118):

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

Carried over from PR #118's brief, still pending:

- **`SpeechLoopConfig` shape now differs between speech builds.** On macOS-native it has three fields; on every other speech build it has seven. Any future code that introspects this struct (serialization, debug-formatting, builder pattern, etc.) needs matching cfg gates. Today the only consumer is `speech_loop::run` in the same crate, so the blast radius is contained.
- **Owned `PathBuf` / `String` in `SpeechLoopConfig` means one extra clone per Path on the non-native build.** This runs once at session start; negligible.
- **`Cli` struct field set now varies by build.** The `Cli` struct itself doesn't change shape between the two speech builds — just which `#[arg]` fields are declared. Future tests that hardcode field counts via reflection would need cfg gates; current tests don't.

Carried over from PR #117's brief, still pending:

- **Open-counter is thread-local, not global.** The `session_store_open_count` test seam relies on `#[tokio::test]`'s default `current_thread` flavour. A future test that opts into `flavor = "multi_thread"` plus `spawn_blocking` will see the counter reset to 0 on the other thread.
- **`SqliteSessionStore::set_locale` is mutable by reference.** Future code that holds the store as `Arc<dyn LearnerStore>` cannot call it. PR #117 now pins this with the end-to-end test — it would fail loudly if the wrap moved.
- **`__concept_language_tag_for_tests` opens a sibling `rusqlite::Connection`.** Silently returns `None` on open failure rather than panicking; "use only after drop" contract.

**Manual real-LLM smoke for Hindi and OpenAI-compat has not run.** Same recommendation as the prior brief:

- Hindi: `~/.cargo/bin/cargo run --bin primer -- --backend cloud --language hi --name Aarav --age 9 --no-persist --verbose`.
- OpenAI-compat: spin up `oMLX --serve` or `llama-server --port 8000 --model <gguf>`; `~/.cargo/bin/cargo run --bin primer -- --backend openai-compat --openai-compat-url http://localhost:8000 --model <model> --no-persist --verbose`.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing.)

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
git status                       # confirm clean on main
git checkout main
git pull
git log --oneline -10            # 1d8a1d7 (PR #118, closes #112) at top

# Check for any new PRs or issues opened since this brief.
gh pr list --state open
gh issue list --state open --limit 30

# Opt-in to the local pre-commit hook (one-time per clone; from PR #109):
git config core.hooksPath .githooks

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 839 passed, 0 failed, 3 ignored (default features).

~/.cargo/bin/cargo test -p primer-cli --features speech
# Expected: 12 passed, 0 failed, 0 ignored.

# On macOS only:
~/.cargo/bin/cargo test -p primer-cli --features speech,macos-native
# Expected: 12 passed, 0 failed, 0 ignored.

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

To exercise the macOS-native build manually (verifies #112's fix landed):

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
