# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-16T0758+0800 (PR #107 in flight — CI workflow polish closing #95. Closed #67 and #69 after verification. No other code changes this turn.)

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **826 Rust tests** under default features. 3 ignored. Add `--features primer-gui/speech` for **117 primer-gui tests** including the voice-mode integration coverage. Add `--features primer-kb-load/fastembed` for the embedding-backed sweeps + the real-BGE-M3 recall tests. Plus 135 Python tests in `data/ingest/` (unchanged this session — Rust-only doc/CI work).
3. **Check PR #107's CI status.** If green and unmerged, merge it. If red, inspect the failing step — the change is small (only `.github/workflows/ci.yml`) so the diagnosis is localised. Behaviour is intended to be identical to pre-#95 — clippy strict, test compile strict, fmt strict.
4. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.

## What we shipped this session

This session was a small follow-up cleanup turn:

### CI workflow polish — PR #107 (in flight) closing #95

- `.github/workflows/ci.yml` — drop the workflow-level `RUSTFLAGS: -D warnings` env, add `-- -D warnings` to the explicit `cargo clippy` step, and set `RUSTFLAGS: -D warnings` as a step-local env on the `cargo test` step so test-only compile warnings still fail CI. Behaviour is preserved exactly; the policy is now co-located with the steps that enforce it (the readability win PR #94 surfaced).
- Branch: `ci/clippy-explicit-deny-warnings`; head SHA `40e5fcf`.
- Local verification: `~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings` exit 0; `RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo test --workspace --no-fail-fast` → 826 / 0 / 3.

### Issues closed after verification (no code change)

- **#67** — Defensive `.unwrap()` in hybrid `pick_winner` `partial_cmp` chains. The shared sweep harness refactor (PR #97 closing #66) already migrated the winner-selection logic to the helper at `src/crates/primer-kb-load/tests/common/sweep.rs`, which uses `.unwrap_or(std::cmp::Ordering::Equal)` uniformly (BM25 selection lines 154–165, hybrid selection lines 522–534). The per-locale sweep files are thin shims with no `partial_cmp` of their own. Acceptance criteria met; closed.
- **#69** — `primer-engine` embedder helpers should return `Result`, not `std::process::exit`. All three helpers (`build_fastembed_embedder`, `build_ollama_embedder`, `build_openai_compat_embedder`) at `src/crates/primer-engine/src/wiring.rs` lines 313–416 return `Result<Option<Arc<dyn Embedder>>, String>` in both `#[cfg(feature)]` arms; the doc comment explicitly captures the invariant ("Earlier versions called `std::process::exit(1)` here which is hostile to any caller that isn't a CLI — never re-introduce that."). Consumers (CLI `main.rs` lines 747–786; GUI `wiring.rs` lines 359–361) properly `match` on the `Result`; `grep -n 'process::exit' src/crates/primer-engine/src/wiring.rs` returns no hits. Acceptance criteria met; closed.

**Verification on the new branch (`src/`):**

- `~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo test --workspace --no-fail-fast` → **826 passed / 0 failed / 3 ignored**

**No README.md or ROADMAP.md update needed** — this session's changes are CI/test-infrastructure polish, not phase-progress or user-facing-feature work.

## What's next

### Immediate (carry-forward + this session's in-flight)

- **PR #107 (closes #95)** — verify CI green and merge. Behaviour-preserving YAML-only change.
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

- **Local llama.cpp inference (Phase 1.1).** `LlamaCppBackend` stub at `primer-inference` is still the entry point. The OpenAI-compat path partially obviates this since llama.cpp's `--server` is already reachable via the new backend, but a direct llama.cpp embedding (without the HTTP hop) remains the long-term Phase 1 goal.
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality). The voice mode (Phase A) GUI work landed in PR #89; #91 closed with PR #101; #92 closed with PR #104. The remaining Phase 2 polish is the still-open piece.
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default.

### Klexikon corpus follow-ups (carried forward)

- **Klexikon corpus expansion past 66.** Klexikon has ~3000 articles. The 2 corpus gaps from PR #93 (`gänsehaut` reflex; tides on the `mond` article) would need either expanded articles or additional Klexikon titles. Concrete next-batch acceptance: pick 30-50 more titles in still-thin clusters. Re-run pipeline. No code change required. Once corpus grows past ~150 passages, re-run the sweep harnesses to verify production defaults still flatline at 100% strict from `top_k=5` onward on non-paraphrase queries.
- **Klexikon license claim spot-check.** WebFetch a sample of ~5 article footers and verify each shows CC-BY-SA-4.0. Low-priority.

### Smaller-scope follow-ups still open

Verified against `gh issue list` 2026-05-16 (#67, #69, #95 closed this session; #95 closure pending PR #107 merge):

- **#103** — voice: cancel-and-retry path drops the first half of the transcript (bug, voice-loop hardening territory).
- **#102** — voice: locale stays stale on session switch (`start_session` doesn't tear down `state.voice`) (bug).
- **#98** — refactor(tests): split `tests/common/sweep.rs` into `bm25`/`hybrid` submodules (enhancement).
- **#96** — tooling: prevent `cargo fmt` drift on `main` (workflow-level pre-commit hook).
- **#87** — primer-gui: end-to-end resume_session test for cross-locale inheritance (enhancement).
- **#86** — primer-gui: avoid double session-DB open on resume (enhancement).
- **#81** — GUI: settings modal needs a focus trap (enhancement).
- **#80** — GUI: expose `Locale::ALL` via a Tauri command instead of hand-mirroring it in `settings.js` (enhancement). **Now relevant under the preview-locale model** — the Tauri command should expose `Locale::ALL` (not the enum), so preview locales are excluded automatically.
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
- `close_session` may drop the final classifier task's `turn_classifications` row on a short conversation. (PR #106's added `tracing::debug!` lines around `save_session` make this easier to diagnose if it surfaces in user reports.)
- Identifier-format divergence across structured-output crates (`llm:{model}` vs `llm:{backend}:{model}`).
- `apply_comprehension` insertion policy (only updates concepts already in `learner.concepts`).
- Concepts are monotonic on `learner_concepts` — saved-once stays saved.
- No cancellation token on streaming-task spawns.
- Background-task spawn order is load-bearing on serialized backends.
- "I just don't want to see this word" escape valve for vocab not yet implemented.

**Carried over from the prior brief, still pending:**

- **Manual real-LLM smoke for Hindi and OpenAI-compat has not run.** Recommended:
  - Hindi: `~/.cargo/bin/cargo run --bin primer -- --backend cloud --language hi --name Aarav --age 9 --no-persist --verbose`. A few child-style Hindi prompts via stdin. Document any obvious translation register issues by appending to `docs/localisation/hi/README.md`'s "Open items" or to `docs/locale/models/HINDI.md`.
  - OpenAI-compat: spin up `oMLX --serve` or `llama-server --port 8000 --model <gguf>`; `~/.cargo/bin/cargo run --bin primer -- --backend openai-compat --openai-compat-url http://localhost:8000 --model <model> --no-persist --verbose`. Confirm streaming works, error path handles a deliberately-bad URL, embedder round-trip via `--embedder-backend openai-compat`.
- **The preview-locale pattern is now established.** The `[meta] status = "preview"` field + `Locale::ALL` exclusion is the canonical way to land a new locale without exposing it to end users prematurely. Future locales (Spanish, Tamil, Bengali, …) should follow this two-firewall pattern, including the `# REVIEW:` markers in the prompt pack.
- **`locale_defaults` is now at the crate root.** Any future code that imports it should use `primer_speech::locale_defaults::*` directly. Grep `grep -rn "voice_loop::locale_defaults" src/crates --include="*.rs"` should always return zero.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing.)

- **Verify before claiming closed.** When a prior PR's commit message says "closes #X, #Y" but the PR was only scoped to close #X, audit #Y's acceptance criteria against current `main` before closing — sometimes the refactor implicitly satisfied it (e.g. #67 / #69 this session), sometimes not. The brief's explicit "verify before claiming closed" line on #67 is the load-bearing pattern.
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
git status                       # confirm clean
git checkout main
git pull
git log --oneline -10            # 5f11dac (PR #106) is still at top until #107 merges

# Check PR #107 status; merge if green.
gh pr checks 107
gh pr view 107

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 826 passed, 0 failed, 3 ignored (default features).

~/.cargo/bin/cargo test -p primer-gui --features speech
# Expected: 117 passed, 0 failed, 0 ignored.

~/.cargo/bin/cargo fmt --all -- --check
# Expected: clean.

~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
# Expected: clean exit 0 (matches the post-#95 explicit form).

RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo test --workspace --no-fail-fast
# Expected: 826 passed (matches the new step-env enforcement).

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
