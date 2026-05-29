# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-29 — Added the **QNN backend + bundle/QAIRT-lib-dir pickers to the Tauri desktop GUI** (`primer-gui`). The CLI has had `--backend qnn --qnn-bundle-dir <path>` for a while; the GUI's `wiring.rs` hardcoded `qnn_bundle_dir: None` / `qnn_qairt_lib_dir: None` and its select/validation rejected the kind. Now `qnn` is fully selectable from Settings → Inference backend with bundle-dir + optional QAIRT-lib-dir path fields. **Committed on branch `feat/gui-qnn-bundle-picker` (NOT pushed, NO PR yet)** — see "Exact commands". **124 `primer-gui` tests pass on the default build (+9 new); 123 with `--features qnn` (the non-feature build-hint test compiles out)**; clippy clean on both; `cargo fmt --check` clean.

## What we shipped this session — QNN GUI bundle picker

**Branch:** `feat/gui-qnn-bundle-picker`. **Commit:** `b471c30`. Not pushed; no PR opened (owner's call — see below). `git status` is otherwise clean.

8 files changed (`git show --stat b471c30`):

- **`src/crates/primer-gui/src/config.rs`** — `BackendConfig` gains `qnn_bundle_dir: Option<PathBuf>` + `qnn_qairt_lib_dir: Option<PathBuf>` (both default `None` via the struct's `#[serde(default)]`). Threaded through `BackendConfigView` (pass-through — **not secrets**, unlike API keys, so no redaction) + `BackendConfigUpdate` (mandatory IPC field — `BackendConfigUpdate` has NO `#[serde(default)]`) + `From`/`into_config` (verbatim move, no `Keep`/`Env` dance). The two existing `BackendConfigUpdate`-deserialising test JSONs were updated to carry the new mandatory fields. 5 new tests (default-None; older-config-loads-with-defaults; round-trip-through-disk; view-pass-through; update-pass-through).
- **`src/crates/primer-gui/src/wiring.rs`** — feeds `qnn_bundle_dir` / `qnn_qairt_lib_dir` from cfg into `BackendParams` (replaced the hardcoded `None`s + their stale "GUI does not yet expose a picker" comment). Added the `"qnn"` arm to `resolve_main_model` returning the `"qnn-pending"` placeholder, plus a post-`build_backend` rebind of `main_model` to `backend.name()` for the qnn kind (mirrors the CLI's `primer-meta.json`-authoritative model id). 2 new tests: `resolve_main_model_qnn_returns_placeholder` (pure); `qnn_without_feature_surfaces_build_hint` (`#[cfg(not(feature = "qnn"))]`-gated — proves the error-inline contract on a default build).
- **`src/crates/primer-gui/src/validation.rs`** — `validate_backend` accepts `"qnn"` (structural-only; feature/bundle-dir/libGenie checks stay in the wiring layer, mirroring how ollama-without-model is a wiring check). 2 new accept tests (with + without bundle dir).
- **`src/crates/primer-gui/Cargo.toml`** — new `qnn` feature forwarding to `primer-engine/qnn` (so a packaged QNN GUI build has `QnnBackend` in the dep graph and `build_qnn_backend`'s real arm constructs).
- **`src/crates/primer-gui/ui/index.html`** — qnn `<option>` in the backend `<select>`; two conditional path fields (`f-backend-qnn-bundle-dir`, `f-backend-qnn-qairt-lib-dir`, each with its `-field` wrapper id) with hint text. **All 4 new DOM ids cross-checked against `settings.js` `getElementById` refs — 0 orphans either direction.**
- **`src/crates/primer-gui/ui/settings.js`** — dom refs, `populate()` (sets the two inputs), `applyBackendKindReveal()` (shows both fields only when `kind === "qnn"`), and `gather()` (sends `qnn_bundle_dir` / `qnn_qairt_lib_dir` as `orNull(...)` — mandatory in the payload, `null` when blank).
- **`ROADMAP.md`** — added a "GUI wiring: QNN backend + bundle/QAIRT-lib pickers" checked line under Phase 1.2; updated the GUI parity bullet to include `qnn`.
- **`CLAUDE.md`** — the GUI backend-parity paragraph (line ~64) updated: `qnn` now in the selectable list; documented that the QNN option is always-shown/errors-inline, the `qnn_*` mandatory-IPC-field gotcha, the not-a-secret pass-through, the `primer-gui/qnn` feature, and the `resolve_main_model` `"qnn-pending"` → `backend.name()` rebind.
- **README.md: NOT changed** — it already says the settings modal mirrors "every CLI flag (backend, model, locale, embedder, …)", which now correctly includes qnn (same reasoning the openai-compat session used).

### Verification (this session)
- `cargo test -p primer-gui` → **124 passed; 0 failed** (default build; +9 new tests).
- `cargo test -p primer-gui --features qnn` → **123 passed; 0 failed** (the `qnn_without_feature_surfaces_build_hint` test is correctly compiled out).
- `cargo clippy -p primer-gui --all-targets` → clean. Same with `--features qnn`.
- `cargo check -p primer-gui --features qnn` → Finished (compiles clean on macOS host; `primer-qnn-sys` returns `PlatformUnsupported` at runtime off-Android).
- `cargo fmt --check -p primer-gui` → clean.
- **Not done:** no end-to-end run against a real Android/QNN device (the `QnnBackend` is Android-only at runtime and the bundle/libGenie.so aren't present here). The GUI wiring is host-tested; the runtime path stays device-unverified — exactly like the CLI's QNN wiring.

## What's next — by priority

**First: push + PR this work if you want it merged.** Suggested: push `feat/gui-qnn-bundle-picker`, open a PR. Self-contained (one crate + 2 doc files), host-tested, clippy/fmt clean on both feature combos.

### Concrete actionable candidates (host-draftable unless noted)

- **Manual GUI smoke of openai-compat** (still open from last session; developer-side, needs a local oMLX/LM-Studio/vLLM server). Acceptance: `cargo run --bin primer-gui`, Settings → openai-compat + model + URL, Save & start, send a turn, see a streamed reply; then flip the embedder to openai-compat on a `--features primer-gui/openai-compat-embedding` build and confirm a hybrid query.
- **Manual GUI smoke of QNN** (developer-side; needs an Android/QNN device + a real Genie bundle, OR a `--features qnn` build on-device). Acceptance: on a `--features qnn` build, Settings → qnn + bundle dir, Save & start, see a streamed reply with the banner showing `qnn:<model>`. On a default build, confirm selecting qnn fails with the "rebuild with --features qnn" hint inline (already unit-tested, but verify the GUI surfaces it as a toast/error rather than a crash).
- **Flip embedding default-on** (unchanged): add a CI job proving the `cdn.pyke.io` ort-runtime download works on Linux+macOS, then flip `default = ["embedding"]` in `primer-cli/Cargo.toml` (consider the GUI too) + the `--embedder-backend` default.
- **#157 on-device Termux validation** (developer-side; standing highest-value follow-up): does `ort-sys`'s build.rs fetch an `aarch64-linux-android` ONNX runtime from cdn.pyke.io? Until proven, the Android default stays `--embedder-backend none`.
- **Step 1.2.6 — QNN benchmark + thermal harness** (`examples/qnn_bench.rs`; target 15+ tok/s decode on Qwen3-4B W4A16, TTFT < 3s, peak < 70°C — top host-draftable QNN item) and **Step 1.2.0 — QAIRT install + chatapp_android device validation** (developer-side; runbook at `docs/devel/qnn-validation-runbook.md`; decode < 8 tok/s on Qwen3-4B → stop-and-reassess).
- **Branch protection on `main`** — still overdue; require the `cargo test (default features)` check. Revisit PR #169's `paths-ignore` at the same time.

### Open queue (issues — unchanged)

| #   | Title | State |
| --- | --- | --- |
| 170 | Stage B: wire Supertonic 3 as a voice-mode TTS backend (Hindi unlock) | Stage B merged (#175); A.5 spike next (developer-side) |
| 166 | Real-audio multi-utterance Whisper smoke (PR #164 follow-up) | needs human at mic + ggml-small.bin |
| 163 | split backends_common.rs once the test module grows | explicit deferral |
| 135 | bump glib → 0.20+ once Tauri 3 ships (RUSTSEC-2024-0429) | waits on Tauri 3 |
| 98  | split tests/common/sweep.rs into bm25/hybrid submodules | defer until 3rd locale |
| 41/40 | data/ingest disambiguation-regex scope / per-source attribution | self-deferred |
| 22  | primer-knowledge: cache prepared statements (Phase 0.2) | self-deferred |
| 21  | CLI: separate `--languages` preference list from bound `--language` | self-deferred |
| 20  | i18n placeholder validator false-fails on translator narrative | self-deferred |

## Open decisions / risks

- **No on-device QNN smoke ran this session.** The wiring is unit-tested (placeholder model id; non-feature build-hint error; config round-trip). The actual `QnnBackend::new` → streaming path is Android-only and device-unverified — same caveat as the CLI's QNN wiring. Don't tell a user "the GUI runs on the NPU now" until a real device run passes.
- **The QNN option is always visible, even on a default build.** This was a deliberate choice (mirrors openai-compat-embedder "error inline" over "hide the option"). The tradeoff: a default-build user sees a qnn option that can never construct on this build — it errors at session-start with the rebuild hint. If that's confusing in practice, the Phase-B alternative is the feature-gated-UI approach (expose the build's feature flags to the frontend via a Tauri command and hide the option). Not done now; flagged here.
- **`BackendConfigUpdate` still has no `#[serde(default)]`** — every backend field is mandatory in the `update_settings` IPC payload. This PR added two more (`qnn_bundle_dir` / `qnn_qairt_lib_dir`) and updated `gather()` + the two existing test JSONs in lockstep. If you add more backend fields, you MUST update `gather()` and any `BackendConfigUpdate`-deserialising test JSON or saves silently fail to deserialize. (Documented in CLAUDE.md.)
- (… plus all carried-forward decisions/risks from prior briefs — ort-sys vendor liability, pykeio anti-AI-PR policy, small-context budget untested on a real 4K tokeniser, QNN ABI smoke unverified vs real libGenie.so, CodeQL not skipped on docs-only. See prior handoffs under `docs/handoffs/`.)

## Patterns to reuse, not reinvent

New from this session:

- **"Always-show + error-inline beats hide-behind-a-feature for a static-HTML settings form."** Rather than plumb the build's cargo-feature state to the frontend to conditionally render the qnn option, we always render it and let `build_qnn_backend`'s `not(feature = "qnn")` arm surface the rebuild hint at session-start. Simpler, fully host-testable, and consistent with the openai-compat-embedder precedent. Gate the *test* of that behaviour with `#[cfg(not(feature = "qnn"))]` so it stays deterministic.
- **"Not-a-secret config fields skip the View/Update redaction dance."** API keys need `ApiKeySourceView` (redact) + `ApiKeyUpdate` (Keep/Env/Inline). Paths don't — `qnn_bundle_dir` is a plain `Option<PathBuf>` that passes through both DTOs verbatim. Don't over-engineer a Keep variant for non-secret fields.
- **"A placeholder model id + post-construction rebind is the qnn-model pattern."** The qnn model comes from the bundle's `primer-meta.json`, not from settings, so `resolve_main_model` returns `"qnn-pending"` and the caller rebinds to `backend.name()` after `build_backend`. The GUI mirrors the CLI here exactly — keep them in sync.
- **"Cross-check every new DOM id in both directions before trusting the frontend."** `grep -oE 'id="f-…"' index.html` vs `grep -oE 'getElementById("f-…")' settings.js` — 0 orphans either way. An id added to one file but not the other throws `null` at populate-time with no compile error to catch it.

Carried forward (prior handoff trail): mirror-don't-parameterise-a-load-bearing-secret-path, a-plain-derive-Deserialize-DTO-makes-every-new-field-mandatory, read-the-actual-frontend-architecture-before-editing, stale-status-in-CLAUDE.md-is-a-real-hazard, guard-host-cfg-bug-with-rustc-target, regression-guard-must-prove-teeth, put-cross-crate-naming-const-in-shared-layer, name-config-knob-by-constraint-not-backend, distinct-error-messages-for-build-vs-runtime, feature-gate-the-dispatch-arm-not-the-struct-shape, public-re-export-protecting-call-sites.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer
git status                        # clean; work is committed on feat/gui-qnn-bundle-picker
git log --oneline -1 feat/gui-qnn-bundle-picker   # b471c30

# === Push + PR this session's work (owner's call) ===
git push -u origin feat/gui-qnn-bundle-picker
gh pr create --fill

# === Re-verify (from src/, rustup proxy to dodge Homebrew rust) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test  -p primer-gui                 # expect 124 passed
~/.cargo/bin/cargo test  -p primer-gui --features qnn  # expect 123 passed
~/.cargo/bin/cargo clippy -p primer-gui --all-targets
~/.cargo/bin/cargo clippy -p primer-gui --all-targets --features qnn
~/.cargo/bin/cargo fmt --check -p primer-gui

# === Manual GUI smoke of QNN (needs --features qnn + a real Genie bundle on Android) ===
~/.cargo/bin/cargo run --bin primer-gui --features qnn
#   Settings → Inference backend → "qnn", set the bundle dir, Save & start new session, send a turn.
#   On a DEFAULT build, selecting qnn must error inline with the "rebuild with --features qnn" hint.
```

Carried-forward smokes (unchanged):

```bash
# Manual GUI smoke of openai-compat (needs a local OpenAI-compatible server):
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run --bin primer-gui
#   Settings → openai-compat + model + URL, Save & start, send a turn. Embedder: rebuild with --features primer-gui/openai-compat-embedding.

# German retrieval-quality regression benchmarks:
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_de
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_quality_hybrid_de

# Python ingestion pipeline tests (uv-only — never pip directly):
cd /Users/hherb/src/primer/data/ingest && .venv/bin/pytest tests/
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- **This session's caveat:** the QNN GUI picker is wired + host-tested but NOT smoke-tested on a real Android/QNN device. Don't tell a user "the GUI runs on the NPU now" until a real device run passes — same caveat as the CLI's QNN wiring.
- The GUI is a full app — when a future brief calls it a scaffold, it's wrong; trust the code.
