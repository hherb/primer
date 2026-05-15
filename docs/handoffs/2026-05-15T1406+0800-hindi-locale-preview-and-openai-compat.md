# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-15T1406+0800 (after opening PR #105 — Hindi locale preview + OpenAI-compat inference/embedding backend. Branch `feat/locale-hindi-preview`, 21 commits on top of `d1b1af3`. Mixed scope by design — see "Branch shape" below.)

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **823 Rust tests** under default features (up from 777 once PR #105 merges; the +46 spans Hindi-preview's +18 + 4 promoted locale_defaults tests + OpenAI-compat's tests + a few misc). 3 ignored. Add `--features primer-gui/speech` for **117 primer-gui tests** including the voice-mode integration coverage. Add `--features primer-kb-load/fastembed` for the embedding-backed sweeps + the real-BGE-M3 recall tests. Plus 135 Python tests in `data/ingest/` (unchanged this session — Rust-only work).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.

## Branch shape

PR #105 carries **two concurrent scopes** on `feat/locale-hindi-preview`:

1. **Hindi locale preview** — authored by Claude in subagent-driven-development cycle from spec → plan → 5 implementation tasks → verification → PR. 13 of the 21 branch commits.
2. **OpenAI-compatible inference + embedding backend** — authored by Horst in parallel commits interleaved into the branch (the user explicitly chose "combined PR" when asked about PR shape). 8 of the 21 branch commits, covering backend + embedder + CLI flags + engine dispatch + fmt + CLAUDE.md.

Both stacks build cleanly together; tests pass; clippy clean on default and speech features.

## What we shipped this session

### Hindi locale preview

**The two firewalls:**
- `Locale::ALL` stays `[English, German]` — CLI/GUI pickers iterate that slice, so end users never reach Hindi via the standard UI.
- `[meta] status = "preview"` — new optional field on prompt packs (closed `PackStatus { Stable, Preview }` enum, allow-list `["stable", "preview"]`, default Stable when absent). `load_cached` emits one `tracing::warn!` per `(process, locale)` pair on Preview load.

**Concrete deliverables:**

- **`primer-core::i18n::Locale::Hindi` variant** with cascade arms in `name`/`bcp47`/`pack_id`/`from_pack_id`/`render_inference_error`. Doc comment names the preview-gate rationale and points at `docs/localisation/hi/README.md`.
- **`render_hindi`** function with 6 Devanagari error strings (Auth, RateLimited ±retry_after, ServiceUnavailable, NetworkUnavailable, ModelNotFound, generic Other). Singular/plural split on `secs == 1` mirrors `render_german`.
- **`prompts/hi.toml`** — 210 lines of Devanagari, structurally complete, `[meta] status = "preview"`, `# REVIEW:` markers above every translation block (`[system_prompt]`, all four `[language_guidance]` bands, `[intent]`, `[engagement]`, `[sections]`, `[question_detection]`, `[voice_state]`). Tense register: `तुम` (informal, child-directed) — flagged in the file header for native-speaker review.
- **`PackStatus` validator** + `MetaSection.status: Option<String>` with `#[serde(default)]` + drift-prevention test pinning the allow-list strings.
- **Warn-once gate** — `static GATE: OnceLock<Mutex<HashSet<Locale>>>`. Graceful degrade on poison (warn unconditionally; never silence — spec-mandated). Lock released before `tracing::warn!` runs so a slow subscriber can't block other warn-once emitters.
- **`LOCALE_DEFAULTS["hi"]`** — `hi_IN-rohan-medium` Piper voice + multilingual `ggml-small.bin` Whisper, `approx_total_mb = 540`.
- **`docs/localisation/hi/README.md`** — preview status page documenting the two firewalls, machine-translation note, adaptation notes (tense register, complexity marker = तत्सम / Sanskrit-rooted rather than syllable count, vocabulary examples, factual-prefix matching), corpus gap (no Hindi children's wiki exists; candidates documented: NCERT, Pratham Books StoryWeaver, Wikisource Hindi), voice asset pinning, open items checklist for promoting to stable.
- **`docs/locale/models/HINDI.md`** — empty model-evaluation skeleton; mirrors the German page's shape.
- **`docs/localisation/README.md`** — `hi` row added to the supported-locales table with `🟡 preview` tag.
- **`CLAUDE.md` gotcha** — new bullet immediately after the locale-aware `{minutes}` interpolation bullet, documenting the preview-status convention end-to-end.

**Architectural cleanup along the way:**

- **`locale_defaults` promoted from `voice_loop/` to the crate root of `primer-speech`.** The module has zero speech-backend deps (only imports `primer_core::i18n::Locale`). Two consequences: (a) 4 previously dormant regression tests (English-default, German-default, URL-pinning, size-sanity) now run in default `cargo test --workspace` instead of needing the `voice-loop` feature; (b) the one external consumer `primer-gui/src/voice/assets.rs` migrated to `primer_speech::locale_defaults::*` and the matching doc-comment in `primer-gui/src/config.rs` was updated. No backwards-compat re-alias left in the tree (per CLAUDE.md: "avoid backwards-compat hacks").

**Tests added (TDD throughout):**

- 5 in `primer-core::i18n::tests` — `locale_hindi_pack_id_and_bcp47`, `locale_all_excludes_hindi_until_translation_reviewed`, `hindi_inference_errors_contain_devanagari`, `hindi_model_not_found_includes_model_name`, `hindi_other_does_not_leak_inner_dev_string`. Plus the existing `locale_all_lists_every_variant` renamed to `locale_all_contains_only_production_ready_locales` so post-Hindi readers know the gap is intentional.
- 7 in `primer-pedagogy::prompt_pack::tests` — `pack_status_*` (5 tests on the new field) + `preview_warning_emits_once_per_locale_on_load_cached` + `hindi_pack_loads_in_preview_status`, `hindi_pack_intent_lookups_all_populated`, `hindi_pack_voice_state_section_complete`, `hindi_pack_renders_base_with_name_and_age`, `hindi_pack_knowledge_intro_substitutes_age`, `hindi_pack_break_suggestion_intro_substitutes_minutes`, `hindi_load_cached_warns_exactly_once_per_locale`.
- 1 in `primer-speech::locale_defaults::tests` (`hindi_default_is_rohan_plus_small_multilingual`) plus 4 previously feature-gated tests now visible.

**Verification:**

- `~/.cargo/bin/cargo test --workspace` → 823 passed / 0 failed / 3 ignored
- `~/.cargo/bin/cargo test -p primer-gui --features speech` → 117 passed / 0 failed
- `~/.cargo/bin/cargo fmt --all -- --check` clean
- `RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets` clean
- `RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech` clean
- Manual smoke: `--backend stub --language hi --no-persist` emits exactly one `WARN primer::prompt_pack ... locale="hi"` line; session runs to `bye` cleanly.

**Spec + plan:** [`docs/superpowers/specs/2026-05-15-hindi-locale-preview-rollout.md`](docs/superpowers/specs/2026-05-15-hindi-locale-preview-rollout.md) and [`docs/superpowers/plans/2026-05-15-hindi-locale-preview.md`](docs/superpowers/plans/2026-05-15-hindi-locale-preview.md).

### OpenAI-compatible inference + embedding backend (Horst's parallel work)

Horst landed the OpenAI-compat stack across 8 commits on the same branch (he explicitly chose "combined PR" when asked about scope):

- **`OpenAiCompatBackend`** in `primer-inference` — speaks `/v1/chat/completions` with SSE streaming + error classification + bounded jittered retry. Unlocks oMLX, LM Studio, vLLM, llama.cpp `--server`, plus remote providers (Together, Groq, OpenRouter).
- **`OpenAiCompatEmbedder`** in `primer-embedding` — `/v1/embeddings` with native batching.
- **CLI flags**: `--backend openai-compat` + URL/API-key flags. `--embedder-backend openai-compat` wired through engine + CLI dispatch.
- **Engine wiring**: `primer-engine::build_backend` dispatch extended.
- **CLAUDE.md note** covering the new backend.

**Spec + plan:** [`docs/superpowers/specs/2026-05-15-openai-compat-backend-design.md`](docs/superpowers/specs/2026-05-15-openai-compat-backend-design.md) and [`docs/superpowers/plans/2026-05-15-openai-compat-backend.md`](docs/superpowers/plans/2026-05-15-openai-compat-backend.md).

**Both stacks build together cleanly.** Workspace tests, fmt, clippy on default + speech features all clean. Manual Hindi smoke run; OpenAI-compat real-server smoke is a recommended-not-blocking follow-up.

## What's next

### Hindi-locale-preview follow-ups

- **Native-speaker review of `prompts/hi.toml`.** Grep `# REVIEW:` for the blocks flagged for review. Critical items: tense register (तुम vs. आप), age-band vocabulary markers (तत्सम / Sanskrit-rooted vocabulary), factual-prefix list (Hindi syntax places question words at the end so prefix-matching is weak — consider setting `factual_prefixes = []` and relying entirely on the LLM-engagement-classifier path), `[voice_state]` UI copy (cramped in Devanagari).
- **Hindi children's-vocabulary corpus.** Three candidate sources documented in `docs/localisation/hi/README.md`:
  - **NCERT textbooks** ([ncert.nic.in](https://ncert.nic.in/)) — Indian government textbooks; license claim "free to use for educational purposes" needs spot-checking.
  - **Pratham Books StoryWeaver** ([storyweaver.org.in](https://storyweaver.org.in/)) — CC-BY on most books but varies per book; ingest needs per-book license check.
  - **Wikisource Hindi** ([hi.wikisource.org](https://hi.wikisource.org/)) — children's literature; mostly literary, not encyclopedic.
- **`tests/common/hi.rs`** + retrieval-quality / sweep tests for `hi` once a corpus lands.
- **Real-LLM smoke** against `--backend cloud --language hi` and at least three local Ollama models. Populate `docs/locale/models/HINDI.md`.
- **The flip-to-stable PR** when the above are ready: edit `[meta] status = "stable"` in `hi.toml` + add `Self::Hindi` to `Locale::ALL` + remove `# REVIEW:` markers + drop the preview-banner section from `hi/README.md`. Single commit.

### OpenAI-compat backend follow-ups

- **Real-server smoke testing.** Spin up oMLX (Apple Silicon MLX-native server) and one of {LM Studio, vLLM, llama.cpp `--server`}; run `--backend openai-compat --base-url http://localhost:8000 --model <model>` against each; confirm SSE streaming, error classification, and embedder round-trip. Particularly check the Apple-Silicon throughput claim (the spec cites 20–40% gains via MLX vs. Ollama on the same hardware).
- **GUI wiring.** The spec scopes GUI wiring as a deferred follow-up; today the OpenAI-compat backend is reachable only via the CLI. A future PR should mirror the existing `--backend ollama` / `--backend cloud` GUI surface for the new backend.
- **Model evaluation page.** No `docs/locale/models/*` page exists for OpenAI-compat servers per se (they're a transport, not a model); but a `docs/openai-compat-models.md` or extension to existing per-locale model pages could track which models behave well behind which servers.

### Carried-forward larger items

- **Local llama.cpp inference (Phase 1.1).** `LlamaCppBackend` stub at `primer-inference` is still the entry point. The OpenAI-compat path partially obviates this since llama.cpp's `--server` is already reachable via the new backend, but a direct llama.cpp embedding (without the HTTP hop) remains the long-term Phase 1 goal.
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality). The voice mode (Phase A) GUI work landed in PR #89; #91 closed with PR #101; #92 closed with PR #104. The remaining Phase 2 polish is the still-open piece.
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default.

### Klexikon corpus follow-ups (carried forward)

- **Klexikon corpus expansion past 66.** Klexikon has ~3000 articles. The 2 corpus gaps from PR #93 (gänsehaut reflex; tides on the mond article) would need either expanded articles or additional Klexikon titles. Concrete next-batch acceptance: pick 30-50 more titles in still-thin clusters. Re-run pipeline. No code change required. Once corpus grows past ~150 passages, re-run the sweep harnesses to verify production defaults still flatline at 100% strict from `top_k=5` onward on non-paraphrase queries.
- **Klexikon license claim spot-check.** WebFetch a sample of ~5 article footers and verify each shows CC-BY-SA-4.0. Low-priority.

### Smaller-scope follow-ups still open

- **#86** — primer-gui: avoid double session-DB open on resume (enhancement).
- **#87** — primer-gui: end-to-end resume_session test for cross-locale inheritance (enhancement).
- **#80** — GUI: expose Locale::ALL via a Tauri command instead of hand-mirroring it in settings.js (enhancement). **Newly relevant under the preview-locale model** — the Tauri command should expose `Locale::ALL` (not the enum), so preview locales are excluded automatically.
- **#81** — GUI: settings modal needs a focus trap (enhancement).
- **#71** — GUI: tighten CSP before ship (remove `'unsafe-inline'`).
- **#69** — primer-engine: embedder helpers should return Result, not `std::process::exit`.
- **#46** — explore post-RRF `min_score` as a fifth grid axis in the hybrid sweep.
- **#40** — aggregate per-source attribution row for Wikipedia (and Klexikon).
- **#41** — tighten disambiguation regex if false positives appear.
- **#22** — cache prepared statements for the corpus-bootstrap path (Phase 0.2; enhancement).
- **#21** — separate `--languages` preference list from bound `--language` locale.
- **#20** — i18n placeholder validator can false-fail on translator narrative text.
- **Failed-batch persistence sidecar (issue #38 optional).**
- **Network-error retry on Python ingest side.**
- **Pre-commit fmt hook (workflow-level).**
- **Probe-function duplication between CLI and GUI.** `primer-cli/src/main.rs::probe_espeak_ng_data` and `primer-gui/src/lib.rs::probe_espeak_ng_data` carry byte-identical logic except for the log channel. Low-priority.

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

- **Manual real-LLM smoke for both new scopes has not run.** Recommended before PR #105 merges:
  - Hindi: `~/.cargo/bin/cargo run --bin primer -- --backend cloud --language hi --name Aarav --age 9 --no-persist --verbose`. A few child-style Hindi prompts via stdin. Document any obvious translation register issues by appending to `docs/localisation/hi/README.md`'s "Open items" or to `docs/locale/models/HINDI.md`.
  - OpenAI-compat: spin up `oMLX --serve` or `llama-server --port 8000 --model <gguf>`; `~/.cargo/bin/cargo run --bin primer -- --backend openai-compat --base-url http://localhost:8000 --model <model> --no-persist --verbose`. Confirm streaming works, error path handles a deliberately-bad URL, embedder round-trip via `--embedder-backend openai-compat`.
- **Branch carries mixed scope.** PR #105 is "Hindi preview + OpenAI-compat backend" by Horst's explicit choice. Per-commit blame is preserved (Horst's commits keep his author line; Claude's keep the `Co-Authored-By` trailer); reviewers can read the diff by area.
- **The preview-locale pattern is now established.** The `[meta] status = "preview"` field + `Locale::ALL` exclusion is the canonical way to land a new locale without exposing it to end users prematurely. Future locales (Spanish, Tamil, Bengali, …) should follow this two-firewall pattern, including the `# REVIEW:` markers in the prompt pack.
- **`locale_defaults` is now at the crate root.** Any future code that imports it should use `primer_speech::locale_defaults::*` directly, never via `voice_loop::locale_defaults` (that path was deleted). The grep for old-path consumers in any future PR is `grep -rn "voice_loop::locale_defaults" src/crates --include="*.rs"` — should always return zero.
- **The shared `[meta] status` validator is the right place to land any future cross-pack lifecycle field** (e.g. a future `status = "deprecated"` for a locale that's about to be retired). The allow-list is closed and any new value lands as a deliberate three-place change: enum variant + validator arm + drift-prevention test.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing.)

- **REINFORCED: TDD-driven validator extension.** Add the failing test → watch it fail → land the validator change → land the consumer (data file or producer site). Task 1 / Task 3 in the Hindi plan followed this rigorously.
- **REINFORCED: Subagent-driven development with two-stage review (spec + code-quality) per task.** Each of Tasks 1–4 went through implementer → spec reviewer → code quality reviewer; only Task 5 (docs-only) skipped the code-quality stage. Worked well; caught real issues at each stage (DRY violation in Task 1, lock-held-across-warn in Task 2, misleading test name in Task 3, dead re-export in Task 4).
- **REINFORCED: Promote modules that have outgrown their original location.** Task 4's `locale_defaults` promotion is the model — when a module's deps are narrower than its host module's, promotion is a net positive (4 dormant tests reactivated for free).
- **REINFORCED: Two-firewall preview gates for safety-critical opt-outs.** `Locale::ALL` exclusion + `[meta] status = "preview"` is overkill for low-stakes flags but exactly right for "this could reach a child and they wouldn't know it's machine-translated".
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
- **Subagent-driven development workflow** for plan execution.
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
# Resume on main (after PR #105 merges):
cd /Users/hherb/src/primer
git status                       # confirm clean
git checkout main
git pull
git log --oneline -10            # the PR-105 squash-merge commit should be near the top

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 823 passed, 0 failed, 3 ignored (default features).

~/.cargo/bin/cargo test -p primer-gui --features speech
# Expected: 117 passed, 0 failed, 0 ignored.

~/.cargo/bin/cargo fmt --all -- --check
# Expected: clean.

RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets
# Expected: clean exit 0 (mirrors CI).

RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech
# Expected: clean exit 0.
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
    --backend openai-compat --base-url http://localhost:8000 \
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
