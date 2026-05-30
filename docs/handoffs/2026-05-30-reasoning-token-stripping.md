# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-30 — Shipped **reasoning-token (chain-of-thought) stripping** for the `ollama` and `openai-compat` inference backends. Reasoning-mode models (DeepSeek-R1, QwQ, Qwen3, Gemma4-thinking, medgemma, …) no longer leak their internal `<think>…</think>` / Gemma4 `<|channel>…<channel|>` traces into the child-visible response. Work is on branch **`feat/reasoning-token-stripping`**, pushed, open as **PR #187** (https://github.com/hherb/primer/pull/187). **CI is fully green** (all 10 checks SUCCESS: `cargo test (default features)`, `cargo check (non-default features)`, `cargo clippy (macOS feature combos)`, `cargo build (aarch64-linux-android)`, CodeQL ×5). **Ready to merge** (owner's call). Verified locally under the pinned **1.88** toolchain: `fmt --check`, `clippy --workspace -D warnings`, `test --workspace` all green (**942 passed / 0 failed / 4 ignored**).

## What we shipped this session — reasoning-token stripping (Ollama + openai-compat)

**Branch:** `feat/reasoning-token-stripping`. **PR:** #187 (open, CI green). Branched from `main` @ `148fd48` (the #186 hybrid-default-on squash merge). 19 commits, no stray content — `main..HEAD` is exactly this feature.

Architecture: a pure, heavily-tested streaming state machine in `primer-core` suppresses any text between configured `(open, close)` marker pairs, robust to markers split across stream chunks. Both backends route every parsed chunk through one shared helper before forwarding. Because `InferenceBackend::generate()` aggregates `generate_stream()`, the non-streaming path (classifier/extractor/comprehension JSON parsing) is cleaned for free. When a model reasons but emits **no** visible answer, the backend emits `InferenceError::ReasoningWithoutAnswer`, rendered via the existing i18n boundary as a friendly "thinking problem, try again" (EN/DE/HI) and dropped as a normal mid-stream error (the partial Primer turn is discarded; the child turn stays).

Commit trail (oldest → newest):

| SHA | Commit |
| --- | --- |
| `a7ff021` | docs(spec): reasoning-token stripping (Ollama + openai-compat) |
| `2e3de17` | docs(spec): reasoning-without-answer i18n fallback + did_suppress |
| `f483a0c` | docs: defer GUI custom-marker editor to ROADMAP 0.3; ship CLI now |
| `ec40ba0` | docs(roadmap): record reasoning-token stripping under Phase 0.3 |
| `366f700` | docs(plan): reasoning-token stripping implementation plan |
| `876d3c0` | feat(core): pure `ReasoningFilter` streaming CoT-marker stripper |
| `8144ce8` | fix(core): unbreak finalize_visible doc link + add UTF-8 boundary tests |
| `837bcc2` | feat(core): `ReasoningWithoutAnswer` error + EN/DE/HI render |
| `4ee5721` | test(core): include ReasoningWithoutAnswer in exhaustive error tables |
| `dd0d8ff` | feat(inference): strip reasoning markers in OllamaBackend stream |
| `2318b54` | feat(inference): shared reasoning-filter helper; wire openai-compat; refactor ollama |
| `595edf8` | refactor(inference): finalize-visible signal is a bool, not a byte count |
| `5f94900` | feat(engine): thread `reasoning_markers` through `BackendParams` |
| `6da604a` | feat(cli): `--reasoning-marker` flag to append custom strip pairs |
| `3ad5157` | test(inference): ignored `gemma4:e4b` live reasoning-strip confirmation |
| `71269b5` | polish(cli): clarify reasoning-marker help; add parse→pair glue tests |
| `fc05c91` | docs(claude): reasoning-token stripping is now implemented |
| `b9c8091` | docs: mark reasoning-token stripping complete (README/ROADMAP); note subsystem propagation |
| `494e5f7` | docs(readme): list reasoning-token stripping under what-works-today |

### Components

1. **`primer-core/src/reasoning.rs`** — pure `ReasoningFilter` (state machine: `Outside`/`Inside`; holds back the longest suffix that could be a split-marker prefix so a marker split across chunks never leaks), `default_markers()`, `finalize_visible(had_visible, tail, did_suppress) -> Option<String>` (`None` ⇒ emit error). 19 unit tests incl. split-open/close, multiple blocks, unbalanced (no leak), false-prefix (`<thinking out loud>` ≠ `<think>`), UTF-8 boundaries, Gemma4 asymmetric pair.
2. **`primer-core/src/consts.rs`** — `reasoning::DEFAULT_MARKERS = [("<think>","</think>"), ("<|channel>","<channel|>")]`.
3. **`primer-core/src/error.rs` + `i18n.rs`** — `InferenceError::ReasoningWithoutAnswer` (non-retryable; added to the exhaustive `is_retryable`/`Display` test tables + the Hindi-Devanagari guard) + EN/DE/HI renders.
4. **`primer-inference/src/reasoning_stream.rs`** — shared `process_filtered_chunk(...) -> FilterAction { Nothing, Forward, Final }`; ONE verified implementation, both backends delegate (no drift). 6 direct unit tests.
5. **`ollama.rs` + `openai_compat.rs`** — `pub(crate) reasoning_markers` field, `with_extra_markers(Vec<(String,String)>)` builder (appends to defaults), spawn-task wiring through the shared helper. 3 wiring tests each (real helper, not a mirror) + an `#[ignore]`'d live `gemma4:e4b` confirmation test on ollama.
6. **`primer-engine/src/wiring.rs`** — `BackendParams.reasoning_markers`; `build_backend`'s ollama + openai-compat arms call `.with_extra_markers(...)`. Documented: markers propagate to classifier/extractor/comprehension subsystem backends too (intentional — keeps their JSON clean).
7. **`primer-cli/src/main.rs`** — repeatable `--reasoning-marker <OPEN> <CLOSE>` flag + pure `pair_reasoning_markers` helper (drops a trailing odd value safely). 5 tests incl. a real clap parse→pair glue test.

Spec: `docs/superpowers/specs/2026-05-30-reasoning-token-stripping-design.md`
Plan: `docs/superpowers/plans/2026-05-30-reasoning-token-stripping.md`

### Verification (this session, macOS host, pinned 1.88)
- `cargo +1.88 fmt --all -- --check` → clean.
- `cargo +1.88 clippy --workspace --all-targets -- -D warnings` → clean.
- `cargo +1.88 test --workspace --no-fail-fast` → **942 passed / 0 failed / 4 ignored**.
  - The 4 ignored: this branch's `gemma4_live_reasoning_is_stripped` + 3 pre-existing ignores (incl. the `bm25_score_floor_tripwire` diagnostic). None failed.
- CI on PR #187: all 10 checks SUCCESS.
- A whole-branch holistic review traced the child-safety invariant end-to-end (split + unterminated `<think>` blocks cannot leak a byte; both backends behaviorally identical; `generate()` covered via stream aggregation).

**TOOLCHAIN GOTCHA (cost real time this session):** always run cargo from inside `src/`. `rust-toolchain.toml` (pinning 1.88) lives at `src/rust-toolchain.toml` and is ONLY honored from there. Running from the repo root (or via `--manifest-path`) silently resolves to the user-default `stable` (1.96), whose newer lints (`clippy::manual_is_multiple_of`, `useless_conversion`) fire on **pre-existing untouched** code (`primer-storage`, `primer-knowledge`, test helpers) and produce false "failures." CI uses the pinned 1.88. When in doubt, `cargo +1.88 …` from `src/`.

## What's next — by priority

**First: merge PR #187.** CI is fully green and a holistic review passed. `gh pr merge 187 --squash` (owner's call), then `git branch -D feat/reasoning-token-stripping`.

### Concrete actionable candidates

- **GUI custom-marker editor (the deferred half of this feature; ROADMAP 0.3).** The GUI already gets default stripping for free (it builds the same backends; its `BackendParams.reasoning_markers` is `Vec::new()`). Remaining work: add `reasoning_markers` (array of `{open, close}`) to the GUI backend config struct + `BackendConfigView` + `BackendConfigUpdate` DTOs, a Settings `<textarea>` (one `open<whitespace>close` pair per line, first-whitespace separator), and `settings.js::gather()` must send it (the `BackendConfigUpdate`-has-no-`serde(default)` gotcha — every field mandatory, send `[]` when empty). Acceptance: on a GUI build, Settings → add `[[r]] [[/r]]`, Save & start, a model emitting `[[r]]…[[/r]]` has it stripped; empty textarea ⇒ defaults only. NOT a secret — passes through View/Update verbatim (no Env/Keep dance).
- **`openai_compat.rs` test-module split (low effort, tracked follow-up).** The file is ~755 lines, over the 500 guideline, almost entirely its `#[cfg(test)]` module. Split tests to a sibling file (`openai_compat/tests.rs` or a `tests/` integration file) to bring production code under 500. Same applies as more backends get reasoning wiring tests.
- **Step 1.2.0 — QAIRT install + chatapp_android device validation + run `qnn_bench`** (developer-side; standing highest-value QNN gate). Runbook `docs/devel/qnn-validation-runbook.md`. Acceptance: on the RedMagic 11 Pro via Termux, `cargo run --release --example qnn_bench --features qnn -- --bundle-dir ~/primer-bundles/qwen3-4b --duration-secs 900 --thermal-out ~/storage/shared/primer-thermal.csv` prints a verdict. Decode < 8 tok/s on Qwen3-4B → stop-and-reassess; pass (≥15 tok/s, <3s, ≤70 °C) → flip ROADMAP 1.2 ✅ with numbers + CSV.
- **#157 on-device Termux validation** (developer-side): does `ort-sys`'s build.rs fetch an `aarch64-linux-android` ONNX runtime from cdn.pyke.io? Until proven, Android default stays BM25-only (the `--no-default-features` android CI guard enforces it). This branch did NOT touch the embedder/Android path, so it doesn't change that calculus.
- **Branch protection on `main`** — still overdue; require the `cargo test (default features)` check.

### Open queue (issues — unchanged)

| #   | Title | State |
| --- | --- | --- |
| 170 | Stage B: wire Supertonic 3 as a voice-mode TTS backend (Hindi unlock) | Stage B merged (#175); A.5 spike next |
| 166 | Real-audio multi-utterance Whisper smoke (PR #164 follow-up) | needs human at mic + ggml-small.bin |
| 163 | split backends_common.rs once the test module grows | explicit deferral |
| 135 | bump glib → 0.20+ once Tauri 3 ships (RUSTSEC-2024-0429) | waits on Tauri 3 |
| 98  | split tests/common/sweep.rs into bm25/hybrid submodules | defer until 3rd locale |
| 41/40 | data/ingest disambiguation-regex scope / per-source attribution | self-deferred |
| 22  | primer-knowledge: cache prepared statements (Phase 0.2) | self-deferred |
| 21  | CLI: separate `--languages` preference list from bound `--language` | self-deferred |
| 20  | i18n placeholder validator false-fails on translator narrative | self-deferred |

## Open decisions / risks

- **Custom markers propagate to subsystem backends.** `--reasoning-marker` pairs (and the built-in defaults) apply to the classifier/extractor/comprehension backends too, because they share `BackendParams` via `build_backend`. Intentional and documented on the field (stripping keeps subsystem JSON clean). If a future use-case needs chat-only custom markers, the fix is separate `chat_` vs `subsystem_` marker fields.
- **Gemma4 marker bytes are doc-sourced, not yet device-confirmed.** `("<|channel>","<channel|>")` comes from the ollama gemma4 model docs. The `#[ignore]`'d `gemma4_live_reasoning_is_stripped` test confirms them against a running `ollama` with `gemma4:e4b` pulled — run it on demand; if the real stream diverges, the cure is a one-line edit to `consts::reasoning::DEFAULT_MARKERS`.
- **Reasoning-without-answer shows a friendly "try again", not a partial.** By design (better than raw CoT to a child). If a flaky model frequently truncates mid-thought, the child sees the fallback repeatedly — acceptable for v1.
- **QNN / Android risks carried forward** (ort-sys vendor liability, QNN ABI smoke unverified vs real libGenie.so, qnn_bench numbers device-unmeasured). See prior handoffs under `docs/handoffs/`.

## Patterns to reuse, not reinvent

New from this session:

- **Stream-spanning text transforms need a stateful filter, not per-chunk `str::replace`.** Markers (and any multi-token pattern) arrive split across chunks; hold back the longest suffix that could be a prefix of any target, emit the rest, decide at `finish()`. `ReasoningFilter` is the template.
- **Put the shared per-chunk step in one helper both backends delegate to** (`reasoning_stream::process_filtered_chunk` + a `FilterAction` enum). The two stream loops were 95% identical; extracting it killed the duplication AND gave the wiring tests a real code path instead of a mirror. Do this the moment a second backend needs the same logic.
- **`generate()` aggregates `generate_stream()` — fix the stream, get the non-streaming path free.** The classifier/extractor/comprehension JSON parsing benefits without separate work.
- **User-facing strings route through `render_inference_error` (the single i18n boundary), never string-literaled in a backend.** A new `InferenceError` variant + EN/DE/HI arms is the whole job; the exhaustive per-locale `match` makes the compiler force all locales. Keep the per-variant `error.rs` test tables (`is_retryable_truth_table`, `display_produces_dev_facing_english`) and the Hindi-Devanagari guard exhaustive — add the new variant to each.
- **A boolean signal should be a `bool`, not a `usize` zero-check.** Caught in review: `total_visible: usize` used only as `== 0` became `had_visible: bool`. Model the question you're actually asking.
- **Run cargo from `src/` so the 1.88 pin is honored.** (See the toolchain gotcha above — the single most time-wasting trap in this repo right now.)

Carried forward (prior handoffs): feature-aware compile-time-conditional defaults; flipping a cargo default has CI blast radius (audit every job); a "prove the download works" CI gate is build-level not run-level; the final holistic review catches cross-file staleness; pure-core + thin-device-example for hardware-gated harnesses; conservative verdict aggregations (min/p95) beat means; not-a-secret config fields skip the View/Update Keep/Env dance; distinct-error-messages-for-build-vs-runtime.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer
git status                                   # clean; work pushed on feat/reasoning-token-stripping
gh pr view 187                               # the open PR for this session's work (CI green)

# === Merge (CI is green) ===
gh pr merge 187 --squash                     # owner's call
git branch -D feat/reasoning-token-stripping # after merge

# === Re-verify locally (from src/, ALWAYS with +1.88 or from inside src/) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo +1.88 test --workspace --no-fail-fast        # 942 pass / 0 fail / 4 ignored

# === Targeted reasoning tests ===
~/.cargo/bin/cargo +1.88 test -p primer-core reasoning          # pure filter (19)
~/.cargo/bin/cargo +1.88 test -p primer-inference reasoning     # shared helper + wiring
~/.cargo/bin/cargo +1.88 test -p primer-cli reasoning_marker    # flag parse→pair (5)

# === Run the ignored live Gemma4 confirmation (needs `ollama serve` + `ollama pull gemma4:e4b`) ===
~/.cargo/bin/cargo +1.88 test -p primer-inference gemma4_live -- --ignored --nocapture

# === Try it interactively against a reasoning model ===
~/.cargo/bin/cargo run --bin primer -- --backend ollama --model qwen3:8b          # <think> stripped
~/.cargo/bin/cargo run --bin primer -- --backend ollama --model gemma4:e4b        # <|channel> stripped
~/.cargo/bin/cargo run --bin primer -- --backend ollama --model X \
    --reasoning-marker '[[r]]' '[[/r]]'                                            # append a custom pair
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- **This session's caveat:** the feature is verified on a macOS *host* under pinned 1.88 AND CI is fully green on PR #187. The Gemma4 markers are doc-sourced; the `#[ignore]`'d live test is the device-confirmation path. The GUI custom-marker EDITOR is deferred (the GUI still strips with the defaults).
- The GUI is a full app — when a future brief calls it a scaffold, it's wrong; trust the code.
