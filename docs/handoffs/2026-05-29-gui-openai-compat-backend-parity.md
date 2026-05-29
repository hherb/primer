# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-29 — Added **OpenAI-compatible backend + embedder parity to the Tauri desktop GUI** (`primer-gui`). The CLI has spoken `--backend openai-compat` / `--embedder-backend openai-compat` for a while; the GUI hardcoded `localhost:8000` + `api_key: None` and its settings/validation rejected the kind. Now `openai-compat` is fully selectable from Settings → Inference backend (server URL + optional `OPENAI_COMPAT_API_KEY`-or-inline key) and the embedder picker. **Work is UNCOMMITTED on `main`** — see "Exact commands" for the branch+commit step. **115 `primer-gui` tests pass (0 failed)**; clippy clean on default AND `--features primer-gui/openai-compat-embedding`; `cargo fmt --check` clean.

## Important correction discovered this session

**`primer-gui` is a fully working desktop app, not a scaffold.** The old "empty window only (step 2 of 10)" status in CLAUDE.md / earlier briefs was badly stale. The actual crate already ships: launch session-picker, streaming chat (`send_message` → `primer://chunk` / `primer://turn_complete`, mid-stream `cancel_response`), settings modal, session persistence + resume (with the locale-inheritance discipline), a pedagogy-signals + learner + turn-list sidebar, and voice mode behind `--features speech`. CLAUDE.md line 38 has been rewritten to describe reality + the openai-compat parity + the `BackendConfigUpdate`-has-no-serde-default gotcha.

## What we shipped this session — OpenAI-compat GUI parity

**Branch:** none yet — changes are uncommitted in the `main` working tree. **Commit SHAs:** _none yet_ (owner to branch + commit; see below). `git status` shows exactly these 8 files modified, nothing else (no stray empty-diff files).

Content-changed files (8):

- **`src/crates/primer-gui/src/config.rs`** — `BackendConfig` gains `openai_compat_url: String` (default `http://localhost:8000`) + `openai_compat_api_key_source: ApiKeySource` (default `Env` → reads `OPENAI_COMPAT_API_KEY`). `EmbedderConfig` gains `openai_compat_url: Option<String>` (falls back to the backend URL). Threaded through `BackendConfigView` (redacted) + `BackendConfigUpdate` (write) + `From`/`into_config`. 4 new unit tests (redaction of the oc key, default URL, `Keep` preserves the oc key independently of the cloud key, older-config-without-the-fields loads with serde defaults).
- **`src/crates/primer-gui/src/wiring.rs`** — resolves the oc key independently of the cloud key (`Inline` → key, `Env` → `OPENAI_COMPAT_API_KEY`), feeds `BackendParams.openai_compat_url`/`_api_key` from cfg instead of the old hardcoded values, adds the `"openai-compat"` arm to `resolve_main_model` (model required) and to `build_embedder` (URL falls back to the backend URL; reuses the same resolved key). 3 new tests (backend without/with model, embedder without model).
- **`src/crates/primer-gui/src/validation.rs`** — `validate_backend` / `validate_embedder` accept `openai-compat`. 2 new accept tests. (Model-required stays a wiring-layer check, matching how ollama-without-model is a wiring test — validation is structural-only.)
- **`src/crates/primer-gui/Cargo.toml`** — new `openai-compat-embedding` feature forwarding to `primer-engine/openai-compat-embedding` (mirrors `embedding` / `ollama-embedding`).
- **`src/crates/primer-gui/ui/index.html`** — backend `<select>` gets the openai-compat option; new server-URL field; new openai-compat API-key fieldset (env/inline radios + password input); embedder `<select>` gets the option + an embedder server-URL field.
- **`src/crates/primer-gui/ui/settings.js`** — dom refs, `state.hasOcInlineKey`, populate/reveal/gather + a sibling `applyOcApiKeyReveal` / `resolveOcApiKeyUpdate` (kept parallel to the cloud path rather than parameterised so the long-standing cloud code bisects cleanly). `gather()` now sends `openai_compat_url` + `openai_compat_api_key_source` (mandatory — `BackendConfigUpdate` has no `#[serde(default)]`) and the embedder `openai_compat_url`.
- **`src/crates/primer-gui/ui/index.html`** — the static settings form gains the openai-compat backend `<option>`, a server-URL field, a full openai-compat API-key fieldset (env/inline radios + password input, hidden until that backend is picked), the embedder `<option>`, and an embedder server-URL field. **All 11 new DOM ids were cross-checked against the `settings.js` `getElementById` refs (`comm`-diff: 0 orphans either direction)** — important because an earlier edit pass landed the `settings.js` refs *before* the HTML, which would have thrown `null` at populate-time.
- **`ROADMAP.md`** — the openai-compat backend line's "Follow-ups: GUI wiring (deferred…)" note updated to record GUI parity landing, with a live-server smoke as the remaining follow-up.
- **`CLAUDE.md`** — line ~64 GUI paragraph rewritten (see correction above).
- **README.md: NOT changed** — it already describes `primer-gui` as a full app whose settings modal mirrors "every CLI flag (backend, model, locale, embedder, …)", which now correctly includes openai-compat. (An earlier draft of this brief wrongly claimed a README edit; there is none.)

### Verification (this session)
- `cargo test -p primer-gui` → **115 passed; 0 failed** (+9 openai-compat tests across config/wiring/validation, run by name and individually confirmed green).
- `cargo clippy -p primer-gui --all-targets` → clean. Same with `--features primer-gui/openai-compat-embedding`.
- `cargo build -p primer-gui --features primer-gui/openai-compat-embedding` → Finished, no warnings. (On a default build, selecting the openai-compat *embedder* surfaces the "requires the openai-compat-embedding cargo feature" error inline — the openai-compat *backend* needs no feature gate.)
- **Not done:** no live end-to-end against a real oMLX/LM-Studio/vLLM server (no server running here); no JS test harness exists in this crate, so the frontend is covered indirectly by the Rust view/update round-trip tests. A manual GUI smoke is the obvious follow-up (see below).

## What's next — by priority

**First: branch + commit this work, then it's mergeable.** Suggested branch `feat/gui-openai-compat-backend`, then open a PR. It's self-contained (one crate + 2 doc files), host-tested, clippy/fmt clean.

### Concrete actionable candidates (host-draftable unless noted)

- **Manual GUI smoke of openai-compat** (developer-side; needs a local OpenAI-compatible server). Acceptance: `cargo run --bin primer-gui` (or `--features speech`), Settings → Inference backend → OpenAI-compatible, set model + server URL, Save & start new session, send a turn, see a streamed reply. Then flip the embedder to openai-compat on a `--features primer-gui/openai-compat-embedding` build and confirm a hybrid query.
- **QNN bundle picker in the GUI** (the other backend-parity gap). `wiring.rs` still hardcodes `qnn_bundle_dir: None` / `qnn_qairt_lib_dir: None`. Surface a bundle-dir + qairt-lib-dir picker in Settings and plumb them into `BackendParams`. Lower value until the QNN backend is device-validated (it would wire a path into an untested runtime), but the wiring + validation are host-testable now.
- **Flip embedding default-on** (unchanged from prior briefs): add a CI job proving the `cdn.pyke.io` ort-runtime download works on Linux+macOS, then flip `default = ["embedding"]` in `primer-cli/Cargo.toml` (and consider the GUI) + the `--embedder-backend` default.
- **#157 on-device Termux validation** (developer-side; the standing highest-value follow-up): does `ort-sys`'s build.rs fetch an `aarch64-linux-android` ONNX runtime from cdn.pyke.io? Until proven, the Android default stays `--embedder-backend none`.
- **Step 1.2.6 — QNN benchmark + thermal harness** (top host-draftable QNN item) and **Step 1.2.0 — QAIRT install + chatapp_android device validation** (developer-side; runbook at `docs/devel/qnn-validation-runbook.md`; decode < 8 tok/s on Qwen3-4B → stop-and-reassess).
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

- **No live openai-compat smoke ran this session.** The wiring is unit-tested (the `OpenAiCompatBackend` constructs without network I/O, so `openai_compat_with_model_constructs` proves construction, not a real round-trip). A real server smoke is the gating check before claiming "openai-compat works in the GUI" to a user.
- **`BackendConfigUpdate` has no `#[serde(default)]`** — every backend field is mandatory in the `update_settings` IPC payload. The frontend `gather()` was updated to send the two new fields; if you add more backend fields, you MUST update `gather()` in lockstep or saves silently fail to deserialize. (Now documented in CLAUDE.md.)
- The openai-compat embedder still needs the `openai-compat-embedding` cargo feature at build time; a default GUI build surfaces the feature-missing error inline (good — strictly better than a silent BM25 downgrade), but a packaged GUI that wants hybrid-via-openai-compat must be built with that feature.
- (… plus all carried-forward decisions/risks from prior briefs — ort-sys vendor liability, pykeio anti-AI-PR policy, small-context budget untested on a real 4K tokeniser, QNN ABI smoke unverified vs real libGenie.so, CodeQL not skipped on docs-only. See prior handoffs under `docs/handoffs/`.)

## Patterns to reuse, not reinvent

New from this session:

- **"Mirror, don't parameterise, a load-bearing secret path."** The openai-compat API-key UI is a verbatim sibling of the cloud one (`applyOcApiKeyReveal` / `resolveOcApiKeyUpdate`, `hasOcInlineKey`) rather than a generalisation of the cloud functions — the cloud path stays byte-identical so a future bisect for a key-handling regression lands on the right change, and the two secrets resolve independently (no cross-contamination on save).
- **"A plain `#[derive(Deserialize)]` DTO makes every new field a mandatory IPC field."** Adding a field to `BackendConfigUpdate` (no `serde(default)`) silently breaks the save path until the frontend sends it. Always pair a backend-DTO field add with the `gather()` send + an `older_config_*_loads_with_defaults` test on the *disk* type (which DOES have `serde(default)`).
- **"Read the actual frontend architecture before editing it."** This crate's settings UI is a static HTML form (`index.html`) + `populate()`/`gather()` in `settings.js`, NOT a JS element-builder — an early wrong assumption cost an edit round. The form-field id convention is `f-<area>-<field>` mirrored into `dom.fields`.
- **"Stale status in CLAUDE.md/handoffs is a real hazard."** The "GUI is an empty window" claim sent this session looking for step-3 wiring that shipped long ago. Trust the code over the prose; fix the prose when you find the drift.

Carried forward (prior handoff trail): guard-host-cfg-bug-with-rustc-target, regression-guard-must-prove-teeth, literal-match-not-regex-when-needle-is-code, self-validate-the-guard's-assumption, honest-preconditions, read-CONTRIBUTING-before-upstream-PR, vendor-verbatim-patch-minimally, put-cross-crate-naming-const-in-shared-layer, name-config-knob-by-constraint-not-backend, route-every-reader-through-one-effective-value-method, genuine-TDD-red-even-for-trivial-pure-functions, distinct-error-messages-for-build-vs-runtime, feature-gate-the-dispatch-arm-not-the-struct-shape, public-re-export-protecting-call-sites, `Script`-enum-driven mock.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer
git status                       # 8 real content changes; session.rs + NEXT_SESSION.md have empty diffs (fmt stat-touch)

# === Branch + commit this session's work (owner's call) ===
git checkout -b feat/gui-openai-compat-backend
git add -A
git commit            # message e.g. "feat(gui): OpenAI-compatible backend + embedder parity in Settings"
git push -u origin feat/gui-openai-compat-backend
gh pr create --fill

# === Re-verify (from src/, rustup proxy to dodge Homebrew rust) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-gui                                   # expect 115 passed
~/.cargo/bin/cargo clippy -p primer-gui --all-targets
~/.cargo/bin/cargo clippy -p primer-gui --all-targets --features primer-gui/openai-compat-embedding
~/.cargo/bin/cargo build  -p primer-gui --features primer-gui/openai-compat-embedding

# === Manual GUI smoke of openai-compat (needs a local server, e.g. oMLX/LM Studio/vLLM) ===
# Start the server, then:
~/.cargo/bin/cargo run --bin primer-gui
#   Settings → Inference backend → "OpenAI-compatible", set model + server URL, Save & start new session, send a turn.
#   For the embedder, rebuild with --features primer-gui/openai-compat-embedding first.
```

Carried-forward smokes (unchanged):

```bash
# Hindi preview locale (developer-only):
cd /Users/hherb/src/primer/src
RUST_LOG=primer::prompt_pack=warn,info ~/.cargo/bin/cargo run --bin primer -- \
    --backend stub --name Aarav --age 8 --language hi --no-persist

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
- **This session's caveat:** openai-compat GUI parity is wired + unit-tested but NOT smoke-tested against a real server. Don't tell a user "the GUI talks to LM Studio/vLLM now" until the manual smoke above passes.
- The GUI is a full app — when a future brief calls it a scaffold, it's wrong; trust the code.
